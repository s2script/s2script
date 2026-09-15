# KHook PR A review fixes — dynamic implementation plan

**Goal:** Resolve PR #221's review findings on stock Metamod, with `.s2sp` hot reload
managed inside resident s2script. Native shim hot reload is not required.
**Spec:** [Current remediation design](../specs/2026-09-14-khook-pr-a-remediation-design.md).
**Decision:** [Stock-host scope](../specs/2026-09-15-khook-stock-host-decision.md).
**Published implementation:** PR #221 commit `71d7464` contains the reviewed stock
delivery and script-reload changes. Start remaining work from the current PR head.
S1 and S2 are complete; do not redispatch them. The original local worker baseline
`fccea2b` is execution history, not a required checkout for a new agent.

## Execution rules

Use the user's dynamic subagent workflow. Assign bounded work to isolated branches
with explicit owned files and baseline SHAs. Integrate sequentially after review;
workers do not push, modify other workers' files or dispatch additional agents.
Reuse available agents for scoped review. Escalate unresolved architecture decisions
rather than implementing speculative host workarounds. Keep evidence in the ledger.

No Metamod/KHook source patches, gitlink changes, private host builds as operator
requirements, writes into host internals or native hot-reload gate. Preserve JS APIs,
PR A scope, pending/failed distinctions and all other migration acceptance. Keep PR
#221 draft and PR B blocked until accepted. Do not merge or release.

| Package | Owner | Files | Dependencies |
| --- | --- | --- | --- |
| S1 stock delivery | delivery worker | patch directory removal; host build/verifier/archive preparation; cloud installer/tests; sniper build helper; notices; BUILDING/INSTALL | confirmed schema 2 contract |
| S2 script reload and evidence | fixture/controller worker | native/JS acceptance fixtures, controller/tests, probe README/testdata, runner | confirmed reload proof; host manifest digest stays opaque |
| S3 native lifecycle | coordinator + scoped reviewer | shim lifecycle/bindings and focused tests only after shutdown boundary validation | real engine ordering evidence before choosing shutdown implementation |
| S4 integration/docs | coordinator | remediation and parent docs; CI wiring; cross-package joins | S1/S2 reviewed commits; S3 evidence |
| S5 final review | independent reviewer | read-only final diff, baseline failures and validation evidence | S4 |

S1 and S2 run independently. The coordinator corrects scope/docs and audits native
lifecycle alongside them. There is no dependency patch implementation package.

## S1 — stock delivery

- [x] Delete the dependency patch series and application path. Prepare exact
  unmodified source for optional CI builds without changing vendored checkouts.
- [x] Replace patch-dependent manifest schema with schema 2 stock provenance.
  Support official releases with expected archive checksum and safe extraction;
  optional source builds record exact checked source SHAs. Keep actual file hashes,
  required loader layout, ELF architecture and GLIBC validation.
- [x] Preserve transactional installation, running-server refusal, verification of
  existing destination bytes, rollback and invalidation of stale build receipts.
- [x] Update operator docs and deterministic notices. No mandatory private host.
- [x] Run source-preparation, artifact/archive and installer regressions, including
  malformed/empty binaries, escapes, stale receipts and failure before replacement.
  Report missing platform tools honestly. Commit for independent scoped review.

## S2 — `.s2sp` lifecycle and trustworthy evidence

- [x] Keep twelve cases; replace misleading `native_unload_reload` subcheck with
  `script_hot_reload` under `entity_slot_reuse_map_teardown`.
- [x] Arm a real before/reload/after sequence with resident shim and probe. The
  probe retains a native target; old JS generation installs test PRE/POST callbacks
  and owns a marker. After actual `.s2sp` reload, require new generation PRE=1,
  POST=1, original=1, old callbacks=0 and old marker gone. Leave dedicated test
  subscriptions to ledger cleanup. The marker is a game-world entity explicitly
  removed in OnPluginEnd; it proves that cleanup path, not entity-ledger disposal.
- [x] Resume only JS run binding after reload. Do not replay completed initial
  filter/phase observations or replace persisted terminal evidence.
- [x] Add actual fixture negative tests for duplicate/stale/missing callbacks,
  missing original, unchanged generation and leaked old resources. Update native,
  JS, parser fixtures and README as one contract.
- [x] Fix controller validation before pending shortcuts and before side effects:
  malformed supplied identity always fails; run IDs are bounded command-safe tokens.
- [x] Retain independent command/original, phase removal, event consumption,
  human-observation and immutable-history corrections. Run controller, real fixture,
  observer, typecheck/build and parser positive-count regressions; commit for review.

## S3 — resident shim and safe process shutdown

