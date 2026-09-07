# Slice 7: Index entity hooks

**Status:** Implemented and independently reviewed; automated correctness and native model scaling checks pass. Actual human-viewer SetTransmit validation remains manual.
**Branch:** `core/hardening-07-hook-index`
**Parent / PR base:** `core/hardening-06-async-budgets`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/sdkhooks.rs, core/src/sdkhooks_transmit.rs and corresponding tests;
tools/s2bench/src/plugin.ts. Keep shim/src/s2script_mm.cpp's visibility merge semantics.

**Boundary:** Dispatch addresses an entity host identity and hook kind. Per-kind active counts
and entity membership are updated with the subscription store, not rebuilt per dispatch.

- [ ] Pin registration order, result folding, disabled handlers, owner unload, entity removal,
  slot/serial reuse, and subscribe/unsubscribe during a callback against the existing behavior.
- [ ] Replace full HOOKS scans with keyed ordered subscriber collections and reverse indexes
  needed for owner/entity cleanup. Keep snapshots detached before entering JS.
- [ ] Maintain per-kind entity sets/counts so SetTransmit can enumerate just its hooked entities.
  Preserve hide-only AND merging with native visibility rules.
- [ ] Benchmark 1/100/1,000 total hooks while keeping addressed subscribers constant; vary
  viewers and hooked entities separately. Report lookup work and frame cost separately from JS.
- [ ] Run core and live damage/SetTransmit tests. Accept only behavioral parity with improved
  scaling and no material small-workload regression across repeated comparable runs.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.

## Reviewed implementation and evidence

Entity/kind buckets, ordered subscriptions, reverse owner indexes, and active counts replace
unrelated-hook scans. Dispatch still uses detached snapshots, so callback-time subscription
changes retain the previous semantics. Entity enumeration uses the first remaining subscriber's
order; counters measure visited entries rather than merely counting returned results.

Independent review closed entity enumeration ordering and a tautological work-counter issue.
A real stale-generation native witness fails when its owner guard is bypassed. Registration
order, unload/removal, callback mutation, and stale identity behavior remain covered.

The native **model** uses the same addressed-subscriber workload at 1, 100, and 1,000 total
hooks. Recorded median lookup times were approximately 41/42/42 ns versus the baseline's
42/83/375 ns. These are model data-structure timings, excluding V8, JS, SourceHook, the shim,
and the engine; they are not end-to-end performance claims. Entity and viewer scaling
workloads are recorded separately, and final same-workload reruns remain stack-wide.

`tools/s2bench` exposes controlled hook-load, stats, and reset commands. The isolated live
pilot registered a hook but recorded zero snapshots/callbacks with bots: headless bots do
not provide real CheckTransmit viewer traffic. No live SetTransmit throughput or parity
claim is made. A human viewer is still required for that manual gate. The benchmark fixture
was reset and removed from the owned test server after the pilot.

The combined reviewed hook/timer/async parent passed the full Linux native gate at
`5e40276651ac26afa569aa35ec931e58424182f1`: 736 core tests, two separate pressure cases,
shim and symbol checks. Final integrated engine/soak validation remains pending.
