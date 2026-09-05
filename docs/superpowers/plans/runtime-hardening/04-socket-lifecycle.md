# Slice 4: Unify socket termination and cancellation

**Status:** Complete for automated acceptance; final integrated runtime gate remains stack-wide.
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

- [x] Add injected writer tests for immediate write failure and a write future that never
  completes. Assert error then close on failure and bounded worker termination on owner unload.
- [x] Add connect cancellation, TCP connect timeout, peer-close/local-close races, and
  Connected followed immediately by Closed tests. Preserve subscription-before-close ordering.
- [x] Route write errors through the same terminal path as read errors. Use cancellation/abort
  handles or a separate cancellation signal selected while connecting and writing. Put a finite
  deadline on graceful close and abort the transport when that deadline expires.
- [x] Retire connection state, handlers, and active-ledger entries exactly once. Ensure stale
  completions after reload cannot settle a new connection's promise.
- [x] Run local loopback integration tests and core tests. Check no worker/connection count
  growth after 1,000 connect/fail/close cycles.

## Evidence required before completion

- [x] Record the regression test and its failure on the parent implementation.
- [x] Record implementation commits and passing focused checks.
- [x] Record applicable full-gate and live-server results, with environment limitations stated.
- [x] Record exact child bases and carry this implementation into the next committed slice; later in-progress branches remain part of final stack integration.
- [x] Set status to complete only when this slice's required gates pass.

## Implementation and evidence

Implementation `5ddf58b` and review fixes `f572c64` are restacked onto completed client identity.
The pre-restack review hashes were `3112c208` and `dbf5cd22`; the socket production diff is unchanged.

TCP/UDP/WebSocket workers use a monotonic independent control lane, cancellable connect and
write futures, a 10-second connect deadline and 1-second graceful deadline. Fair read/write
selection runs under control priority. A single epilogue emits at most one terminal envelope;
owner shutdown is silent. Terminal-pending ownership survives connect Promise settlement and
the checkpoint so continuation-installed subscriptions observe data, error, then close before
registry/handler/ledger retirement.

Independent Astra review approved spec and quality after correcting read starvation and two
coverage gaps. A bounded probe reproduced 10,000 inbound chunks with zero outbound bytes under
the biased read selector; the fair selector sent the queued eight bytes. New in-isolate tests
force same-batch connect/data/error/duplicate-terminal ordering for both adapters and exercise
1,000 alternating real-worker lifecycle cycles with actual owner-generation resources and
subscriptions. They assert worker, connection, pending event, mux and ledger baselines, including
late/repeated/stale-generation cleanup. Earlier synthetic adapter tests are labeled separately.

- macOS full core: **693 passed**, plus boundary and diff checks.
- Linux after inheriting the map reconciliation fix: **694 passed in 11.53 seconds**; full
  `scripts/ci-native.sh` passed, including real TCP/UDP/WS loopbacks, injected failure/stalled
  transports, C++ sanitizer selftests, shim build and installed-game symbol checks.
- No SDK/API change in this slice; existing JS gate evidence remains applicable. Outbound data
  admission is intentionally the subsequent Task 6 contract.
- Runtime artifacts are not yet installed on the CS2 test server. The final integrated runtime
  and mixed-workload soak remain stack-wide gates; no engine-specific behavior changed here.

Logs/report: controller plan scratch `task-4-report.md`, `task-4-review.md`, `task-4-core.log`,
round-one focused logs, and `/tmp/s2script-hardening-nebula-slice4-ci-native.log`.
