//! Engine-generic, V8-free async runtime primitives: a fixed-size threadpool and a timer queue.
//! Holds NO V8 handles — jobs/timers carry a `u64` id that `v8host` maps to a PromiseResolver.

use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

pub type JobResult = Result<(), String>;
pub type Job = Box<dyn FnOnce() -> JobResult + Send + 'static>;

pub struct Pool {
    job_tx: SyncSender<(u64, Job, crate::async_limits::JobLease)>,
    completion_rx: std::sync::Mutex<Receiver<(u64, JobResult, crate::async_limits::JobLease, crate::async_limits::QueueTicket)>>,
    _workers: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(workers: usize) -> Self {
        Self::with_capacity(workers, crate::async_limits::policy().worker_queue_items)
    }
    pub fn with_capacity(workers: usize, capacity: usize) -> Self {
        let (job_tx, job_rx) =
            mpsc::sync_channel::<(u64, Job, crate::async_limits::JobLease)>(capacity);
        let (done_tx, completion_rx) = mpsc::channel::<(
            u64,
            JobResult,
            crate::async_limits::JobLease,
            crate::async_limits::QueueTicket,
        )>();
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let mut handles = Vec::new();
        for _ in 0..workers.max(1) {
            let job_rx = job_rx.clone();
            let done_tx = done_tx.clone();
            handles.push(std::thread::spawn(move || loop {
                // Lock only to dequeue; release before running the (possibly long) job.
                let next = { job_rx.lock().unwrap().recv() };
                match next {
                    Ok((id, job, lease)) => {
                        let res = if lease.cancel.cancelled() {
                            Err("AsyncCancelled".into())
                        } else {
                            job().map_err(crate::async_limits::diagnostic)
                        };
                        let _guard = crate::async_limits::delivery_guard(&lease.cancel);
                        if _guard.is_some() {
                            let _ = done_tx.send((
                                id,
                                res,
                                lease,
                                crate::async_limits::QueueTicket::new(0),
                            ));
                        }
                    }
                    Err(_) => break, // all senders dropped → pool shutting down
                }
            }));
        }
        Pool {
            job_tx,
            completion_rx: std::sync::Mutex::new(completion_rx),
            _workers: handles,
        }
    }

    #[cfg(test)]
    pub fn submit(&self, id: u64, job: Job) {
        crate::async_limits::resume_delivery();
        self.try_submit(id, job, crate::async_limits::domain().job(None, 0).unwrap())
            .unwrap();
    }
    pub fn try_submit(
        &self,
        id: u64,
        job: Job,
        lease: crate::async_limits::JobLease,
    ) -> Result<(), String> {
        self.job_tx
            .try_send((id, job, lease))
            .map_err(|_| "AsyncQueueFull".into())
    }
    pub fn try_recv_completed(&self) -> Option<(u64, JobResult, crate::async_limits::JobLease)> {
        let (id, result, lease, _queue) = self.completion_rx.lock().unwrap().try_recv().ok()?;
        Some((id, result, lease))
    }

}

#[derive(Clone, Copy, Debug)]
pub enum TimerKind {
    Deadline(std::time::Instant),
    Frame(u64), // resolve when the frame counter reaches this target
}

pub struct TimerQueue {
    entries: Vec<(u64, TimerKind)>,
}

impl TimerQueue {
    pub fn new() -> Self { TimerQueue { entries: Vec::new() } }
    pub fn push(&mut self, id: u64, kind: TimerKind) { self.entries.push((id, kind)); }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }

    /// Remove a pending timer by id (used by the ledger teardown when its owning plugin unloads,
    /// so a dropped continuation's timer stops keeping the frame detour alive).  Returns true if a
    /// timer with that id was present.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(tid, _)| *tid != id);
        self.entries.len() != before
    }

    pub fn due(&mut self, now: std::time::Instant, frame: u64) -> Vec<u64> {
        self.due_limited(now, frame, usize::MAX)
    }
    pub fn due_limited(&mut self, now: std::time::Instant, frame: u64, limit: usize) -> Vec<u64> {
        if limit == 0 {
            return Vec::new();
        }
        TIMER_EXAMINED.fetch_add(
            self.entries.len() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut ready = Vec::new();
        self.entries.retain(|(id, kind)| {
            let is_due = match kind {
                TimerKind::Deadline(t) => now >= *t,
                TimerKind::Frame(target) => frame >= *target,
            };
            if is_due && ready.len() < limit {
                ready.push(*id);
                false
            } else {
                true
            }
        });
        ready
    }
}