- [x] Confirm script subscription removal only changes plugin-owned routing/rows;
  shared native hooks/provider remain resident and peers are unaffected.
- [x] Remove tests and requirements solely for modified-host native unmapping.
  Preserve real checked-binding ownership tests and completed-helper cleanup.
- [ ] Validate stock engine shutdown order and resource availability on a live
  server. Trace an earlier public shutdown phase, callback/original return and
  Metamod Disconnect. The current callback-only active count cannot prove that no
  hooked original is on the stack. No synchronous self-removal or busy waiting.
- [ ] Keep ordinary native unload during gameplay nondestructive and unsupported;
  it must not begin whole-shim retirement. Choose the smallest plugin-side shutdown
  implementation supported by the ordering evidence, retaining callback contexts
  and the provider through all physical
  removals. Validate partial-install and degraded-load ownership separately: the
  existing core-init failure path stays loaded for diagnosis. Do not invent a
  false-return load path merely to satisfy a checklist. Do not replace missing engine
  evidence with a guessed phase hook, private-host patch or a refusal-only claim.
- [ ] Add meaningful production-path regression tests for the selected lifecycle
  and repeat real process shutdown. If the server is unavailable, record the exact
  unmet prerequisite and leave this package incomplete.

## S4 — integration and documentation

- [x] Review S1/S2 commits before sequential integration. Reconcile schema 2 host
  provenance with existing acceptance `host_manifest_digest`; do not infer runtime
  identity from the controller checkout.
- [x] Remove patch-specific lifetime harness/gate and stale instructions. Wire
  retained/new tests through ci-native/ci-js scripts. The existing workflow retains
  two inert patch-directory path filters: removing those optional entries requires
  workflow-write authorization unavailable to this session. They do not apply patches
  or change which retained tests run.
- [ ] Keep actual installed artifact receipt generation executable; reject wrong
  binaries and stale runs. Keep real-client capture and restore paths documented.
- [ ] Run focused gates once integrated, then applicable full JS/native gates and
  Linux/sniper builds. Linux/live absence remains pending, not a passing local test.
- [ ] Update docs-only PR #220 without introducing code files, and PR #221 without
  overwriting new remote work. Check CI for the pushed head. Report incomplete S3
  or human/live acceptance explicitly; keep draft status.

## S5 — review and evidence

- [ ] Independent review of final scope, native lifetime, script cleanup, stock
  delivery and immutable acceptance. Implementation owners resolve findings and
  rerun affected checks before re-review.
- [ ] Map F1–F5 to actual commits/tests and remaining live observations. No false
  final-pass claim from parser success, mock counters or a build-only result.

## Current evidence

Stock delivery and script-reload/evidence packages are implemented and independently
reviewed, published together in `71d7464`. Individual worker commits and test logs
remain recorded in the local execution ledger. No findings remain from those
scoped reviews. The obsolete patched-host lifetime harness and dependency
patch directory/application path have been removed.

Fresh integrated validation passed 16 source/manifest/archive tests, 135 installer
checks, 58 controller tests and the shared shell judge self-test. Checked binding,
coordinator and actual command/native driver sanitizer harnesses also pass; these
are unit/engine-boundary tests and do not establish terminal shutdown safety.
The fixture worker and independent reviewer verified the 21-test JS fixture suite,
plugin build and retained-marker negative. The full integrated JS script passed
its available checks, then stopped at the Docker-only final gate because Docker
is unavailable locally. Linux/native CI is tracked separately in the execution
ledger.

The actual official Metamod `2.0.0.1467` Linux archive matches GitHub's published
asset checksum; its tag resolves to the pinned upstream source. Production release
preparation and separate artifact verification pass using real GNU readelf 2.43.
This is real artifact inspection on macOS, not Linux execution or live CS2 proof.

S3 remains incomplete: stock process-shutdown ordering, quiescence and engine
resource availability need live validation before selecting the smallest plugin
cleanup implementation. No native shutdown code has been falsely declared fixed.
Independent review of `30f0cbe` confirmed script subscription removal changes only
routing references/filters and the runtime replacement test is Metamod agnostic.
It also confirmed two current control-flow gaps: an ordinary rejected native unload
can begin retirement and disrupt gameplay, and forced host shutdown offers no retry
for pending cleanup. These require plugin-side resolution; an actual shutdown crash
has not been demonstrated. Native library unmapping remains outside acceptance.
Actual installed-runtime receipt generation and remaining live/client observations
also remain integration work. Keep PR #221 draft and PR B blocked. None of these
remaining tasks justify reintroducing a host patch or native hot-reload requirement.
