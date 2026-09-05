# Slice 2: Prune subscription indexes

**Status:** Complete: independent review, local/Linux gates, and 1,000 live reloads passed.
**Branch:** `core/hardening-02-channels`
**Parent / PR base:** `core/hardening-01-ledger`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/channels.rs, core/src/multiplexer.rs, core/src/owner_stores.rs if its
interface needs adjustment. **Tests:** channels.rs and owner-disposal integration tests.

**Boundary:** Removal returns emptied channel names for engine unsubscribe, even when the
descriptor itself has already been removed. Snapshot semantics remain unchanged.

- [x] Pin the shared-channel case: persistent owner A, 1,000 subscribe/unload cycles for B;
  assert one live subscriber and one reverse ID mapping.
- [x] Pin unique-channel churn: create and dispose 1,000 channels; assert zero subscribers,
  zero reverse mappings, and zero retained empty descriptors.
- [x] Store sufficient ownership in the reverse index or return exact removed IDs from the
  descriptor. Remove only the disposed subscriptions' mappings, including remove_by_owner_on.
- [x] Remove empty descriptors after collecting engine unsubscribe notifications. Cover
  duplicate disposal, unknown IDs, and a handler subscribing/unsubscribing during dispatch.
- [x] Run focused channels tests, core tests, and owner teardown/reload tests.

## Evidence required before completion

- [x] Record the regression test and its failure on the parent implementation.
- [x] Record implementation commits and passing focused checks.
- [x] Record applicable full-gate and live-server results, with environment limitations stated.
- [x] Review the diff against the parent and restack descendants using recorded old tips.
- [x] Set status to complete only when this slice's required gates pass.

## Local evidence

Base captured before implementation: `431f1a9`. Production commit: `f3fa748`.

The five new channel regressions failed on the parent implementation as expected: shared-channel
owner churn retained 1,001 reverse mappings instead of one; unique-channel churn retained 1,000
empty descriptors; `remove_by_owner_on` retained the disposed owner's reverse mapping; final ID
disposal retained its empty descriptor in both the idempotence and snapshot-mutation cases.

After implementation, the focused channel set passed 13/13. The owner teardown filter passed 24/24,
the reload filter passed 11/11, and the exact macOS full-core command passed 663/663 with the same 10
baseline warnings. `make check-boundary` and `git diff --check` passed. The repository-wide formatter
check remains red on pre-existing formatting across untouched files; no broad formatting was applied.

## Controller verification

- Independent Sol reviewer approved spec compliance and code quality without findings.
- Full Linux `scripts/ci-native.sh` passed on `7ed7220`, including 663 core tests in 10.48s,
  shim build/self-tests, and symbol checks against the actual CS2 engine installation.
- Release build passed in the Bullseye builder with the same 10 baseline warnings. Installed core
  SHA-256: `dd3d2569b12e396ac14c1d96081cce50ff5b22aabf515f1ead228a26f82f8b79`.
- Isolated CS2 container `s2script-cs2-hardening`, port 27016: 100 batches of 10 antiflood reloads
  produced exactly 1,000 acknowledgements and 1,000 `Active` transitions. All 14 base plugins
  remained running. The captured suffix had no panic, fatal, segmentation-fault, load-error,
  or save-error matches. This is lifecycle churn validation, not a frame-performance claim.
- The first custom RCON harness failed: a timeout could discard a partial packet and desynchronize
  subsequent reads. Its failed run is retained as evidence; the passing run uses the repository's
  existing complete-packet client under a process timeout. No sanitizer or runtime check was bypassed.
- Whole-process RSS was captured separately and is not attributed to pure core or V8 allocations;
  retained-index regression tests supply the exact logical storage assertions.
- Logs: `/tmp/s2script-hardening-nebula-slice2-ci-native.log` and
  `/tmp/s2script-hardening-live-batch10-gate.log`. Detailed live artifacts are in the controller's
  scratch `live-churn/batch-20260905T060929Z/` directory.
- Descendants were restacked after implementation. This evidence-only update is carried forward
  with the next reviewed slice; integrated soak and performance comparison remain final-stack gates.