impl Default for TimerQueue { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
    #[test]
    fn bounded_worker_queue_rejects_without_retaining_the_rejected_job() {
        crate::async_limits::resume_delivery();
        let pool = Pool::with_capacity(1, 2);
        let d = crate::async_limits::Domain::new(crate::async_limits::AsyncPolicy {
            jobs_global: 4,
            jobs_per_owner: 4,
            ..Default::default()
        });
        let (entered, wait) = std::sync::mpsc::channel();
        let (release, rx) = std::sync::mpsc::channel();
        pool.try_submit(
            1,
            Box::new(move || {
                entered.send(()).unwrap();
                rx.recv().unwrap();
                Ok(())
            }),
            d.job(None, 32).unwrap(),
        )
        .unwrap();
        wait.recv().unwrap();
        for id in [2, 3] {
            pool.try_submit(id, Box::new(|| Ok(())), d.job(None, 32).unwrap())
                .unwrap();
        }
        assert_eq!(
            pool.try_submit(
                4,
                Box::new(|| panic!("rejected work must not run")),
                d.job(None, 32).unwrap()
            )
            .unwrap_err(),
            "AsyncQueueFull"
        );
        assert_eq!(d.jobs.snapshot().items, 3);
        release.send(()).unwrap();
        let mut ids = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while ids.len() < 3 {
            if let Some((id, result, _lease)) = pool.try_recv_completed() {
                assert!(result.is_ok());
                ids.push(id);
            } else {
                assert!(std::time::Instant::now() < deadline);
                std::thread::yield_now();
            }
        }
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(d.jobs.snapshot().items, 0);
    }
    #[test]
    fn limited_due_selection_retains_insertion_order_and_zero_retains_everything() {
        let mut q = TimerQueue::new();
        let now = std::time::Instant::now();
        q.push(1, TimerKind::Frame(0));
        q.push(2, TimerKind::Deadline(now));
        q.push(3, TimerKind::Frame(0));
        assert!(q.due_limited(now, 0, 0).is_empty());
        assert_eq!(q.len(), 3);
        assert_eq!(q.due_limited(now, 0, 1), vec![1]);
        assert_eq!(q.due_limited(now, 0, 1), vec![2]);
        assert_eq!(q.due_limited(now, 0, 1), vec![3]);
    }

    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn deadline_timer_is_due_only_after_its_instant() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(1, TimerKind::Deadline(now + Duration::from_millis(50)));
        assert_eq!(q.due(now, 0), Vec::<u64>::new());              // not yet
        assert_eq!(q.len(), 1);
        assert_eq!(q.due(now + Duration::from_millis(60), 0), vec![1]); // now due, removed
        assert_eq!(q.len(), 0);
    }

    #[test]
    fn frame_timer_is_due_at_or_after_target_frame() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(7, TimerKind::Frame(5));
        assert_eq!(q.due(now, 4), Vec::<u64>::new()); // frame 4 < 5
        assert_eq!(q.due(now, 5), vec![7]);           // frame 5 reached
        assert!(q.is_empty());
    }

    #[test]
    fn multiple_due_timers_all_returned_and_removed() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(1, TimerKind::Deadline(now));            // already due
        q.push(2, TimerKind::Frame(1));                 // due at frame 1
        q.push(3, TimerKind::Deadline(now + Duration::from_secs(10))); // not due
        let mut due = q.due(now, 1);
        due.sort();
        assert_eq!(due, vec![1, 2]);
        assert_eq!(q.len(), 1); // only #3 remains
    }

    #[test]
    fn pool_runs_job_off_thread_and_reports_completion() {
        let pool = Pool::new(2);
        let ran = Arc::new(AtomicBool::new(false));
        let r2 = ran.clone();
        pool.submit(
            42,
            Box::new(move || {
                r2.store(true, Ordering::SeqCst);
                Ok(())
            }),
        );
        // Poll for completion (worker runs on another thread).
        let mut got = None;
        for _ in 0..1000 {
            if let Some(c) = pool.try_recv_completed() {
                got = Some(c);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (id, res, _lease) = got.expect("job never completed");
        assert_eq!(id, 42);
        assert!(res.is_ok());
        assert!(ran.load(Ordering::SeqCst));
    }

    #[test]
    fn try_recv_completed_is_nonblocking_when_empty() {
        let pool = Pool::new(1);
        assert!(pool.try_recv_completed().is_none()); // nothing submitted → immediate None
    }
}

static TIMER_EXAMINED:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
pub(crate) fn timer_examined() -> u64 {
    TIMER_EXAMINED.load(std::sync::atomic::Ordering::Relaxed)
}
