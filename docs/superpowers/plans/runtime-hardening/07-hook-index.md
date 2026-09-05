# Slice 7: Index entity hooks

**Status:** Planned; implementation has not started.
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
