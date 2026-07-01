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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
