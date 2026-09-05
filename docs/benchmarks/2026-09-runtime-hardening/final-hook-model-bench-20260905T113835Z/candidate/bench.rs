mod sdkhooks;

use sdkhooks::HookStore;
use std::time::Instant;

const TARGET_ENTITY: u64 = 0xABCD;
const TARGET_KIND: &str = "OnTakeDamage";
const HOOK_SCALES: &[usize] = &[1, 100, 1_000];
const ENTITY_SCALES: &[usize] = &[1, 100, 1_000];
const VIEWER_SCALES: &[usize] = &[1, 16, 64];

struct Stats { p50: u128, p95: u128, p99: u128, max: u128 }

fn stats(mut samples: Vec<u128>) -> Stats {
    samples.sort_unstable();
    let at = |p: usize| samples[(samples.len() * p / 100).min(samples.len() - 1)];
    Stats { p50: at(50), p95: at(95), p99: at(99), max: *samples.last().unwrap() }
}

fn print_stats(name: &str, dimensions: &str, samples: usize, result: Stats) {
    println!(
        "{name},{dimensions},samples={samples},p50_ns={},p95_ns={},p99_ns={},max_ns={}",
        result.p50, result.p95, result.p99, result.max
    );
}

fn snapshot_samples(total: usize) -> (usize, usize, Stats) {
    let store = HookStore::with_total(total, 1, TARGET_ENTITY);
    let (_, visited) = store.snapshot_kind_with_visit_count(TARGET_ENTITY, TARGET_KIND);
    let samples = 10_000;
    let mut sink = 0usize;
    for _ in 0..100 {
        sink ^= std::hint::black_box(store.snapshot_kind(TARGET_ENTITY, TARGET_KIND).len());
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let snapshot = store.snapshot_kind(TARGET_ENTITY, TARGET_KIND);
        sink ^= std::hint::black_box(snapshot.len());
        if let Some(first) = snapshot.first() {
            sink ^= std::hint::black_box(first.handler as usize);
            sink ^= std::hint::black_box(first.generation as usize);
            sink ^= std::hint::black_box(first.owner.len());
        }
        timings.push(started.elapsed().as_nanos());
    }
    (sink, visited, stats(timings))
}

fn entity_snapshot_samples(entities: usize) -> (usize, usize, Stats) {
    let settransmit = HookStore::with_distinct_entities(entities, TARGET_KIND);
    let (_, visited) = settransmit.snapshot_kind_entities_with_visit_count(TARGET_KIND);
    let samples = 10_000;
    let mut sink = 0usize;
    for (entity_id, first_sub_id) in settransmit.kind_membership_metadata(TARGET_KIND) {
        sink ^= std::hint::black_box((entity_id ^ first_sub_id) as usize);
    }
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let snapshot = settransmit.snapshot_kind_entities(TARGET_KIND);
        std::hint::black_box(snapshot.as_slice());
        sink ^= std::hint::black_box(snapshot.len());
        if let Some(first) = snapshot.first() {
            sink ^= std::hint::black_box(first.0 as usize);
            sink ^= std::hint::black_box(first.1 as usize);
        }
        if let Some(last) = snapshot.last() {
            sink ^= std::hint::black_box(last.0 as usize);
            sink ^= std::hint::black_box(last.1 as usize);
        }
        timings.push(started.elapsed().as_nanos());
    }
    (sink, visited, stats(timings))
}

fn modeled_frame_samples(entities: usize, viewers: usize) -> (usize, Stats) {
    let store = HookStore::with_distinct_entities(entities, TARGET_KIND);
    let samples = 2_000;
    let mut sink = 0usize;
    let mut timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let hooked = store.snapshot_kind_entities(TARGET_KIND);
        let started = Instant::now();
        for &(entity, _serial) in &hooked {
            for _viewer in 0..viewers {
                sink ^= std::hint::black_box(store.snapshot_kind(entity as u64, TARGET_KIND).len());
            }
        }
        timings.push(started.elapsed().as_nanos());
    }
    (sink, stats(timings))
}

fn main() {
    println!("# candidate SDK hook model: indexed entity/kind buckets and maintained kind membership");
    println!("# excludes V8, JS callbacks, SourceHook, shim, engine, and process startup");
    let mut sink = 0usize;
    for &total in HOOK_SCALES {
        let (value, visited, result) = snapshot_samples(total);
        sink ^= value;
        print_stats(
            "sdkhook_snapshot",
            &format!(
                "total_hooks={total},matching_hooks=1,observed_visited_bucket_entries={visited}"
            ),
            10_000,
            result,
        );
    }
    for &entities in ENTITY_SCALES {
        let (value, visited, result) = entity_snapshot_samples(entities);
        sink ^= value;
        print_stats(
            "sdkhook_entity_snapshot",
            &format!(
                "hooked_entities={entities},observed_visited_kind_entities={visited}"
            ),
            10_000,
            result,
        );
    }
    for &entities in ENTITY_SCALES {
        for &viewers in VIEWER_SCALES {
            let (value, result) = modeled_frame_samples(entities, viewers);
            sink ^= value;
            print_stats(
                "sdkhook_modeled_frame_lookup",
                &format!("hooked_entities={entities},viewers={viewers},js_callbacks=0"),
                2_000,
                result,
            );
        }
    }
    println!("# sink={sink}");
}
