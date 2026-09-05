# Slice 8: Replace linear timer scheduling

**Status:** Planned; implementation has not started.
**Branch:** `core/hardening-08-timer-index`
**Parent / PR base:** `core/hardening-07-hook-index`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/async_rt.rs and timer integration/tests in core/src/v8host.rs.
**Boundary:** Preserve push/remove/due behavior or update all callers atomically. Ledger release
from slice 1 remains authoritative; scheduling indexes must not create a second lifetime owner.

- [ ] Add a deterministic oracle test comparing old and new schedules over interleaved
  insertion, time/frame advancement, cancellation, equal deadlines, and repeated cancellation.
- [ ] Use a deadline min-heap with stable sequence ordering and frame-target buckets. Add an
  ID index for cancellation; remove or compact cancelled heap entries so far-future cancellation
  cannot accumulate permanent tombstones.
- [ ] Preserve ordering when both frame and deadline timers become due in one drain. Test
  nextTick/nextFrame chains, repeating timers, self-kill, callback-created timers, and unload.
- [ ] Integrate due-work batching with slice 6 without dropping overdue timers or changing
  nextTick/nextFrame target calculation. Record any deliberate API-semantic change explicitly.
- [ ] Compare idle/due/cancel-heavy workloads at baseline sizes. Run core tests and live
  frame-order checks; publish results before claiming an improvement.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.
