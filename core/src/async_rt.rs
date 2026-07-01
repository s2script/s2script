//! Engine-generic, V8-free async runtime primitives: a fixed-size threadpool and a timer queue.
//! Holds NO V8 handles — jobs/timers carry a `u64` id that `v8host` maps to a PromiseResolver.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

pub type JobResult = Result<(), String>;
pub type Job = Box<dyn FnOnce() -> JobResult + Send + 'static>;

pub struct Pool {
    job_tx: Sender<(u64, Job)>,
    completion_rx: Receiver<(u64, JobResult)>,
    _workers: Vec<JoinHandle<()>>,
}

impl Pool {
    pub fn new(workers: usize) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<(u64, Job)>();
        let (done_tx, completion_rx) = mpsc::channel::<(u64, JobResult)>();
        let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
        let mut handles = Vec::new();
        for _ in 0..workers.max(1) {
            let job_rx = job_rx.clone();
            let done_tx = done_tx.clone();
            handles.push(std::thread::spawn(move || loop {
                // Lock only to dequeue; release before running the (possibly long) job.
                let next = { job_rx.lock().unwrap().recv() };
                match next {
                    Ok((id, job)) => { let res = job(); let _ = done_tx.send((id, res)); }
                    Err(_) => break, // all senders dropped → pool shutting down
                }
            }));
        }
        Pool { job_tx, completion_rx, _workers: handles }
    }

    pub fn submit(&self, id: u64, job: Job) {
        let _ = self.job_tx.send((id, job));
    }

    pub fn try_recv_completed(&self) -> Option<(u64, JobResult)> {
        self.completion_rx.try_recv().ok()
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

    pub fn due(&mut self, now: std::time::Instant, frame: u64) -> Vec<u64> {
        let mut ready = Vec::new();
        self.entries.retain(|(id, kind)| {
            let is_due = match kind {
                TimerKind::Deadline(t) => now >= *t,
                TimerKind::Frame(target) => frame >= *target,
            };
            if is_due { ready.push(*id); false } else { true }
        });
        ready
    }
}

impl Default for TimerQueue { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod tests {
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
        pool.submit(42, Box::new(move || { r2.store(true, Ordering::SeqCst); Ok(()) }));
        // Poll for completion (worker runs on another thread).
        let mut got = None;
        for _ in 0..1000 {
            if let Some(c) = pool.try_recv_completed() { got = Some(c); break; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let (id, res) = got.expect("job never completed");
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
