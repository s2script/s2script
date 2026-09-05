# Slice 9: Prepare plugin/config changes on a worker

**Status:** Planned; implementation has not started.
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
