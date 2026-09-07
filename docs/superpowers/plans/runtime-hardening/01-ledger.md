# Slice 1: Make the ledger track active resources

**Status:** Complete: implementation and independent review approved; local, Linux, and live gates passed.
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

- [x] Add tests for 100,000 one-shot timer completions, 100,000 job completions, explicit disposal
  followed by unload, and late completion after owner reload. Assert retained ledger entries
  equal active resources, not total historical acquisitions.
- [x] Replace duplicated append-only vectors with an ordered active-resource representation.
  Use a monotonic acquisition sequence plus an index for removal; do not replace growth with
  an O(history) scan on every completion or permanent tombstones.
- [x] Add a generation-aware release operation and call it on timer completion/cancellation,
  job completion, connection close, hook disposal, and interface subscription/import release.
  Preserve any acquisition multiplicity that existing consumers depend on.
- [x] Test repeating timers retain one entry while active, self-kill releases it, and unload
  remains exactly once in reverse acquisition order. Verify teardown does not double-borrow
  REGISTRY while releasing entries from a registry already being removed.
- [x] Run core tests and compare churn/teardown measurements against baseline.

## Evidence required before completion

- [x] Record the regression test and its failure on the parent implementation.
- [x] Record implementation commits and passing focused checks.
- [x] Record applicable full-gate and live-server results, with environment limitations stated.
- [x] Review the diff against the parent and restack descendants using recorded old tips.
- [x] Set status to complete only when this slice's required gates pass.

## Local evidence

- Regression RED: the new pure ledger tests initially failed to compile because generation-aware
  `record`, `release`, and `active_resource_count` did not exist. After those APIs were staged, real
  timer, job, hook, interface-subscription, import, and SQLite tests each retained the wrong active
  count; same-owner interface republish retained two rows for one registry entry.
- Implementation: `7a212a9` (`fix(core): track only active plugin resources`) and review fix
  `ba2878c` (`fix(core): release subscriptions with producer`).
- Focused checks: the 100,000 timer/job churn tests pass; lifecycle tests cover one-shot and
  repeating timers, self-kill, real job completion, direct hook disposal, interface unsubscribe and
  republish, import replacement, SQLite close, socket failure, and successful WS/net close.
- Full macOS core suite after review fixes: 658 passed, 0 failed in 6.92s with the documented test-only linker wrapper;
  the same 10 warnings as the 646-test baseline remain. `scripts/check-core-boundary.sh` passes.
  Full output: `/tmp/s2script-hardening-slice1-round1-final-macos.log`.
- Standalone Darwin churn comparison: baseline recorded 100,000 completed timers in 2,491us but
  retained 100,000 rows and spent 487us snapshotting them. The active-ledger record/release loop's
  five warm-up samples were 7,272/4,794/4,247/3,815/3,750us, retained 0 rows, and snapshot in 0us.
  This scratch measurement excludes V8 and the engine and is not a live-performance claim.
- Controller validation below completes the external gates. Descendants were restacked after the
  reviewed implementation; this evidence-only update will be carried through at the next checkpoint.
- Review round 1 regression: producer unload originally left a surviving optional consumer at two
  active rows instead of its one import. Producer removal now returns captured subscriber ownership;
  teardown drops callbacks and generation-releases those rows outside the interface-registry borrow.

## Controller verification

- Astra independent review approved spec compliance and code quality after the producer-unload fix.
- Linux x86_64 on Nebula: 658 core tests passed in 10.28s; the full `scripts/ci-native.sh` run
  independently passed all gates and 658 tests in 9.99s. The shim built and all C++ self-tests passed.
- The unchanged Bullseye GCC 10 sanitizer runtime intermittently failed before `main` with the host's
  address randomization, including an empty program. Running the disposable test container under
  `setarch x86_64 -R` passed repeated controls while a deliberate use-after-free still failed correctly.
  No project sanitizer checks were disabled. Container paths mirror the linked worktree; its Cargo
  registry is also exposed at the legacy license generator's expected `~/.cargo/registry` path.
- Release core built in `rust:bullseye`, requires at most GLIBC 2.30. SHA-256:
  `50aee5010346a4cd0cd14d648c760d37c356fc1c4b2776f9b8730015e513285b`.
  The installed isolated-server artifact matched that hash. Engine symbol checks passed against the
  actual CS2 installation for both build and packaged shims.
- Live isolated container `s2script-cs2-hardening`, port 27016: Metamod loaded s2script, all 14 base
  plugins were running, clientprefs reload and reservedslots unload/load succeeded, and the synthetic
  damage self-test remained active. The existing `s2script-hudlab` server was not modified.
- Baseline JS gate constituent checks passed, including the Docker script gate after exposing the
  installed Docker binary on PATH. This slice changes no JS, SDK, or plugin source.
- Logs: `/tmp/s2script-hardening-nebula-ci-native-complete.log`,
  `/tmp/s2script-hardening-nebula-slice1-engine-symbols.log`,
  `/tmp/s2script-hardening-nebula-slice1-live.log`, and
  `/tmp/s2script-hardening-nebula-slice1-reload.log`.
- The integrated stack soak and performance comparison remain final-stack acceptance gates.
