use std::collections::{BTreeMap,BTreeSet,HashMap};
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

        let (deadlines, deadline_examined) = self.take_due_deadlines(now);
        eligible.extend(
            deadlines
                .into_iter()
                .map(|entry| (entry.sequence, entry.id)),
        );
        let (frames, frame_examined) = self.take_due_frames(frame);
        eligible.extend(frames);
        record_timer_examined(deadline_examined + frame_examined);
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

        let (deadlines, deadline_examined) = self.take_due_deadlines(now);
        for entry in deadlines {
            self.ready.insert(entry.sequence, entry.id);
            self.locations.insert(entry.sequence, EntryLocation::Ready);
        }

        let (frames, frame_examined) = self.take_due_frames(frame);
        for (sequence, id) in frames {
            self.ready.insert(sequence, id);
            self.locations.insert(sequence, EntryLocation::Ready);
        }
        record_timer_examined(deadline_examined + frame_examined);

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

    fn take_due_deadlines(&mut self, now: std::time::Instant) -> (Vec<DeadlineEntry>, u64) {
        if self
            .deadline_targets
            .last_key_value()
            .is_some_and(|(&target, _)| target <= now)
        {
            self.deadline_targets.clear();
            let examined = self.deadline_heap.len() as u64;
            return (std::mem::take(&mut self.deadline_heap), examined);
        }

        let mut due = Vec::new();
        let mut examined = 0;
        while let Some(entry) = self.deadline_heap.first() {
            examined += 1;
            if entry.target > now {
                break;
            }
            due.push(self.remove_deadline_at(0));
        }
        (due, examined)
    }

    fn take_due_frames(&mut self, frame: u64) -> (Vec<(u64, u64)>, u64) {
        let mut due = Vec::new();
        let mut examined = 0;
        while self
            .frame_buckets
            .first_key_value()
            .is_some_and(|(&target, _)| target <= frame)
        {
            let (_, bucket) = self
                .frame_buckets
                .pop_first()
                .expect("frame bucket disappeared during due selection");
            examined += bucket.len() as u64;
            for (sequence, id) in bucket {
                due.push((sequence, id));
            }
        }
        (due, examined)
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


static TIMER_EXAMINED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn record_timer_examined(examined: u64) {
    TIMER_EXAMINED.fetch_add(examined, std::sync::atomic::Ordering::Relaxed);
}
pub(crate) fn timer_examined() -> u64 {
    TIMER_EXAMINED.load(std::sync::atomic::Ordering::Relaxed)
}
