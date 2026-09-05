//! Engine-generic, V8-free async runtime primitives: a fixed-size threadpool and a timer queue.
//! Holds NO V8 handles — jobs/timers carry a `u64` id that `v8host` maps to a PromiseResolver.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread::JoinHandle;

pub type JobResult = Result<(), String>;
pub type Job = Box<dyn FnOnce() -> JobResult + Send + 'static>;

pub struct Pool {
    job_tx: SyncSender<(u64, Job, crate::async_limits::JobLease)>,
    completion_rx: std::sync::Mutex<
        Receiver<(
            u64,
            JobResult,
            crate::async_limits::JobLease,
            crate::async_limits::QueueTicket,
        )>,
    >,
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

#[derive(Clone, Copy, Debug)]
struct DeadlineEntry {
    id: u64,
    sequence: u64,
    target: std::time::Instant,
}

#[derive(Debug)]
enum IdSequences {
    One(u64),
    Many(BTreeSet<u64>),
}

#[derive(Clone, Copy, Debug)]
enum EntryLocation {
    Deadline(usize),
    Frame(u64),
    Ready,
}

pub struct TimerQueue {
    /// Indexed min-heap ordered by `(deadline, insertion sequence)`.
    deadline_heap: Vec<DeadlineEntry>,
    /// Deadline multiplicities make the all-due bulk path observable without scanning the heap.
    deadline_targets: BTreeMap<std::time::Instant, usize>,
    /// Frame target -> insertion sequence -> timer id.
    frame_buckets: BTreeMap<u64, BTreeMap<u64, u64>>,
    /// Eligible entries await bounded drains here in global insertion order.
    ready: BTreeMap<u64, u64>,
    /// Timer id -> every sequence carrying that id. Duplicate ids retain the old queue contract.
    id_sequences: HashMap<u64, IdSequences>,
    /// Sequence -> physical storage location, used for indexed cancellation.
    locations: HashMap<u64, EntryLocation>,
    next_sequence: u64,
}

impl TimerQueue {
    pub fn new() -> Self {
        TimerQueue {
            deadline_heap: Vec::new(),
            deadline_targets: BTreeMap::new(),
            frame_buckets: BTreeMap::new(),
            ready: BTreeMap::new(),
            id_sequences: HashMap::new(),
            locations: HashMap::new(),
            next_sequence: 0,
        }
    }

    pub fn push(&mut self, id: u64, kind: TimerKind) {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("timer insertion sequence exhausted");
        match self.id_sequences.entry(id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(IdSequences::One(sequence));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => match entry.get_mut() {
                IdSequences::One(first) => {
                    let mut sequences = BTreeSet::new();
                    sequences.insert(*first);
                    sequences.insert(sequence);
                    *entry.get_mut() = IdSequences::Many(sequences);
                }
                IdSequences::Many(sequences) => {
                    sequences.insert(sequence);
                }
            },
        }

        match kind {
            TimerKind::Deadline(target) => {
                let index = self.deadline_heap.len();
                self.deadline_heap.push(DeadlineEntry {
                    id,
                    sequence,
                    target,
                });
                *self.deadline_targets.entry(target).or_default() += 1;
                self.locations
                    .insert(sequence, EntryLocation::Deadline(index));
                self.sift_deadline_up(index);
            }
            TimerKind::Frame(target) => {
                self.frame_buckets
                    .entry(target)
                    .or_default()
                    .insert(sequence, id);
                self.locations
                    .insert(sequence, EntryLocation::Frame(target));
            }
        }
    }

    pub fn len(&self) -> usize {
        self.locations.len()
    }
    pub fn is_empty(&self) -> bool {
        self.locations.is_empty()
    }

    /// Remove a pending timer by id (used by the ledger teardown when its owning plugin unloads,
    /// so a dropped continuation's timer stops keeping the frame detour alive).  Returns true if a
    /// timer with that id was present.
    pub fn remove(&mut self, id: u64) -> bool {
        let Some(sequences) = self.id_sequences.remove(&id) else {
            return false;
        };
        match sequences {
            IdSequences::One(sequence) => {
                let location = *self
                    .locations
                    .get(&sequence)
                    .expect("timer id index and location index diverged");
                self.remove_from_location(sequence, location);
            }
            IdSequences::Many(sequences) => {
                for sequence in sequences {
                    let location = *self
                        .locations
                        .get(&sequence)
                        .expect("timer id index and location index diverged");
                    self.remove_from_location(sequence, location);
                }
            }
        }
        true
    }

