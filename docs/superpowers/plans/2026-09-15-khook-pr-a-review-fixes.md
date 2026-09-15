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
- [x] Keep actual installed artifact receipt generation executable; reject wrong
  binaries and stale runs. Keep real-client capture and restore paths documented.
  The following packages are implemented, independently reviewed and integrated
  in the PR A worktree through `6a7cc3a`. Repeat Linux validation on the published
  head; actual receipt generation still requires the live server:

  | Package | Owner and reasoning | Owned files | Evidence |
  | --- | --- | --- | --- |
  | R1 test bundle and JS identity | Sol, medium | new build wrapper/tests; fixture identity module, runtime command and VM tests | fresh build provenance, dirty/failing build rejection, stamped and unstamped fixture runtime |
  | R2 receipt generator | Sol, high | new Python generator and tests | installed hash/manifest checks, stable native/JS witnesses, stale/malformed input rejection, atomic evidence publication |
  | R3 mapped native modules | Astra, high | probe runtime response, helper and focused tests | actual mapped device/inode compared with installed files; missing, replaced and ambiguous modules stay pending |

  These are test-only changes. Root owns README/protocol/CI joins. No production
  loader, SDK, core or dependency modification is required for this tooling.
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

## Runtime identity contract for R1–R3

The bundle wrapper rebuilds shim, core, probe and fixture from one clean, unchanged
source revision and stages the resulting files under an ignored addons tree. It
must not assign the checkout revision to arbitrary preexisting binaries. Generate
the fixture revision and unique build token in a staged source copy; an ordinary
unstamped fixture remains buildable but reports pending identity.

The test bundle manifest uses schema 1, kind `khook-runtime-build`, source revision,
s2script commit, fixture revision/token and exact relative paths/hashes for those
four artifacts. The existing schema 2 Metamod manifest remains separate.

The native `runtime` response becomes schema 2. It identifies the actual mapped
probe, shim, core, Metamod and Metamod loader, including canonical paths and mapped
device/inode values checked against the current files. Distinguish the Metamod
loader from the engine's identically named `libserver.so` by addon layout. A
missing, replaced, deleted or ambiguous module cannot report ready. The independent
JS runtime response reports its embedded fixture revision/token and generation;
mutable probe cvars cannot supply the active script's build identity.

The generator runs in the server's filesystem namespace. Sample both runtime
responses before and after verification; require stable process, generations,
revisions, map, server build and module identities. Verify installed Metamod using
the production verifier, match loaded host modules, compare all four installed
artifacts with the build manifest, and compare the active fixture token/revision.
Validate an existing pending run before side effects. Publish captured evidence
and the existing unsigned operator receipt only after all checks succeed; never
overwrite existing evidence. Offline tests do not establish live server identity.

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

Both Linux JS and native CI passed for `29d9176`, including 904 Rust tests, the
unmodified Metamod/KHook reference build and server-runtime shim/core/probe ELF
checks. The reference host uses Clang; consumer builds retain GCC. Repeat the
applicable gates after integrating R1–R3. This build result is not live CS2 proof.

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
for pending cleanup. These require plugin-side resolution. That source audit did
not demonstrate a crash. The subsequent observations below distinguish failed
probe-record loader cleanup from a later probe-owned entity cleanup crash; neither
discharges the separate production lifecycle gaps.
Native library unmapping remains outside acceptance.
Runtime identity tooling now passes 12 builder tests, 23 actual JS fixture tests,
18 generator tests and 58 controller tests. The native mapped-module helper and
actual command formatter pass local sanitizer tests. Native CI for `4a972b5` also
passed the actual Linux loaded-DSO replacement, deletion, ambiguity, loader-layout
and symlink cases. That run later exposed generated compiler/Python caches being
classified as source changes; the reviewed correction ignores those cache directories
while preserving rejection of real tracked and untracked source changes. The full
bundle wrapper remains part of native CI. Independent review resolved unsafe
symlink cleanup, runtime command mismatch and malformed pending-witness handling.
The corrected probe produced an installed-runtime receipt on Nebula; the failed
suite and remaining live/client observations are recorded below. Keep PR #221 draft
and PR B blocked. None of these
remaining tasks justify reintroducing a host patch or native hot-reload requirement.

## Nebula execution — September 15

The Sol/medium remote operator used only `s2script-cs2-hardening` on host port 27016.
The separate `s2script-hudlab` instance and its mounts were excluded. A fresh
self-contained clone at `e6b89d9` passed the full runtime bundle build. Before
replacement, RCON showed zero humans and two bots. The verified preinstall backup
contains 116 files at
`/home/ghirakawa/s2script-khook-backups/e6b89d9-preinstall-20260915T174030Z`.
Its checksum-manifest SHA-256 is
`1ade9b6a6631de1e9023d993db3309f01705a36eee0d38121cf5b30566a8dca7`.
The old runtime's Docker-stop timeout/exit 137 is a separate baseline observation.

