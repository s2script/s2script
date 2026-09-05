# Slice 1: Make the ledger track active resources

**Status:** Planned; implementation has not started.
**Branch:** `core/hardening-01-ledger`
**Parent / PR base:** `main`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/plugin.rs, core/src/jobs.rs, core/src/v8host.rs and resource release paths
in core/src/{db,sqldb,ws,net,interfaces}.rs. **Tests:** plugin.rs, jobs.rs and the existing
in-isolate timer/job tests in v8host.rs.

**Boundary:** Resource acquisition records ownership; completion/disposal releases the same
resource from the matching owner generation. Unload consumes surviving entries in reverse order.

- [ ] Add tests for 100,000 one-shot timer completions, 100,000 job completions, explicit disposal
  followed by unload, and late completion after owner reload. Assert retained ledger entries
  equal active resources, not total historical acquisitions.
- [ ] Replace duplicated append-only vectors with an ordered active-resource representation.
  Use a monotonic acquisition sequence plus an index for removal; do not replace growth with
  an O(history) scan on every completion or permanent tombstones.
- [ ] Add a generation-aware release operation and call it on timer completion/cancellation,
  job completion, connection close, hook disposal, and interface subscription/import release.
  Preserve any acquisition multiplicity that existing consumers depend on.
- [ ] Test repeating timers retain one entry while active, self-kill releases it, and unload
  remains exactly once in reverse acquisition order. Verify teardown does not double-borrow
  REGISTRY while releasing entries from a registry already being removed.
- [ ] Run core tests and compare churn/teardown measurements against baseline.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.
