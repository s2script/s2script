# Slice 8: Replace linear timer scheduling

**Status:** Implemented and independently reviewed; local/Linux correctness gates and repeated native benchmarks pass. Final integrated live gate remains pending.
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

## Reviewed implementation and evidence

An indexed deadline heap plus frame buckets and reverse locations replaces full timer scans.
Cancellation physically removes entries; there is no historical cancellation tombstone list.
Duplicate-ID behavior and global due ordering are preserved, and `due_limited(0)` retains
all work. The frame budget uses actual eligible entries examined, not returned vector length.

Review found and closed a full-heap rebuild when 65 timers were due beside 100,000 future
timers (approximately 1.444 ms before the fix versus 45 microseconds after). An independent
100,000-operation schedule oracle, 13 timer tests and 11 V8 integration tests passed.

The final combined code at `5e40276651ac26afa569aa35ec931e58424182f1` passed 736 core tests,
both fresh-process pressure cases, and the full Linux native/shim/symbol gate. The async
and timer integration received a separate independent review.

Five paired native benchmark runs retain the original workload sizes and timing boundaries.
Large idle queues and heavy cancellation improve, but costs are explicit: 10,000 due timers
measured approximately 9 to 27 microseconds (slower), and cancellation at size ten increased
from approximately 42 to 541 ns. Cancellation at 10,000 improved from roughly 20.6 to 2.1 ms.
Sub-resolution single-call idle measurements reported as zero do **not** mean zero work or
100% improvement; a separate batched measurement observed roughly 10 ns versus 4.9 microseconds
at 10,000 idle timers. The eight-sample largest cancellation tails are coarse.

These are native data-structure measurements, excluding V8, callbacks, the engine and RSS.
Final exact-source timer reruns and integrated live validation remain stack-wide. A separate
slice-8-only live install was skipped after its Linux gate; the final release will include
these changes with the later loader and mechanical extraction slices.
