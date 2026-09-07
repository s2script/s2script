# Slice 9: Prepare plugin/config changes on a worker

**Status:** Implemented and independently reviewed through parent integration; local core/pressure/JavaScript gates pass. Linux and final loader-aware live soak remain pending.
**Branch:** `core/hardening-09-loader-worker`
**Parent / PR base:** `core/hardening-08-timer-index`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Create:** core/src/loader_worker.rs. **Modify:** core/src/loader.rs, core/src/v8host.rs,
core/src/lib.rs, shim config-path plumbing and shared engine-op declarations only if required.
**Tests:** loader.rs, loader_worker.rs, config reload integration tests.

**Boundary:** Worker inputs are owned paths/bytes and revision tags. Worker outputs are owned
prepared data. No V8 handles, engine callbacks, or pointers cross this boundary.

- [ ] Test slow reads without delaying the frame thread, malformed/truncated archives, edit A
  overtaken by edit B, delete/recreate while parsing, plugin unload during preparation, and
  worker shutdown while a result is pending.
- [ ] Capture resolved plugin/config paths on the main thread; preserve current sanitization
  and override rules. Perform periodic discovery, reads, content comparison, and archive parsing
  on the worker with bounded submissions/results and per-path coalescing.
- [ ] Tag requests/results with lifecycle epoch and path revision. Discard obsolete results.
  Apply current prepared results on the main thread with dependency ordering and existing
  failed-reload behavior; never let a worker decide engine liveness or publish interfaces.
- [ ] Preserve watch baselines, missing files, invalid config diagnostics, permissions, and
  incompatible API-version handling. Ensure replacing a file mid-read cannot apply mixed data.
- [ ] Trace periodic frames to verify no directory scan, watched-config read, or archive parsing
  remains there. Run reload/config tests and live hot reload under slow filesystem conditions.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.

## Reviewed implementation and evidence

A dedicated joined worker owns periodic directory scans, stable regular-file reads, archive
parsing, and watched-config comparisons. Requests, result obligations, retained prepared
plugins, config baselines/proposals and retirement controls have finite count/byte budgets.
Generation/revision tags reject stale results, and validation occurs before replacing a
running plugin. READY/WAITING payloads retain their leases. Saturation remains retryable or
returns a named refusal; it does not silently evict a running version.

Config paths use a separate versioned C-ABI resolver registration, preserving the unversioned
S2EngineOps field layout. The shim copies a path into owned main-thread data before worker
submission. An unmatched old shim refuses config-dependent loading by name. FIFO opens use
nonblocking mode before type validation; joined shutdown can still wait on a stalled regular
file kernel operation. Explicit config APIs and crash-spool operations remain synchronous
outside the periodic watcher scope. The separate loader drain is soft between indivisible
lifecycle actions, outside the async frame budget.

Independent review closed latest-change coalescing, Loading/WAITING lifecycle validation,
provider contract stabilization, retained payload charging, UTF-8 expansion, frame-thread
config comparison, and unbudgeted batch application. Baseline acknowledgement occurs only
after all intended consumers apply; bounded retirement is reserved before watched reads.
Focused failures also proved and fixed superseded failed-read proposals, queued reads after
retirement, collapsed generation retirements, stale pending watch re-registration, and
retained payload location during a re-entrant onUnload callback. No review finding remains
open at the final local implementation.

The nested `loader` section of S2SCRIPT_ASYNC_LIMITS_JSON is the single immutable operator
configuration entry point. Unknown/invalid settings use complete defaults. An impossible
individual path/control request returns Oversized; the policy does not incorrectly require
all possible maximum-length paths to fit. [ASYNC_LIMITS.md](../../../ASYNC_LIMITS.md) documents
all limits and the diagnostic schema, including shared config path/byte totals and
active/ready/waiting/applying lease locations.

- Final reviewed implementation `96abe50f3713d35f589b72bcfc93478b0062f2ce`: 791 core tests
  pass, two pressure cases pass, and 80 loader-filtered tests pass.
- Full JavaScript gate on the preceding metrics commit passes all 584 SDK tests, plugin/
  example typechecks, code generation and Docker tests; the last fix only changes Rust.
- Cargo checks, boundary/name/invoke-ABI checks, worker engine/V8 isolation, and production
  loader filesystem isolation pass.
- Engine-order and deferred-sentinel shell checks pass in the Mac's existing linux/amd64
  Docker image with a read-only repository mount: all 126 engine-op names/order/arities
  agree, and the shared deferred sentinel is -1000. This avoids the macOS BSD sed
  parsing limitation. Full Linux native/shim/symbol validation is still required.
- The loader-aware mixed harness has 34 passing deterministic tests. It checks actual caps,
  stable highwaters/limits, bounded combined idle, exact warm-up byte changes, measured
  retention plateau, and exact config restoration/application. This is **not live proof**.

The final 60-minute mixed soak has not run. Worker/main diagnostics are independently
sampled logical accounting, not a transactional snapshot or a bound on whole-process RSS.