The official stock Metamod archive and installed tree verified successfully. The
four fresh runtime artifacts matched their manifest; shim and stamped JS loaded.
These measurements isolated the acceptance JS fixture; the previous 29 archives
(25 active plugins), configs and data were preserved. The probe failed to load
with unresolved `_Z20MurmurHash2LowerCasePKcj`, preventing a runtime receipt and
the live suite. A second Sol/medium worker fixed the missing existing SDK support
source and added an actual probe relocation gate in `7ffbeb6`. Both JS CI
`35004051680` and native CI `35004051696` passed on that head, including a fresh
bundle build. The corrected probe has not yet been deployed on Nebula.
Nebula subsequently rebuilt the full `7ffbeb6` bundle successfully from a clean
checkout. All four hashes were recomputed, and the relocation check reports only
the permitted host-provided allocator symbol.

Two `e6b89d9` CS2 child-process SIGSEGVs were captured with the probe absent. The
first followed an explicit `meta retry` of the failed probe (the retained transcript
brackets the crash but lacks individual command timestamps). The second followed
a real RCON `quit`, reached
`Source2Shutdown` at 17:51:43.802976 UTC and segfaulted about 312 ms later. The
wrapper reported exit zero with no OOM and no restart policy. Complete logs,
minidumps and sidecars are retained under the operator's
`.gate/remote-khook-pr221/` evidence directory; their copied hashes were verified.
Cross-checking the saved AMD64 contexts maps both faulting instructions to
`ld-linux-x86-64.so.2 + 0x15961`. The exact matching loader binary (build ID
`1b3277a419c3fa42b199e5a170ea215b32689793`) reads offset `0x31f` through a null
argument there. Breakpad stack scans identify stock Metamod `Unloader::Check`,
`CPlugin::~CPlugin` and `_Unload`, followed by `Retry` in the first capture and
`UnloadAll` in the second. These scanned return addresses symbolize against the
exact preserved host binary; they are not a complete CFI-unwound stack.
An independent source audit confirms both failed-record deletion paths can pass
the failed probe's null library handle to `dlclose`. Together these observations
strongly support failed-probe cleanup as the cause. They do not establish a fault
in s2script's hook-retirement coordinator. The plugin-side loader correction is
the next live test; no upstream change is part of this remediation.

After a temporary SSH signing interruption, the operator verified all backup
checksums and restored the original addon and Metamod trees. The test server is
running again on port 27016 with zero humans, two bots and the original 25-plugin
state (23 running and two UI fixtures unloaded). Failed runtime trees are retained
separately. The corrected full bundle was built in the isolated clone while
the original server remained running. The next bounded run uses the corrected
bundle and captures runtime identity before automated checks, with the disposable
debugger available to capture any new fault. Preserve the crash artifacts and repeat actual
loading, both plugin orders and shutdown. A separately labelled focused `.s2sp`
reload diagnostic may run without clients, but it cannot complete the full staged
acceptance or replace missing human observations. No live suite or reload pass has
been claimed from this attempt.

The corrected `7ffbeb6` probe-first run subsequently loaded both native plugins
and produced a genuine installed-runtime receipt. The run ID is
`khook-a-f32514a556fd4b56a6841d8e37e350ac`; the receipt SHA-256 is
`6c26255d20f9e9e07cd161753f2e94e51db76d47d20456ef804ce38c5efbba06`.
The first collect failed: the controller reported three passing cases, six pending
and two failing, with the peer-action case additionally missing because its records
contained invalid JSON. Exact optimized probe disassembly shows direct controlled
target calls let compiler interprocedural assumptions precompute false verdicts.
The matching displayed invocation counts therefore did not make that test valid.
The probe also installed its independent Touch Function observer before the shim
scanned the Touch signature; startup logged probe resolution followed by shim
signature failure, and the script's SDKHook registration failed.

Stopping this run exposed a distinct shutdown crash: the probe's `R6CleanupOwned`
called `UTIL_Remove` after world teardown. The saved fault is in game `libserver`,
with scanned callers through `ProbePlugin::Unload`; it is not the earlier failed
probe-record cleanup. Docker again reported exit zero. GDB had been detached
before quit, so its earlier SIGINT capture is not the crash capture. Preserve the
minidump, raw run, startup/shutdown logs and receipt under
`.gate/remote-khook-pr221/live-7ff-probe-first-failure-20260915T1846/`.
The original server and all 25 plugin states are restored and verified.

A Sol/medium implementer now owns the probe corrections: opaque runtime calls
with complete numeric verdict evidence, delayed independent Touch observation,
and public level-lifecycle tracking that forbids world access after shutdown.
The Sol/medium remote operator independently checked the optimized binary and
preserved the crash evidence. Review the fixes, rebuild the complete stamped
bundle, then repeat probe-first, script reload, actual shutdown and reverse order.
Human observations, script reload and reverse-order acceptance remain incomplete;
the production shim's separate S3 obligations are not discharged by a probe fix.

