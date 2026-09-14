# KHook PR A Remediation Implementation Plan

> **For the implementation coordinator and subagents:** Use the user's selected dynamic workflow. Execute only assigned R/Step IDs, on explicit baselines with scoped file ownership. This plan prepares fixes to PR A; it does not authorize starting PR B. Checkboxes require evidence.

**Goal:** Resolve review findings F1–F5 in PR #221 and complete PR A's actual acceptance gate.
**Architecture:** Repair and reproducibly build the pinned Metamod host, integrate checked shim retirement, require verifiable runtime artifacts, and finish independent native/JS/human acceptance. Preserve the existing JS API and three-PR migration boundary.
**Tech stack:** Metamod/KHook C++17, AMBuild 2.2+, Linux x86_64 sniper, Rust core shutdown query, Python 3 evidence controller, existing SDK TypeScript fixture.
**Spec:** [KHook PR A remediation design](../specs/2026-09-14-khook-pr-a-remediation-design.md). Read it with this plan.
**Reviewed code:** `57a329b7814c6e1d50f32cc804741ddd14b730bd`, branch `cursor/khook-sourcehook-cutover-8628`, [PR #221](https://github.com/s2script/s2script/pull/221).
**Planning baseline:** `14c7dec930389d2aca2d8db445c7fe568589c5d8`, [PR #220](https://github.com/s2script/s2script/pull/220). The remediation documents are subsequent planning commits; bring them into the implementation checkout before dispatch.

## Global constraints

- Read `CLAUDE.md`, applicable `AGENTS.md`, the remediation spec and the [parent migration plan](2026-09-14-khook-migration.md). Remediation requirements supersede conflicting assumptions in T2–T8; other parent constraints remain.
- Linux x86_64, PLAPI 18, GLIBC <= 2.31 for anything deployed to CS2. Never use a macOS/ARM host result as native trampoline evidence.
- Source baseline: Metamod `7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33`; nested KHook `1e200e4cc8e0badcb7cf941525268d6977f6a4e6`. Deliver host corrections as tracked patches built from those exact inputs, with a patch digest and binary manifest.
- No automatic latest-drop approval, sentinel engine calls, second detour engine, or game-thread waiting for removal.
- Keep callback objects, provider, runtime and library alive for their complete lifetimes. Fix provider ownership and id bookkeeping together before wiring `BeginRemove` into production.
- Preserve JS and FFI contracts; the new shutdown query is an internal shim/core addition built atomically with its consumer.
- Do not implement PR B inline hooks, original-module resolution, damage descriptor conversion or PR C precache here.
- Every live pass requires evidence for its exact run/build. Required missing human evidence remains pending. No worker may fill it with invented observations.
- One amended PR A, not one PR per remediation package. No production release or merge is part of this handoff.

## Dispatch graph and ownership

| Package | Depends on | Worker scope | Verifiable deliverable |
|---------|------------|--------------|------------------------|
| R1 | Baseline and spec | Corrected Metamod patch, host lifetime tests, pinned build recipe | Actual patched provider/unloader passes delayed-removal regressions; reproducible host + manifest |
| R2 | R1 host semantics fixed/integrated | Checked binding and shim/core shutdown | Callback-safe rejection, physical retirement and retry; no lost ownership |
| R3 | R1 manifest schema/build output fixed | Installer/verifier and transactions | Invalid artifacts rejected; corrected artifact installs/skips; rollback proven |
| R4 | Baseline/spec; independent of R1–R3 | Evidence protocol/controller/runner and host self-tests | One authoritative judge with run-bound subchecks and human input |
| R5 | R4 protocol fixed; R1 for real-host execution | Native probe event/command/frame observations | No consumed-event reads; command failure controls detected |
| R6 | R4; R5 observer interfaces fixed | Stateful SDKHooks and human-assisted fixtures | Every required pending case has an executable completion/failure path |
| R7 | R1–R6 integrated | Coordinator, single live operator, final docs/CI | Full PR A acceptance on the integrated revision or explicit pending evidence |

R1 and R4 can start in parallel. R2 and R3 can run concurrently after R1. R5 and R6 can author concurrently only after agreeing the protocol and exclusive ownership of their file sections; both touch the probe/JS fixture, so default to R5 then R6 when ownership cannot be separated. R7 is serialized integration/deployment.

The coordinator is the sole writer of shared `scripts/ci-native.sh`, shared fixture sections and final operator docs unless it grants a lease. Each worker uses an isolated worktree at an exact integrated SHA. Changes needed outside its allowlist are reported for reassignment. One operator owns the CS2 container, host tree, deployment, clients and RCON runs; other workers must not mutate those resources during a gate.

Start a durable workflow ledger with R/Step ID, owner, prerequisites, file allowlist, baseline/result/integrated SHA, status and evidence paths. An implementation report must contain actual commands/results and unresolved failures/pending checks. Distinguish implemented, awaiting integration, host-verified and live-accepted. Apply consumer changes only after interface-producing work is integrated/reviewed. Preserve this mapping if the dynamic workflow splits packages further.

Every worker receives:

```text
Package and steps; bounded objective; exact baseline/branch/worktree
Read: this plan's constraints, assigned steps, spec sections and repo guidance
Inputs: prerequisite commits and exact interface/schema contracts
Ownership: allowed files/sections; coordinator for shared-file changes
Expected checks and outputs; known pending environment/client requirements
Return: scoped commits, changed files, test logs, interface notes and blockers
```

## File responsibilities

New paths below are implementation deliverables, not files supplied by this documentation commit.

| Path | Owner / responsibility |
|------|------------------------|
| `patches/metamod-source/series`, `0001-khook-owned-provider-retirement.patch` | R1: reproducible corrections to the pinned host |
| `scripts/build-metamod-pinned.sh` | R1: isolated source preparation, AMBuild, artifact/manifest generation |
| `shim/tests/khook_host_lifetime_test.cpp`, `scripts/test-khook-host-lifetime.sh` | R1: compile actual patched provider/unloader with controllable backend |
| `shim/src/khook_binding.h`, `khook_map.h`, `shim/tests/khook_binding_test.cpp` | R2: ownership, quiescing, guards and retarget rejection |
| `shim/src/khook_shutdown.h`, `shim/tests/khook_shutdown_test.cpp`, `scripts/test-khook-shutdown.sh` | R2: small production unload coordinator exercised with injected actions |
| `shim/src/s2script_mm.{h,cpp}`, `sdkhooks_vp.{h,cpp}` | R2: lifecycle integration and guarded dispatch |
| `shim/include/s2script_core.h`, `core/src/ffi.rs`, `core/src/v8host.rs`, `core/src/v8host/tests.rs` | R2: actual core-idle shutdown query and its tests |
| `scripts/verify-metamod-artifact.py`, `scripts/cloud/install.sh`, `scripts/cloud/test-install-metamod.sh` | R3: manifest/binary validation and transaction |
| `scripts/khook_acceptance.py`, `scripts/test-khook-live.sh`, `scripts/test-khook-acceptance.py` | R4: one evidence registry, driver/judge and regression tests |
| `tools/khook-probe/acceptance_observer.h`, `shim/tests/khook_acceptance_observer_test.cpp`, `scripts/test-khook-observer.sh` | R5: shared consuming-call observer and engine-free lifetime tests |
| `tools/khook-probe/{plugin.cpp,CMakeLists.txt,README.md}` | R5 then R6: native independent observations and runbook |
| `examples/khook-acceptance/{src/plugin.ts,package.json}` | R5 then R6: JS delivery/decisions and actual stateful exercises |
| `scripts/ci-native.sh` | Coordinator: integrate every new host regression |
| `docs/INSTALL.md`, `docs/BUILDING.md`, `scripts/gen-licenses.sh`, `licenses/licenses.txt` | R7/coordinator: corrected-host compatibility and reproducible notices |

If `v8host` has moved on a newer baseline, locate the implementation of `v8host::shutdown` and use that owning module; do not invent a duplicate host-state singleton.

### R1: Repair host provider ownership and removal tracking

**Spec:** §§3–4 and §6.1. **Finding:** F1; producer for F3.
**Inputs:** exact Metamod/KHook gitlinks and existing `khook_binding.h` receipt contract.
**Outputs:** patched provider lifetime with existing `IKHook` ABI; `build/metamod-pinned/tree/`; `build/metamod-pinned/metamod-build.json`; build log and ordered `patchset_sha256` digest using the spec's manifest schema.

- [ ] **Step 1: Add failures against the real pinned code.** Build an engine-free harness including the actual host provider and unloader code. Use an injected physical-removal backend and a library-release callback so the test can hold an original call open and release completion deterministically. Do not substitute a second implementation of the provider's ownership algorithm. Expose existing Unloader code through a small internal header in the patch if required.

Required named cases and assertions:

```text
provider_during_post: close plugin while original held -> API remains usable in POST
provider_during_remove: helper GetContext during removal -> live API provider
provider_during_dlclose: static destruction calls provider -> valid until close returns
inline_and_virtual: both accepted id kinds belong to the provider
explicit_then_unload: explicit completion then host unload -> no stale-id hang
duplicate_remove: N removal subscribers -> one backend call, N completions
pending_insert: cancellation before first fire -> helper/completion exactly once
unknown_id: absent async removal -> completion, no backend call
reentrant_completion: callback removes another id -> no lock inversion
release_once: no in-flight work -> library/provider released once, no permanent leak
```

Run `bash scripts/test-khook-host-lifetime.sh --baseline` against the unpatched source and preserve failures for provider lifetime/tracking. The normal invocation targets the patched code. If a baseline case cannot be run safely without isolation, run that case in a subprocess and check its failure/ASan diagnostic.

- [ ] **Step 2: Implement the host patch.** Modify `core/metamod_plugins.{h,cpp}`, `core/metamod.cpp` and `core/metamod_khook.h` in the isolated source tree. `GetDetourInterface` must return a stable provider whose shared owner travels into Unloader. Track pending/active/removing ids for both Setup APIs. Join duplicate removal requests and complete absent ids within the provider; never send duplicate requests to the pinned backend's absent-id path. Drain helper and completion callbacks before idle notification. Keep provider ownership through static destruction/dlclose; call external code outside internal locks.

```text
register -> owned id
first Remove(id) -> mark removing, retain context, one backend Remove
repeat Remove(id) -> join waiters
backend complete -> helper already completed; finish waiters; mark complete
retire provider -> refuse registration; join every outstanding id
all outstanding callbacks done -> dlclose while provider retained -> release provider
```

Cover forced-unload and `UnloadAll` delayed paths: no early free and no retry spin. Retain pending teardown and finish/retry at a valid lifecycle point; do not run the plugin's V8 shutdown from KHook's removal worker.

- [ ] **Step 3: Produce an applyable patch and pinned build.** Record only the required upstream source changes in the patch and series. `scripts/build-metamod-pinned.sh` prepares a fresh isolated tree at the exact SHAs, checks clean inputs, applies the series, and fails on drift. It never rewrites the developer's submodule checkout. Compute the digest by the spec's exact pathname/NUL/bytes/NUL rule.

The wrapper runs inside the sniper environment. Use the pinned source's configure interface:

```bash
# Run in the isolated Metamod build directory after AMBuild 2.2+ is available.
# The wrapper sets MMS_PATCHED_SOURCE and S2_REPO to verified absolute paths.
HL2SDKCS2="$S2_REPO/third_party/hl2sdk" \
  python3 "$MMS_PATCHED_SOURCE/configure.py" --sdks=cs2 --targets=x86_64 --enable-optimize
ambuild
```

Stage the complete loader/runtime layout, not just one `.so`. Generate the versioned manifest from the verified source, ordered patch set and successful output. Validate ELF architecture, required loader files, hashes and GLIBC floor; write the manifest last. Record AMBuild/compiler versions. Missing tools produce a precise build failure; a prebuilt file planted in a directory is not success.

- [ ] **Step 4: Verify and deliver.** Run the patched host harness under ASan/UBSan, including delayed insertion/removal. Run the complete pinned-host build. Confirm a second isolated build uses the same source/patch identity; do not require identical binary hashes if the recorded build embeds nondeterministic upstream metadata. Commit patch/scripts/tests with logs and manifest location; runtime output stays untracked. The coordinator reviews R1 before consumers integrate.

### R2: Integrate shutdown and checked binding retirement

**Spec:** §4.4–4.5. **Finding:** F1.
**Inputs:** R1 corrected provider completion behavior; existing SDKHooks/interface inventory.
**Outputs:** nonblocking unload/retry; every owned Virtual retired; internal `int s2script_core_can_shutdown(void)` returns 1 only when actual core state is safe to shut down and 0 otherwise.

`khook_shutdown.h` owns only the small Running/Retiring/Ready decision state; it is not another hook registry. Its production coordinator consumes injected actions with these responsibilities: `can_shutdown() -> bool` (core idle and no active dispatch), `begin_retirement() -> void`, `retirement_complete() -> bool`, `finish_cleanup() -> void`. It returns Busy, Pending or Complete. `S2ScriptPlugin::Unload` supplies real actions; the host test supplies counted/delayed actions to the same coordinator. Call begin/finish at most once and never call finish after a failed readiness check.

- [ ] **Step 1: Add integration regressions before changing teardown.** Extend binding tests with deferred completion, callback guard rejection, global active counts, same-address idempotence, different-address Configure rejection, and retry. Add a core test that queries `can_shutdown` while JS/host dispatch is borrowed, then after return. Add a lifecycle harness covering the actual shim coordinator with injected shutdown/retirement actions; count shutdown calls.

```text
active callback -> Unload=false, zero destructive actions and shutdown_calls=0
original between PRE/POST -> retirement pending, provider retained, shutdown_calls=0
pending completion -> Unload=false, state Retiring, registration rejected
late completion then external retry -> Unload=true, shutdown_calls=1
second teardown -> no duplicate resource cleanup
last subscriber already removed -> physical binding still retired
same Function address -> existing accepted receipt
different Function address while owned -> named failure, old binding still retires
```

- [ ] **Step 2: Implement guards and core query.** Use the actual v8host borrow/dispatch state; do not infer idle from a quiet log or a caught shutdown panic. Track active checked callbacks across threads and guard direct core-dispatch entry paths such as the ConCommand trampoline. Centralize Running/Retiring/Ready state with rejection of new subscriptions/dispatch during retirement. Every callback must honor its guard; merely constructing and ignoring a false Observe is insufficient.

- [ ] **Step 3: Wire physical removal.** Inventory all interface bindings and the fourteen SDKHooks kinds. Begin retirement once per object, even when filters/subscriber rows are empty. Keep context/runtime alive until `S2Hook_DrainRetirement()` is allowed and `S2Hook_RetirementPending()==0`, plus no active core/dispatch hold. First pending unload returns a named retry error without unregistering/finally freeing resources it still needs. Finish existing cleanup and core shutdown exactly once on a safe later retry. No GameFrame-dependent completion loop or immediate binding deletion.

- [ ] **Step 4: Validate on the integrated host baseline.** Run:

```bash
bash scripts/test-khook-host-lifetime.sh
bash scripts/test-khook-binding.sh
bash scripts/test-khook-shutdown.sh
cargo test -p s2script-core
```

Use the repository's single-threaded Rust configuration. Compile the shim with its callers; record pending real command/unload cases for R7. Commit the core query with its shim consumer atomically, and return the complete registration/retirement inventory.

### R3: Verify artifacts and make install rollback complete

**Spec:** §6. **Finding:** F3.
**Inputs:** R1's source/patch identity and generated manifest. **Outputs:** `python3 scripts/verify-metamod-artifact.py --tree TREE --manifest FILE` exits 0 for the expected corrected build, nonzero with a named reason otherwise; installer uses this result before mutation and for skip validation.

- [ ] **Step 1: Reproduce the false verification.** Add the reviewed zero-byte pinned-tree case, a text file containing `GetDetourInterface`, a real wrong-source ELF, wrong architecture, truncated ELF, stale/wrong/missing manifest, modified hash and incomplete loader tree. Keep the previous destination byte-for-byte unchanged on each rejection. Separate verifier tests from transaction tests with explicit injected verification in the latter.

- [ ] **Step 2: Implement one verifier.** Parse the manifest strictly. Compare full source SHAs, target, PLAPI and patch digest with the expected build inputs in the repo. Require all manifest paths to stay inside the candidate tree and all required loader files to be listed. Check each file/hash and actual ELF architecture/GLIBC requirements. Use `readelf`/Python bounded reads and return named errors for unavailable tools or malformed binaries. Require the independent build manifest; never create it from the candidate in this verifier.

- [ ] **Step 3: Replace installer trust paths.** Prefer the corrected pinned build and require its manifest even for `S2_METAMOD_PINNED_TREE`. Add `S2_METAMOD_BUILD_MANIFEST` for the independently supplied manifest path. Remove automatic success from marker strings or "pin-origin" directory names, and disable unverified latest-drop selection. Copy the validated manifest into the installed identity only after success; preserve distinction between build manifest and installation receipt.

- [ ] **Step 4: Make the transaction verifiable.** Stage on the destination filesystem; preflight all files and write the staged receipt before the rename where possible. Check every copy/rename/write result, including rollback. Refuse replacement while CS2 runs. Inject failures before/after each rename and on receipt/VDF writes. Either the complete previous tree remains usable or the complete verified replacement is installed; never return success with a partial tree. The skip path rechecks the artifact, not only its sidecar.

- [ ] **Step 5: Run and deliver.** Run `bash scripts/cloud/test-install-metamod.sh` and verify R1's actual output:

```bash
python3 scripts/verify-metamod-artifact.py \
  --tree build/metamod-pinned/tree \
  --manifest build/metamod-pinned/metamod-build.json
```

Include successful repeat install and preserved rollback pair. Supply exact updated install/build commands for R7's docs owner. No live-tree swap from this worker.

### R4: Define one acceptance controller and judge

**Spec:** §7.1 and §7.3. **Finding:** F4; interface producer for F2/F5.
**Inputs:** existing twelve case names and native/JS records. **Outputs:** shared Python registry of required producer/subchecks; versioned run manifest; structured human observations; exit codes 0 pass / 1 fail-invalid / 2 pending.

- [ ] **Step 1: Define command and record interfaces.** Keep `bash scripts/test-khook-live.sh --self-test` and `A --from-file FILE`. Add staged operation:

```bash
bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
bash scripts/test-khook-live.sh A --judge --run-dir build/khook-acceptance/run-001 \
  --observations build/khook-acceptance/run-001/human.json
```

`run-001` is an example directory, not the identity: generate a unique `run_id` and record exact build/revision/host/server identities inside it. Prepare creates native/JS state once. Collect advances documented actions and stores observations without resetting the run. Judge is read-only. If the default `A` shorthand needs clients, print their specific actions and exit 2, retaining the run for continuation.

Native/JS command protocol: `s2_khook_probe prepare <run_id>`, `s2_khook_probe collect <run_id>`, and equivalent `s2_khook_accept prepare|collect|report|teardown <run_id>`. Preserve a report-only path. Unknown/mismatched runs produce invalid evidence, never silently reuse a prior run.

Publish exact subcheck names/producer ownership in the Python registry and README before R5/R6 dispatch. Keep expected and observed typed fields, evidence references and one terminal status; JSON examples in the README must match executable test fixtures.

- [ ] **Step 2: Write judge regressions.** `scripts/test-khook-acceptance.py` is a standard-library unittest entry point. Test valid all-pass, required pending, actual mismatch, missing record, duplicate native/JS/human subcheck, wrong run, wrong source revision, unsupported schema/case, malformed JSON and contradictory result/legacy pass fields. A forged top-level pass with missing/failed required evidence must not pass. A later JS record cannot overwrite a native failure. `--from-file` and live mode must use the same parser/judge.

```bash
python3 scripts/test-khook-acceptance.py
bash scripts/test-khook-live.sh --self-test
```

- [ ] **Step 3: Implement persistent orchestration.** Store run metadata, commands and raw/normalized records under the run directory so unload/map changes cannot erase them. Implement bounded polling/retries with deadlines and explicit pending/fail reasons. Do not send `prepare` again while collecting a current run. Preserve each producer's evidence and aggregate required subchecks instead of replacing records by case name.

- [ ] **Step 4: Implement human ingestion.** Validate case/subcheck, run/build identity, actor slots/identity, observed outcome, capture path and timestamp. Require the listed native/JS subchecks independently. Supply a schema/example with `pending` observations, never prefilled pass data. Demonstrate in tests that adding valid observations to a complete automated record changes pending to pass, while observations for another run, absent actors or a failed native assertion cannot.

- [ ] **Step 5: Deliver the frozen protocol.** Run both test commands, send R5/R6 the command syntax, record schemas and named subchecks, and have the coordinator wire the host self-tests into native CI. Keep B/C explicitly unavailable; this package does not author those suites.

### R5: Make native event and command observations independent

**Spec:** §5 and §7.4. **Findings:** F2, F5.
**Inputs:** R4 protocol; R1 fixed host for live execution. **Outputs:** shared consuming-call observer used by probe/tests; native evidence joined with JS for frame/client/command case.

- [ ] **Step 1: Add a consuming-original regression.** Extract only the probe's bookkeeping into `acceptance_observer.h`. The host test invokes the same observer around an object whose original deletes it. Invoke POST after deletion; ASan must detect the old event dereference and the fixed code must not touch consumed memory. Add nested matching/nonmatching events, peer-skipped automatic original, one listener delivery and double delivery. Keep callback/object lifetime separate from the copied observation metadata.

```bash
bash scripts/test-khook-observer.sh
```

- [ ] **Step 2: Replace FireEvent observation.** Create a caller-owned scope around the explicit fixture `FireEvent` call. Store only call identity, pointer value for comparison, nesting and counters. PRE/POST correlate to that scope without reading consumed event fields. The engine listener reads only during its valid callback. Ignore unrelated events safely. Record actual listener/message delivery separately from automatic-original skip state so the Handled/explicit-original case is interpreted correctly.

- [ ] **Step 3: Implement independent command proof.** Register a probe-owned test ConCommand whose engine callback counts received tokens. The probe's ClientCommand PRE returns Ignore for these tests; POST records actual skipped state for the same token. The JS acceptance plugin's `command.onClientCommand` supplies Continue or Handled according to the token and reports its invocation. Remove the competing JS `command(...)` registration for this name. Require a real client command; RCON sends are only control operations. Do not use probe votes to make the desired outcome happen.

```text
continue token: js=1, native_pre=1, native_post=1, engine=1, skipped=false
handled token:  js=1, native_pre=1, native_post=1, engine=0, skipped=true
```

Verify the chosen command route before fixing the witness: require real-client ClientCommand entry plus original observation. If the registered ConCommand bypasses ClientCommand, select a valid engine command and observe its validated original boundary using a test-only KHook consumer. Keep the same positive/negative assertions and record the actual commands/route; DispatchConCommand-only counters are insufficient.

Join native frame/client observations with the matching s2script publics/run/client identities. No acceptance plugin -> fail the required delivery subcheck. Add controls for missing JS hook, flipped decision, suppressed Continue and unsuppressed Handled. All controls must be detected by the same judge, then reset before the real run. Peer precedence experiments use different controlled targets/tokens and cannot satisfy these subchecks.

- [ ] **Step 4: Compile and validate fixtures.** Prevent compiler inlining/devirtualization from bypassing controlled target interception in optimized builds (out-of-line noinline targets and virtual calls through runtime-selected pointers). Apply R2's lifetime rules to probe unload as well. Build the probe in sniper and the JS fixture from the repo root:

```bash
cmake -S tools/khook-probe -B build/khook-probe -DS2_SOURCE_DIR="$PWD" -DCMAKE_BUILD_TYPE=Release
cmake --build build/khook-probe -j
node packages/sdk/dist/cli.js build examples/khook-acceptance
```

The root-path SDK invocation is intentional: the existing workspace CLI can otherwise select the workspace plugins. Record host observer/judge results; leave real-client observations pending until R7. Commit the fixture changes together with their schema-compatible negative controls.

### R6: Finish stateful SDKHooks and human-assisted cases

**Spec:** §7.2–7.3. **Finding:** F4.
**Inputs:** R4 persistent run/subcheck registry; R5 native observer interfaces; R2 for unload behavior. **Outputs:** completion/failure paths for the five formerly permanent-pending cases, deterministic cleanup, and a client runbook.

- [ ] **Step 1: Implement entity and phase state machines.** Reset per-run counters only in prepare, clean up all owned entities and subscriptions, and make repeat preparation explicit. Require successful spawn of A/B before judging filtering. For phase removal, exercise the actual SDKHooks path with exact counts after each of these actions:

```text
subscribe PRE+POST -> invoke -> PRE=1 POST=1 original=1
remove PRE -> invoke -> PRE=0 POST=1 original=1
restore PRE, remove POST -> invoke -> PRE=1 POST=0 original=1
restore both, PRE self-unsubscribes -> invoke twice -> first both, second POST only
remove final phase -> invoke -> PRE=0 POST=0 original=1
```

Use a validated schedulable engine entity or a test-only driver that reaches the actual SDKHooks adapter with valid engine objects. A no-op logic entity that never Thinks is not a completed test. Do not replace the test with separate unrelated Dummy Virtual registrations.

- [ ] **Step 2: Implement delete/reuse/map/reload proof.** Persist old index+serial/identity in controller evidence; remove the entity, acquire a reused slot with a different identity using bounded attempts, invoke the new occupant, and reject any old-subscription delivery. Continue the same run across map change and JS plugin unload/reload. Store before/after callbacks and identities outside the plugin, and require a fresh subscription after reload. Bound all waits; inability to create the required condition is pending with the missing condition, not a false pass.

Include the s2script/probe native unload/reload subchecks with R2's explicit pending/retry protocol. Keep a separate peer loaded so missing restoration and stale registrations are visible.

- [ ] **Step 3: Implement voice, visibility and recipient exercises.** Prepare three-client voice allowed/denied/unmuted phases; use native effective-listen/original observations plus actual listening observations. For transmit, spawn a visible networked entity in both clients' PVS and alternate A-only/all visibility. For recipient masks, send a valid observable event to a strict client subset and then suppress it for all; collect actual outgoing recipient decisions and client receipts. Do not mutate policy and immediately declare its effect observed.

Each prepared phase emits its exact client actions, actor identities, expected observations and native prerequisites. Collect accepts evidence for that phase/run and emits typed observations for the controller; human input uses R4's file interface. `report` stays read-only. No code edit or manual fabrication of native JSON is needed to finish a successfully observed run.

- [ ] **Step 4: Test transitions and clean up.** Extend host evidence tests for each state transition, stale post-map record, missing actor, negative outcome and cleanup/reprepare. Run typechecking and build the acceptance plugin. Update `tools/khook-probe/README.md` with exact commands/client actions and capture locations for all twelve cases, including failed/pending examples and retry behavior. Return the explicit list of cases that still need live/human execution; implemented workflows are not passing evidence.

### R7: Integrate, document and close PR A acceptance

**Spec:** §§3, 8 and parent T6–T8. **Findings:** all.
**Inputs:** reviewed R1–R6 commits and evidence. **Owner:** coordinator plus the designated live operator.

- [ ] **Step 1: Integrate host regressions in CI.** Add the host lifetime, binding, observer, installer and controller self-tests to `scripts/ci-native.sh`; retain all existing gates. Confirm host fixtures use the real patched provider code. Build/test on Linux x86_64; required sanitizers failing to initialize are pending/failed evidence rather than an ASan success claim.

- [ ] **Step 2: Update compatibility docs and notices.** INSTALL/BUILDING must require the corrected source+patch identity and show reproducible build/install/rollback commands. State that PLAPI 18 or a GetDetourInterface string alone is insufficient. The runtime identity includes both original SHAs and the patch-set digest. Update deterministic notice generation to identify the altered host and include its required notices; do not claim an unchanged upstream artifact. Keep existing license freshness checks.

Append completion history only after completed gates. Add a short amendment link in the parent migration docs explaining that this spec replaces their unsafe provider-lifetime/install assumptions. Keep all unfinished parent T7/T8 evidence pending.

- [ ] **Step 3: Run full CI and server builds.** Stage R1's corrected host, the shim/core built by `scripts/build-sniper.sh`, and the optimized probe. Build the SDK/JS acceptance fixture. Run:

```bash
make ci
bash scripts/test-khook-host-lifetime.sh
bash scripts/test-khook-observer.sh
python3 scripts/verify-metamod-artifact.py \
  --tree build/metamod-pinned/tree --manifest build/metamod-pinned/metamod-build.json
```

Do not repeatedly run unchanged gates without cause; reference the same successful results when `make ci` already ran them. Re-run affected gates after integration fixes. Record exact source/build/host manifest identities for deployment.

- [ ] **Step 4: Execute live acceptance.** Stop the owned CS2 server, transactionally install the corrected host, stage the matching s2script and test plugins, then restart without `--force-recreate`. Record `meta version`, `meta list` and artifact identities. Run R4's prepare/collect/judge workflow, required stateful actions and real-client observations. Repeat with the opposite probe/s2script load order. Exercise ordinary command-origin unload, callback-requested unload with safe retry, and process shutdown with delayed work; no crash, hang, stale callback or permanently resident plugin is acceptable.

- [ ] **Step 5: Review the final evidence and update PR A.** Report F1–F5 -> implementing commits -> regression logs -> live subchecks. A reviewer checks this mapping and the final diff. Mark ready only when full CI, corrected host verification and every required suite A subcheck pass on the final code. If clients/server are unavailable, hand off the implemented fixes, exact commands and remaining pending subchecks, keep PR #221 draft, and keep PR B blocked.

## Self-review and coverage

| Requirement | Work package |
|-------------|--------------|
| Provider survives original/POST/removal/static destruction; no stale-id hang | R1, R2, R7 |
| No shim/core shutdown while active; retry completes; rejected Function retarget stays owned | R2 |
| Corrected host is reproducibly built and its artifact identity verified | R1, R3, R7 |
| Consumed game-event pointer never read by observer | R5 |
| Five permanent-pending cases have real collection/completion paths | R4, R6 |
| Independent s2script delivery/original proof and negative controls | R5, R7 |
| Scoped dynamic worker assignments, shared-file ownership and integrated gates | Dispatch graph; every R package |
| Human evidence is run-bound and never invented | R4, R6, R7 |

All steps are intentionally unchecked. The documentation handoff does not implement the host patch, new scripts, internal shutdown query or fixture protocols. Workers must supply those deliverables and their evidence.

## Handoff prompt

> Implement the KHook PR A remediation spec and plan in `docs/superpowers/specs/2026-09-14-khook-pr-a-remediation-design.md` and `docs/superpowers/plans/2026-09-14-khook-pr-a-remediation.md`. Start from PR #221 (`cursor/khook-sourcehook-cutover-8628`, reviewed at `57a329b7`) and bring in the documentation handoff commit. Use a dynamic workflow with the specified package dependencies and file ownership. Start R1 and R4 independently; integrate the corrected host contract before shim retirement and installer work. Resolve F1–F5, run the required host/CI checks and collect real live/client evidence where available. Keep missing required evidence pending, PR #221 draft until accepted, and PR B blocked. Do not merge or release as part of this work.
