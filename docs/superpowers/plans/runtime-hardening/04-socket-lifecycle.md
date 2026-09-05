# Slice 4: Unify socket termination and cancellation

**Status:** Planned; implementation has not started.
**Branch:** `core/hardening-04-socket-lifecycle`
**Parent / PR base:** `core/hardening-03-client-identity`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/net.rs, core/src/ws.rs, core/src/http.rs only if its spawn API must return
an owned cancellation handle; connection-ledger adapters in core/src/v8host.rs.
**Tests:** net.rs, ws.rs and existing in-isolate network tests.

**Boundary:** One connection produces at most one terminal signal. Explicit owner teardown
removes state without dispatching into the unloaded context. Cancellation is not queued behind data.

- [ ] Add injected writer tests for immediate write failure and a write future that never
  completes. Assert error then close on failure and bounded worker termination on owner unload.
- [ ] Add connect cancellation, TCP connect timeout, peer-close/local-close races, and
  Connected followed immediately by Closed tests. Preserve subscription-before-close ordering.
- [ ] Route write errors through the same terminal path as read errors. Use cancellation/abort
  handles or a separate cancellation signal selected while connecting and writing. Put a finite
  deadline on graceful close and abort the transport when that deadline expires.
- [ ] Retire connection state, handlers, and active-ledger entries exactly once. Ensure stale
  completions after reload cannot settle a new connection's promise.
- [ ] Run local loopback integration tests and core tests. Check no worker/connection count
  growth after 1,000 connect/fail/close cycles.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.