The reviewed probe corrections were published with this plan in `c1f746a`.
Both CI jobs and the fresh Nebula bundle build passed. Actual probe-first loading
and runtime identity passed again, for run
`khook-a-e038254599644254b5181f68215cba4f`; receipt SHA-256 is
`88d6ceea46a42a74facd2ba9a7f43f5001a1d72327cf04565251c3fb00083756`.
The optimized probe now has all nine indirect controlled target calls.

Preparation then aborted immediately after the JS prepare command with
`double free or corruption (out)`. The saved stack contains probe return addresses
`+0xc262` and `+0x1f358`; exact disassembly identifies `ProbeSetCvarString` returning
from libc `free`, called from the phase acknowledgment in `Hook_GameFrame`.
That function allocates through `g_pMemAlloc` but frees the old engine-owned string
through libc. Earlier scanned hook-callback addresses are not sufficient to blame
KHook. The Sol/medium implementer owns a narrow allocator-consistency correction
and a regression using an allocator whose returned storage is not libc-owned.

The original server and all 25 plugin states are restored. Preserve this failed
run under `.gate/remote-khook-pr221/live-c1f746a-prepare-abort-20260915T1931/`.
Root verified all 62 copied evidence hashes. No collect, script reload, reverse
order or corrected shutdown was reached. GDB stopped on SIGABRT but its script
mistook a stopped inferior for an exited one; update the diagnostic script to
capture all fatal signals and explicitly distinguish stopped from exited before
the next live attempt. The original preinstall shutdown was clean and does not
establish shutdown safety for the corrected bundle.

The allocator correction was published in `f9fc704`. Both CI jobs, the full Nebula
bundle and the exact compiled allocator-call check passed. The live probe-first
run produced receipt `ff99e7fd263cf3707a6a1e18493a582695f14132149c25f1d617655e37383cc9`
for run `khook-a-502b5571fa8f486bb5677bed3ba2f8ef`. Prepare/collect remained alive:
six cases passed, five were pending for clients/map/reload, and one failed.
Only the peer Override/Override expected returns differed: AB observed 99 versus
expected 7, and BA observed 7 versus expected 99. Independent pinned-source review
confirmed that the fixture had assumed registration order. The wrappers install
both generated thunks; the insertion code places each new uniform wrapper before
the prior one, and strictly higher actions preserve the first executed equal vote.
Correct the expectation and record callback execution order explicitly; retain all
count/original and mixed-priority assertions. Do not change production precedence.

Ordinary RCON quit then reached probe level invalidation and the production shim's
pending-retirement response, followed by network shutdown and a real child
SIGSEGV. Docker returned zero again. The original server and all 25 plugin states
are restored. Preserve the 51 initially verified evidence files under
`.gate/remote-khook-pr221/live-f9fc704-peer-order-failure-20260915T1949/` and the
later offline analysis alongside them. No reload or reverse-order run followed
the failure. The saved dump identifies an invalid-pointer read in the exact
`libtier0.so` at offset `0x2ea670`; the recovered frame-pointer chain reaches the
engine and server executable, with no shim/probe/core frame. All relevant modules
remain mapped. It does not identify the residual resource responsible. Logs and
stock source do establish that s2script's final cleanup was skipped: forced
Metamod removal does not retry a pending `Unload` response.

The next bounded S3 task is an observation-only probe trace of the public
`ISource2ServerConfig` / `IAppSystem` `PreShutdown` and `Shutdown` PRE/POST phases,
followed by the plugin's `Unload`. Acquire the named public interface and derive
slots from SDK member pointers. Preserve the original call and retain trace-hook
objects until their stacks have returned. Run the trace on the owned server with
the corrected peer fixture, then select the production cleanup boundary from the
result. A separate worker reviews synchronization and resource lifetime before
integration. Diagnostic work uses saved artifacts, ordinary logs and public API
observations; no host patch, debugger attachment or native hot reload is required.

The peer correction and initial public-lifecycle trace were published in
`9ae2d69`; both CI jobs and the full Nebula build passed. Neither subsequent
startup attempt reached acceptance. The first test tree omitted required package
data; the corrected tree verified all three master gamedata files, JavaScript,
the isolated fixture, the four artifact hashes and stock host, then also crashed
before RCON readiness. Both saved dumps identify the same stock-loader write:
`libserver.so+0x17958`, `mov %rdx,0x8(%rax)`, with fault address exactly equal to
the logged server-config vtable plus eight bytes (`Disconnect`, slot 1).
No lifecycle PRE/POST callback fired. This is separate from the prior shutdown
failure.

Pinned source explains the startup interference: KHook virtual setup restores
the vtable page to RX, while the loader writes its `Disconnect` entry after
original server initialization and `AllPluginsLoaded`, without changing that
page's protection again. Move only the new probe lifecycle registrations to the
first accepted game frame; preserve their off-stack synchronous removal. Add
load/current thread IDs to the trace. Do not change upstream code or page
protection. The original server and all 25 plugin states were restored and
verified. Repeat the corrected trace before selecting the production S3 boundary.
