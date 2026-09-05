#![allow(dead_code)]

mod async_rt;
mod sdkhooks;

use std::time::{Duration, Instant};

const HOOK_SCALES: &[usize] = &[1, 100, 1_000];
const TIMER_SIZES: &[usize] = &[0, 10, 1_000, 10_000];
const TARGET_ENTITY: u64 = 0xfeed_beef;
const TARGET_KIND: &str = "OnTakeDamage";

#[derive(Debug)]
struct Stats {
    p50: u128,
    p95: u128,
    p99: u128,
    max: u128,
}

fn stats(mut samples: Vec<u128>) -> Stats {
    assert!(!samples.is_empty());
    samples.sort_unstable();
    let at = |percent: usize| samples[(samples.len() * percent / 100).min(samples.len() - 1)];
    Stats { p50: at(50), p95: at(95), p99: at(99), max: *samples.last().unwrap() }
}

fn print_stats(workload: &str, size: usize, samples: usize, s: Stats) {
    println!(
        "{workload},n={size},samples={samples},p50_ns={},p95_ns={},p99_ns={},max_ns={}",
        s.p50, s.p95, s.p99, s.max
    );
}

fn hook_samples(total: usize, matching: usize) -> (usize, Stats, usize) {
    let store = sdkhooks::HookStore::with_total(total, matching, TARGET_ENTITY);
    let sample_count = 10_000;
    let mut sink = 0usize;
    // Warm the instruction/data paths without including cold-start work in the samples.
    for _ in 0..100 {
        sink ^= std::hint::black_box(store.snapshot_kind(TARGET_ENTITY, TARGET_KIND).len());
    }
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let started = Instant::now();
        let snapshot = store.snapshot_kind(TARGET_ENTITY, TARGET_KIND);
        sink ^= std::hint::black_box(snapshot.len());
        // Keep the detached fields observable as well as the result length. This prevents a
        // future compiler from treating the owned snapshot contents as dead benchmark work.
        if let Some(first) = snapshot.first() {
            sink ^= std::hint::black_box(first.handler as usize);
            sink ^= std::hint::black_box(first.generation as usize);
            sink ^= std::hint::black_box(first.owner.len());
        }
        samples.push(started.elapsed().as_nanos());
    }
    (sink, stats(samples), sample_count)
}

fn queue_with_deadlines(n: usize, at: Instant) -> async_rt::TimerQueue {
    let mut queue = async_rt::TimerQueue::new();
    for id in 0..n as u64 {
        queue.push(id, async_rt::TimerKind::Deadline(at));
    }
    queue
}

fn idle_samples(n: usize) -> Stats {
    let now = Instant::now();
    let mut queue = async_rt::TimerQueue::new();
    for id in 0..n as u64 {
        queue.push(id, async_rt::TimerKind::Deadline(now + Duration::from_secs(3_600)));
    }
    let sample_count = 2_000;
    for _ in 0..20 {
        std::hint::black_box(queue.due(now, 0));
    }
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let started = Instant::now();
        std::hint::black_box(queue.due(now, 0));
        samples.push(started.elapsed().as_nanos());
    }
    stats(samples)
}

fn due_samples(n: usize) -> (usize, Stats) {
    // Rebuild outside the timed region: this models a populated scheduler and isolates drain
    // work from registration/setup, as the frame loop does in production.
    let sample_count = match n {
        0 => 2_000,
        10 => 2_000,
        1_000 => 500,
        10_000 => 200,
        _ => 100,
    };
    let now = Instant::now();
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let mut queue = queue_with_deadlines(n, now);
        let started = Instant::now();
        let due = queue.due(now, 0);
        std::hint::black_box((due, queue.len()));
        samples.push(started.elapsed().as_nanos());
    }
    (sample_count, stats(samples))
}

fn cancel_samples(n: usize) -> (usize, Stats) {
    // Removing one-by-one is intentionally the baseline's expensive cancellation path:
    // TimerQueue::remove uses Vec::retain, so each kill scans the remaining queue.
    // Adaptive repetition keeps the 10,000-entry O(n²) case bounded while retaining repeated
    // samples for the smaller cases.
    let sample_count = match n {
        0 => 2_000,
        10 => 1_000,
        1_000 => 100,
        10_000 => 8,
        _ => 100,
    };
    let now = Instant::now();
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let mut queue = async_rt::TimerQueue::new();
        for id in 0..n as u64 {
            queue.push(id, async_rt::TimerKind::Deadline(now + Duration::from_secs(3_600)));
        }
        let started = Instant::now();
        for id in 0..n as u64 {
            std::hint::black_box(queue.remove(id));
        }
        std::hint::black_box(queue.is_empty());
        samples.push(started.elapsed().as_nanos());
    }
    (sample_count, stats(samples))
}

fn main() {
    println!("# sdk hook baseline: ordered Vec<Entry>, one addressed match at end");
    println!("# timer baseline: TimerQueue Vec<(u64, TimerKind)> with retain-based due/remove");
    let mut sink = 0usize;

    for &total in HOOK_SCALES {
        let (value, result, samples) = hook_samples(total, 1);
        sink ^= value;
        println!(
            "sdkhook_snapshot,total_hooks={total},matching_hooks=1,samples={samples},p50_ns={},p95_ns={},p99_ns={},max_ns={}",
            result.p50, result.p95, result.p99, result.max
        );
    }

    for &n in TIMER_SIZES {
        print_stats("timer_idle", n, 2_000, idle_samples(n));
        let (samples, result) = due_samples(n);
        print_stats("timer_due", n, samples, result);
        let (samples, result) = cancel_samples(n);
        print_stats("timer_cancelheavy", n, samples, result);
    }

    println!("# sink={sink}");
}
