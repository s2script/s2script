# Slice 6: Bound async admission and frame processing

**Status:** Implemented and independently reviewed; local/Linux gates and bounded live component checks pass. Final integrated 60-minute soak remains pending.
**Branch:** `core/hardening-06-async-budgets`
**Parent / PR base:** `plugins/hardening-05-cookie-persistence`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Create:** core/src/async_limits.rs for policy/counters. **Modify:** core/src/{lib,http,ws,net,
db,sqldb,async_rt,jobs,v8host,cookies}.rs and affected SDK declarations/prelude adapters.
**Tests:** each engine's saturation tests and in-isolate frame ordering tests.

**Boundary:** Count bytes before copying/enqueueing where possible. Reserve capacity before
ledgering a job. Never block the game thread on a bounded synchronous sender.

- [ ] Add deterministic tests with tiny injected capacities: admission rejection creates no job
  or ledger leak; queues plateau; control/close signals survive a full data queue; one busy
  source cannot prevent another source's completion or timer progress.
- [ ] Introduce configurable policy values in one module. Establish shipping defaults from the
  baseline workloads; test code injects its own small values rather than relying on defaults.
  Record the chosen count/byte limits and their rationale in the slice's PR and operator docs.
- [ ] Bound per-owner outstanding jobs, per-connection outbound data, global inbound/completion
  bytes, database result rows/bytes, and the cookie outbox. Oversized results become named
  errors before unbounded materialization; streaming producers await capacity off-thread.
- [ ] Preserve existing return contracts: send returns false when not accepted; rejected async
  admission settles with a named error. Any API without an overload signal must add one with
  appropriate compatibility/version handling. Already accepted cookie writes remain owned.
- [ ] Replace drain-until-empty loops with fair count/byte-limited batches and a soft elapsed-time
  stop between items. Leave remaining work queued for subsequent frames. Preserve ordering of
  connect settlement, microtasks, message delivery, terminal delivery, and registry pruning.
- [ ] Export queue depth, queued bytes, rejections, and drain-duration metrics for the dev
  harness. Explain that one JS callback/microtask checkpoint can exceed the soft budget.
- [ ] Run sustained producer/slow-consumer tests, core gates, and a live saturation/recovery soak.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.

## Reviewed implementation and evidence

The runtime now uses process-stable count/byte admission and lifetime leases across isolate
resets, per-owner limits, bounded completion/input/socket/timer partitions, and fair frame
poll/delivery budgets. Old detached producers retain their reservations until their actual
payloads and actors drop. SQLite input leases survive input destruction, UTF-8 and row
capacity growth are charged, and acknowledged offline cookie cache entries are evicted.
See [operator limits and limitations](../../../ASYNC_LIMITS.md).

Independent review found and closed poll phase-lock starvation, early SQLite input release,
and undercharged materialized buffer capacity. The final implementation also removes its
new compiler warnings; existing repository warnings remain. `threadSleep` types now reflect
the existing Promise return, and socket send acceptance is a boolean in the SDK minor change.

- Reviewed code at `c5f224a261c0a4f8ea5b7bb80a9946a14344e05d`: 723 core tests pass.
- Both fresh-process pressure cases pass: tiny-policy reinitialization/admission/progress,
  and oversized completion fairness beside a due timer with a full poll round.
- Full JavaScript/Docker gate passes. Linux full native gate passes: 723 core tests plus
  the two separate pressure cases, shim build and symbol checks.
- Isolated Nebula runtime source c5f224a: a 256-job live pressure workload settles with
  64 successes, 192 named AsyncQueueFull rejections, and zero unexpected errors.
- The 300-second mixed pilot completed its workload and returned resource gauges to
  baseline, but is **not a passing acceptance soak**: it recorded two engine navigation
  errors and too few bot cycles for slot reuse. A corrected fixture later proved actual
  slot reuse (20 attempts, two proofs, zero failures); final soak counters start fresh.

Socket lifetime reservations conservatively consume one global job throughout the socket's
life. Frame limits are soft between indivisible work; they do not guarantee an entire engine
frame under two milliseconds. Logical application-owned retention is bounded; upstream
protocol/driver memory, V8 retention, allocator overhead, and whole RSS are not covered.
The final integrated 60-minute loader-aware mixed soak remains required.