    pub fn due(&mut self, now: std::time::Instant, frame: u64) -> Vec<u64> {
        let pending_before = self.locations.len();
        let mut eligible: Vec<(u64, u64)> = self
            .ready
            .iter()
            .map(|(&sequence, &id)| (sequence, id))
            .collect();
        self.ready.clear();

        eligible.extend(
            self.take_due_deadlines(now)
                .into_iter()
                .map(|entry| (entry.sequence, entry.id)),
        );
        eligible.extend(self.take_due_frames(frame));
        if !eligible.is_sorted_by_key(|&(sequence, _)| sequence) {
            eligible.sort_unstable_by_key(|&(sequence, _)| sequence);
        }

        let drains_everything = eligible.len() == pending_before;
        let mut due = Vec::with_capacity(eligible.len());
        for (sequence, id) in eligible {
            if !drains_everything {
                self.locations.remove(&sequence);
                self.unlink_id_sequence(id, sequence);
            }
            due.push(id);
        }
        if drains_everything {
            self.locations.clear();
            self.id_sequences.clear();
        }
        due
    }

    /// Remove at most `limit` eligible timers, preserving insertion order across deadline and
    /// frame timers. Excess eligible work remains queued for a later drain; a zero limit performs
    /// no selection work. Deadline discovery takes an all-due heap at once or performs one indexed
    /// heap removal per eligible deadline; frame discovery extracts only eligible target buckets.
    /// The returned-item limit does not bound that selection work.
    pub fn due_limited(&mut self, now: std::time::Instant, frame: u64, limit: usize) -> Vec<u64> {
        if limit == 0 {
            return Vec::new();
        }

        for entry in self.take_due_deadlines(now) {
            self.ready.insert(entry.sequence, entry.id);
            self.locations.insert(entry.sequence, EntryLocation::Ready);
        }

        for (sequence, id) in self.take_due_frames(frame) {
            self.ready.insert(sequence, id);
            self.locations.insert(sequence, EntryLocation::Ready);
        }

        let mut due = Vec::with_capacity(limit.min(self.ready.len()));
        while due.len() < limit {
            let Some((sequence, id)) = self.ready.pop_first() else {
                break;
            };
            self.locations.remove(&sequence);
            self.unlink_id_sequence(id, sequence);
            due.push(id);
        }
        due
    }

    fn remove_from_location(&mut self, sequence: u64, location: EntryLocation) {
        match location {
            EntryLocation::Deadline(index) => {
                let removed = self.remove_deadline_at(index);
                debug_assert_eq!(removed.sequence, sequence);
            }
            EntryLocation::Frame(target) => {
                let bucket = self
                    .frame_buckets
                    .get_mut(&target)
                    .expect("timer location referenced a missing frame bucket");
                bucket.remove(&sequence);
                if bucket.is_empty() {
                    self.frame_buckets.remove(&target);
                }
                self.locations.remove(&sequence);
            }
            EntryLocation::Ready => {
                self.ready.remove(&sequence);
                self.locations.remove(&sequence);
            }
        }
    }

    fn unlink_id_sequence(&mut self, id: u64, sequence: u64) {
        let remove_id = if let Some(sequences) = self.id_sequences.get_mut(&id) {
            match sequences {
                IdSequences::One(only) => {
                    debug_assert_eq!(*only, sequence);
                    true
                }
                IdSequences::Many(sequences) => {
                    sequences.remove(&sequence);
                    sequences.is_empty()
                }
            }
        } else {
            false
        };
        if remove_id {
            self.id_sequences.remove(&id);
        }
    }

