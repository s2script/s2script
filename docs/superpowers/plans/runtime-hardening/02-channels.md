# Slice 2: Prune subscription indexes

**Status:** Planned; implementation has not started.
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

- [ ] Pin the shared-channel case: persistent owner A, 1,000 subscribe/unload cycles for B;
  assert one live subscriber and one reverse ID mapping.
- [ ] Pin unique-channel churn: create and dispose 1,000 channels; assert zero subscribers,
  zero reverse mappings, and zero retained empty descriptors.
- [ ] Store sufficient ownership in the reverse index or return exact removed IDs from the
  descriptor. Remove only the disposed subscriptions' mappings, including remove_by_owner_on.
- [ ] Remove empty descriptors after collecting engine unsubscribe notifications. Cover
  duplicate disposal, unknown IDs, and a handler subscribing/unsubscribing during dispatch.
- [ ] Run focused channels tests, core tests, and owner teardown/reload tests.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.
