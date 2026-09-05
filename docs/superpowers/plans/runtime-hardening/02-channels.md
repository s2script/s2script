# Slice 2: Prune subscription indexes

**Status:** Implementation and local verification complete; independent review, Linux/sniper, live
CS2 validation, and descendant restacking remain pending.
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
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.

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

Linux/sniper and live CS2 validation are controller-owned and pending. Independent review and
descendant restacking are also pending, so this slice is not marked complete.