    fn take_due_deadlines(&mut self, now: std::time::Instant) -> Vec<DeadlineEntry> {
        if self
            .deadline_targets
            .last_key_value()
            .is_some_and(|(&target, _)| target <= now)
        {
            self.deadline_targets.clear();
            return std::mem::take(&mut self.deadline_heap);
        }

        let mut due = Vec::new();
        while self
            .deadline_heap
            .first()
            .is_some_and(|entry| entry.target <= now)
        {
            due.push(self.remove_deadline_at(0));
        }
        due
    }

    fn take_due_frames(&mut self, frame: u64) -> Vec<(u64, u64)> {
        let mut due = Vec::new();
        while self
            .frame_buckets
            .first_key_value()
            .is_some_and(|(&target, _)| target <= frame)
        {
            let (_, bucket) = self
                .frame_buckets
                .pop_first()
                .expect("frame bucket disappeared during due selection");
            for (sequence, id) in bucket {
                due.push((sequence, id));
            }
        }
        due
    }

    fn deadline_precedes(a: &DeadlineEntry, b: &DeadlineEntry) -> bool {
        (a.target, a.sequence) < (b.target, b.sequence)
    }

    fn swap_deadlines(&mut self, a: usize, b: usize) {
        self.deadline_heap.swap(a, b);
        let a_sequence = self.deadline_heap[a].sequence;
        let b_sequence = self.deadline_heap[b].sequence;
        self.locations
            .insert(a_sequence, EntryLocation::Deadline(a));
        self.locations
            .insert(b_sequence, EntryLocation::Deadline(b));
    }

    fn sift_deadline_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if !Self::deadline_precedes(&self.deadline_heap[index], &self.deadline_heap[parent]) {
                break;
            }
            self.swap_deadlines(index, parent);
            index = parent;
        }
    }

    fn sift_deadline_down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.deadline_heap.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.deadline_heap.len()
                && Self::deadline_precedes(&self.deadline_heap[right], &self.deadline_heap[left])
            {
                right
            } else {
                left
            };
            if !Self::deadline_precedes(&self.deadline_heap[child], &self.deadline_heap[index]) {
                break;
            }
            self.swap_deadlines(index, child);
            index = child;
        }
    }

    fn remove_deadline_at(&mut self, index: usize) -> DeadlineEntry {
        let last = self.deadline_heap.len() - 1;
        if index != last {
            self.deadline_heap.swap(index, last);
            let moved_sequence = self.deadline_heap[index].sequence;
            self.locations
                .insert(moved_sequence, EntryLocation::Deadline(index));
        }
        let removed = self
            .deadline_heap
            .pop()
            .expect("deadline heap unexpectedly empty");
        self.locations.remove(&removed.sequence);
        let remove_target = {
            let count = self
                .deadline_targets
                .get_mut(&removed.target)
                .expect("deadline heap and target index diverged");
            *count -= 1;
            *count == 0
        };
        if remove_target {
            self.deadline_targets.remove(&removed.target);
        }

        if index < self.deadline_heap.len() {
            let parent = index.checked_sub(1).map(|value| value / 2);
            if parent.is_some_and(|parent| {
                Self::deadline_precedes(&self.deadline_heap[index], &self.deadline_heap[parent])
            }) {
                self.sift_deadline_up(index);
            } else {
                self.sift_deadline_down(index);
            }
        }
        removed
    }
}

