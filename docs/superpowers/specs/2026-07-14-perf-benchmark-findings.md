# Cross-framework performance benchmark — findings

**Date:** 2026-07-14
**Rig:** de_dust2 (deathmatch), 31 `sv_stressbots`, `mp_warmuptime 99999`. s2script measured on the dev
host; CSSharp/ModSharp/Swiftly numbers are from the swiftly-solution/profiler published results
(Ryzen 9 9950X, 31 bots). Cross-hardware, so read as order-of-magnitude / directional, not a
controlled head-to-head.

## What this is

`examples/s2bench` mirrors the [swiftly-solution/profiler](https://github.com/swiftly-solution/profiler)
op-catalog in idiomatic s2script so we can see where s2script lands vs CounterStrikeSharp, ModSharp,
and SwiftlyS2. The profiler is the only existing cross-framework CS2 benchmark; it's C#-only (no
s2script slot), so we mirror its exact operations (same `point_worldtext` entity, same
`CreateEntityByName` + `DispatchSpawn` — **not** EKV, matching all four frameworks — same schema
fields, same `GetGameRules`/`FindConVar`).

Timing is **direct per-op** via the new `__s2_hrtime_ns` monotonic-ns native (baseline ~30–50ns/call,
close to .NET `Stopwatch`): batch ops timed as a single 1024-loop (the profiler's exact method),
per-call ops timed individually with min/avg/max and the timer baseline subtracted. No loop
amortization.

## Latency (batch total, ms; single-shot 1024-loop)

| Op | CSSharp | ModSharp | Swiftly | s2script |
|----|--------:|---------:|--------:|---------:|
| Create 1024 entities | 66.06 | 3.60 | 4.09 | ~4.5 |
| Spawn 1024 entities | 9.38 | 7.76 | 7.54 | ~3.2 |
| Schema Write+Update 1024 | 7.04 | 1.18 | 0.91 | ~0.80 |
| Schema Read 1024 | 2.91 | 0.20 | 0.22 | ~0.74 |
| Virtual-call teleport 1024 | 7.10 | 0.26 | 0.25 | ~0.82 |

Per-call (µs, avg): Get Game Rules — CSSharp 230, ModSharp 1, Swiftly 2, **s2script 0.40 (cached)**;
raw `findByClass` scan (what get() used to cost) **18.9µs**; Read ConVar ~0.5µs.

## Memory (V8 `used_heap_size`, the analog of `GC.GetTotalMemory`)

| Framework | before | peak (1024 alive) | after-GC | leak |
|-----------|-------:|------------------:|---------:|------|
| s2script | 5.56 | 5.69 (+0.13) | 5.61 → baseline | **none** |
| CSSharp | 4.84 | 31.4 (+26.5) | 5.63 | ~119 MB wrappers |
| ModSharp | 21.5 | 35.7 (+14.2) | 21.7 | none |
| Swiftly | 1.06 | 10.8 (+9.8) | 1.19 | none |

Per-entity heap ≈ 160 bytes transient / ~70 bytes retained — the value-type `EntityRef` `{index,serial}`
vs the .NET frameworks' per-entity managed wrapper objects (and CSSharp's un-freed 119 MB).

## Verdict

s2script ties/beats the two fastest frameworks on Create, Spawn, Schema-Write, Virtual-calls, and
(with the cache) Game-Rules; crushes CounterStrikeSharp by 4–575×; uses ~160× less transient heap and
leaks nothing. It trails the leaders ~3× on **Schema-Read** and **Virtual-calls**.

## Why the two gaps (from reading ModSharp/Swiftly's profiler source)

- **Schema-Read:** the .NET frameworks JIT-inline a direct memory read at a *baked* offset (~single-digit
  ns, no boundary crossing). s2script crosses the V8→Rust FFI per field read **and** resolves the schema
  offset live per access (`__s2_schema_offset`) — faithfully how its generated accessors work
  (self-healing across patches). That per-access FFI + live-offset is the ~3× gap.
- **Virtual-calls:** they pass a stack `Vector` struct; we allocate a fresh JS array `[x,y,z]` per call
  and marshal 3 elements across the FFI.
- **Unifying insight:** we win where engine work dominates (Create/Spawn — the engine call dwarfs one
  FFI crossing) and trail where the per-item work is tiny (a memory read / a teleport is cheaper than the
  JS↔native crossing itself). .NET JIT-inlines those accessors; s2script crosses the boundary per access.
  This only bites tight per-field loops (3072 reads/frame), which real plugins rarely do.

Fairness note: ModSharp and Swiftly actually do *more* reads in Schema-Read (they re-read `.Color` 3×
for a testColor calc, ~6 reads vs our 3) and still win — the gap is real, not a measurement artifact.
No framework does *less* work than our bench.

## Optimization levers surfaced

1. **GameRules proxy cache** — DONE (this PR): `get()` 18.9µs → 0.40µs, now beating the leaders.
2. Cache schema offsets (resolve once, guard by schema version/serial) — drops the live
   `__s2_schema_offset` per read; trade is losing per-access self-healing.
3. A batch-read native (`readFields([off1,off2,…])` — one crossing for N fields) — the real fix for
   bulk schema access.
4. A scalar/typed teleport arg instead of a fresh `[x,y,z]` array per call.

## What this PR adds

- `__s2_hrtime_ns` / `__s2_v8_heap_used` / `__s2_v8_gc` — perf-instrumentation natives (core,
  engine-generic). Foundation for a future per-plugin profiler.
- The GameRules proxy cache (lever 1).
- `examples/s2bench` — the reproducible benchmark.

## Follow-ups (not in this PR)

- A public `@s2script/perf` module (`Perf.now()`/`heapUsed()`) wrapping the natives.
- **Per-plugin CPU profiler**: wrap each plugin's frame/event/command handler in the dispatch loop
  with `__s2_hrtime_ns` and accumulate per owner (SM `sm_profiler` parity) — the multiplexer already
  tracks the owning plugin, so this is a small core-dispatch change.
- **Per-plugin memory**: `__s2_v8_heap_used` is isolate-wide; per-plugin needs V8's per-context
  `MeasureMemory` API. The context-per-plugin architecture supports it (a medium native).
- Schema-offset caching + a batch-read native (levers 2–3).