impl Default for TimerQueue {
    fn default() -> Self {
        Self::new()
    }
}

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

    /// The pre-index queue, retained only as an independent behavioral oracle for this test.
    struct LinearTimerQueue {
        entries: Vec<(u64, TimerKind)>,
    }

    impl LinearTimerQueue {
        fn new() -> Self {
            Self {
                entries: Vec::new(),
            }
        }

        fn push(&mut self, id: u64, kind: TimerKind) {
            self.entries.push((id, kind));
        }

        fn remove(&mut self, id: u64) -> bool {
            let before = self.entries.len();
            self.entries.retain(|(entry_id, _)| *entry_id != id);
            self.entries.len() != before
        }

        fn due(&mut self, now: Instant, frame: u64) -> Vec<u64> {
            let mut ready = Vec::new();
            self.entries.retain(|(id, kind)| {
                let is_due = match kind {
                    TimerKind::Deadline(target) => now >= *target,
                    TimerKind::Frame(target) => frame >= *target,
                };
                if is_due {
                    ready.push(*id);
                }
                !is_due
            });
            ready
        }

        fn len(&self) -> usize {
            self.entries.len()
        }
    }

    #[test]
    fn deadline_timer_is_due_only_after_its_instant() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(1, TimerKind::Deadline(now + Duration::from_millis(50)));
        assert_eq!(q.due(now, 0), Vec::<u64>::new()); // not yet
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
        assert_eq!(q.due(now, 5), vec![7]); // frame 5 reached
        assert!(q.is_empty());
    }

    #[test]
    fn multiple_due_timers_all_returned_and_removed() {
        let mut q = TimerQueue::new();
        let now = Instant::now();
        q.push(1, TimerKind::Deadline(now)); // already due
        q.push(2, TimerKind::Frame(1)); // due at frame 1
        q.push(3, TimerKind::Deadline(now + Duration::from_secs(10))); // not due
        let mut due = q.due(now, 1);
        due.sort();
        assert_eq!(due, vec![1, 2]);
        assert_eq!(q.len(), 1); // only #3 remains
    }

    #[test]
    fn indexed_queue_matches_linear_oracle_for_interleaved_operations() {
        let base = Instant::now();
        let mut old = LinearTimerQueue::new();
        let mut new = TimerQueue::new();
        let initial = [
            (1, TimerKind::Deadline(base + Duration::from_millis(30))),
            (2, TimerKind::Frame(3)),
            (3, TimerKind::Deadline(base + Duration::from_millis(10))),
            (4, TimerKind::Frame(1)),
            (5, TimerKind::Deadline(base + Duration::from_millis(10))),
            (6, TimerKind::Deadline(base + Duration::from_millis(60))),
        ];
        for (id, kind) in initial {
            old.push(id, kind);
            new.push(id, kind);
        }

        assert_eq!(new.remove(3), old.remove(3));
        assert_eq!(
            new.remove(3),
            old.remove(3),
            "repeated cancellation stays idempotent"
        );
        assert_eq!(new.due(base, 0), old.due(base, 0));

        // Model callback-created and re-armed work by inserting more timers between drains.
        for (id, kind) in [
            (7, TimerKind::Frame(0)),
            (8, TimerKind::Deadline(base)),
            (9, TimerKind::Frame(0)),
        ] {
            old.push(id, kind);
            new.push(id, kind);
        }
        assert_eq!(new.due(base, 0), old.due(base, 0));
        // Re-arm the fired callback timer with the same public id, as `Timers.every` does.
        old.push(7, TimerKind::Deadline(base + Duration::from_millis(20)));
        new.push(7, TimerKind::Deadline(base + Duration::from_millis(20)));
        assert_eq!(
            new.due(base + Duration::from_millis(10), 1),
            old.due(base + Duration::from_millis(10), 1)
        );
        assert_eq!(new.remove(6), old.remove(6));
        assert_eq!(
            new.due(base + Duration::from_secs(1), 4),
            old.due(base + Duration::from_secs(1), 4)
        );
        assert_eq!(new.len(), old.len());
    }

    #[test]
    fn indexed_queue_matches_linear_oracle_under_deterministic_churn() {
        let base = Instant::now();
        let mut now_ms = 0_u64;
        let mut frame = 0_u64;
        let mut random = 0x6a09_e667_f3bc_c909_u64;
        let mut old = LinearTimerQueue::new();
        let mut new = TimerQueue::new();

        for step in 0..2_000 {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let id = (random >> 17) % 64;
            match random % 7 {
                0..=2 => {
                    let kind = if random & 0x100 == 0 {
                        TimerKind::Deadline(
                            base + Duration::from_millis(now_ms + ((random >> 25) % 20)),
                        )
                    } else {
                        TimerKind::Frame(frame + ((random >> 25) % 8))
                    };
                    old.push(id, kind);
                    new.push(id, kind);
                }
                3 => assert_eq!(
                    new.remove(id),
                    old.remove(id),
                    "cancellation diverged at operation {step}"
                ),
                4 => now_ms += (random >> 29) % 5,
                5 => frame += (random >> 29) % 3,
                6 => assert_eq!(
                    new.due(base + Duration::from_millis(now_ms), frame),
                    old.due(base + Duration::from_millis(now_ms), frame),
                    "drain order diverged at operation {step}"
                ),
                _ => unreachable!(),
            }
            assert_eq!(
                new.len(),
                old.len(),
                "queue length diverged at operation {step}"
            );
        }

        assert_eq!(
            new.due(base + Duration::from_secs(1_000), u64::MAX),
            old.due(base + Duration::from_secs(1_000), u64::MAX)
        );
        assert!(new.is_empty());
    }

    #[test]
    fn sparse_due_batches_across_the_old_bulk_threshold_retain_valid_indexes() {
        let now = Instant::now();
        const FUTURE_COUNT: u64 = 2_048;

        for due_count in [64_u64, 65] {
            let mut q = TimerQueue::new();
            for id in 0..due_count {
                q.push(id, TimerKind::Deadline(now));
            }
            for offset in 0..FUTURE_COUNT {
                q.push(
                    10_000 + offset,
                    TimerKind::Deadline(now + Duration::from_secs(3_600)),
                );
            }

            assert_eq!(q.due_limited(now, 0, 1), vec![0]);
            assert!(
                q.remove(due_count - 1),
                "a retained due timer remains cancellable"
            );
            assert!(
                q.remove(10_000 + FUTURE_COUNT - 1),
                "a retained future timer remains cancellable"
            );

            let expected_due: Vec<u64> = (1..due_count - 1).collect();
            assert_eq!(q.due(now, 0), expected_due);
            assert_eq!(q.len(), (FUTURE_COUNT - 1) as usize);
            assert!(q.due(now, 0).is_empty());

            let expected_future: Vec<u64> = (10_000..10_000 + FUTURE_COUNT - 1).collect();
            assert_eq!(q.due(now + Duration::from_secs(3_600), 0), expected_future);
            assert!(q.is_empty());
        }
    }

    #[test]
    fn due_limited_preserves_overdue_work_and_global_insertion_order() {
        let now = Instant::now();
        let mut q = TimerQueue::new();
        q.push(10, TimerKind::Deadline(now));
        q.push(11, TimerKind::Frame(0));
        q.push(12, TimerKind::Deadline(now));
        q.push(13, TimerKind::Frame(0));

        assert_eq!(q.due_limited(now, 0, 0), Vec::<u64>::new());
        assert_eq!(
            q.len(),
            4,
            "a zero-item batch must leave every eligible timer queued"
        );
        assert_eq!(q.due_limited(now, 0, 2), vec![10, 11]);
        assert_eq!(
            q.len(),
            2,
            "excess overdue timers remain pending for a later frame"
        );
        assert_eq!(q.due_limited(now, 0, 1), vec![12]);
        assert_eq!(q.due(now, 0), vec![13]);
    }

    #[test]
    fn cancelling_duplicate_ids_physically_removes_every_indexed_entry() {
        let now = Instant::now();
        let mut q = TimerQueue::new();
        q.push(7, TimerKind::Deadline(now + Duration::from_secs(3_600)));
        q.push(7, TimerKind::Frame(u64::MAX));
        q.push(8, TimerKind::Deadline(now));
        assert!(q.remove(7));
        assert!(!q.remove(7));

        assert_eq!(q.len(), 1);
        assert_eq!(
            q.deadline_heap.len(),
            1,
            "cancelled deadline must not remain as a tombstone"
        );
        assert_eq!(
            q.deadline_targets.len(),
            1,
            "cancelled deadline must leave the target index"
        );
        assert!(
            q.frame_buckets.is_empty(),
            "cancelled frame bucket must be removed"
        );
        assert_eq!(
            q.id_sequences.len(),
            1,
            "cancelled ids must leave the cancellation index"
        );
        assert_eq!(
            q.locations.len(),
            1,
            "cancelled entries must leave the location index"
        );
        assert_eq!(q.due(now, 0), vec![8]);
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

static TIMER_EXAMINED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) fn timer_examined() -> u64 {
    TIMER_EXAMINED.load(std::sync::atomic::Ordering::Relaxed)
}
