# KHook Migration Implementation Plan

> **Proposed successor stack (2026-09-22):** Review the [shared engine bindings design](../specs/2026-09-22-engine-bindings-design.md) before dispatching more B/C work. Upon written-design approval, its S1 replaces T9–T13's old PR grouping while preserving their behavioral/evidence obligations; S2 and S3 cover function authoring and game-package extraction. New executable plans follow written-spec review. T1–T8 and current PR A remediation/acceptance remain authoritative. This notice does not mark any task complete or authorize bypassing the PR A gate.

> **For the coordinator and subagents:** Use the user's selected dynamic workflow. Dispatch bounded work packages using the dependency, ownership and evidence rules below; no particular orchestration tool or skill is required. Task/step IDs are stable and checkbox (`- [ ]`) completion requires evidence.

**Goal:** Load s2script on post–PR-223 Metamod (PLAPI 18, SourceHook deleted) and put every engine intercept we own through KHook’s one-detour-per-address catalog.

**Architecture:** Three stacked PRs. PR A pins Metamod, introduces checked typed bindings, replaces every `SH_*` site, and carries its licenses/operator floor/live-host refresh. PR B moves inline interception to checked typed KHook callbacks, preserves invocation contracts, and resolves/validates original bytes. PR C decides precache and finishes the broader documentation. JS `HookResult` and the Rust multiplexer do not change.

**Tech Stack:** Metamod:Source `master` ≥ `7e24ce9e7a03` (KHook submodule `1e200e4cc8e0`), C++17 shim, SafetyHook linked **inside Metamod** (we only include `khook.hpp`), existing `s2validate` / gamedata validators, Docker CS2 live gate.

**Spec:** `docs/superpowers/specs/2026-09-14-khook-migration-design.md`

**Revision:** 2026-09-14 source-review corrections. These are implementation requirements; no migration or live-gate completion is claimed by this documentation change.

**PR A review amendment:** Use the [current remediation plan](2026-09-15-khook-pr-a-review-fixes.md) and [spec](../specs/2026-09-14-khook-pr-a-remediation-design.md). The user requires stock Metamod and only `.s2sp` hot reload inside resident s2script. No upstream patch or native shim hot-reload gate is required. The amended stock-host lifetime, artifact and acceptance requirements apply throughout this plan, including later PR B/C cleanup. PR B remains blocked until PR A passes.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-09-14-khook-migration-design.md`
- Research: `docs/superpowers/specs/2026-09-14-khook-vs-sourcehook-research.md`
- Hard cutover: new binary needs Metamod PLAPI 18; old binary will not load on that Metamod
- `PLUGIN_SAVEVARS()` is the first line of `Load`; no `Add`/`Configure` before it
- Do not link `libkhook` or SafetyHook into `s2script.so`
- JS API / `HookResult` / multiplexer unchanged
- In-place object edits use Ignore; by-value declarative edits use Recall; CanAcquire retains its vote/engine fold and may submit a POST Override. Usercmd always uses Ignore after neutralization. FireEvent retains its explicit original call and Supersede.
- Across KHook consumers: Ignore < Override < Supersede; the first executed callback's return wins a tie. Registration order does not establish execution order. A later higher-priority action replaces the earlier result.
- Validate before registration; PR B uses verified original bytes for lookup, uniqueness and instruction checks, with live address/range checks retained. INVALID_HOOK is Failed, accepted registration is Pending until observed.
- No sentinel/dummy engine-pointer probes. Do not synchronously destroy bindings inside their callbacks or free context before physical removal completes. No game-thread polling for first fire.
- Linux x86_64 only; sniper build for anything that must load
- One PR per stack entry; each `make ci` green alone (Docker `test-gate.sh` miss is benign for docs-only commits)

**PR map (stack, bottom → top):**

| PR | Branch suffix | Delivers |
|----|---------------|----------|
| A | `khook-sourcehook-cutover` | Checked virtual bindings + Metamod pin + SourceHook sites + licenses/install prerequisites |
| B | `khook-s2detour-cutover` | Original-byte resolution + declarative invocation contracts + named inline detours |
| C | `khook-docs-precache` | Precache + remaining architecture/build documentation |

---

## File Structure

```
third_party/metamod-source/          # PR A: submodule bump (sourcehook/ gone, khook/ arrives)
shim/CMakeLists.txt                  # PR A: include khook, drop sourcehook
shim/src/s2script_mm.h               # PR A: handler signatures → KHook::Return<T>
shim/src/s2script_mm.cpp             # PR A: Virtual members, Add/Remove, handler bodies
shim/src/sdkhooks_vp.cpp             # PR A: Virtual + Configure(slot)
shim/src/sdkhooks_vp.h               # PR A: drop SH_MANUALHOOK comments
shim/src/khook_map.h                 # PR A: NEW — local action helpers, not universal Changed mapping
shim/src/khook_binding.h             # PR A: NEW — checked typed bindings + receipts/removal ownership
shim/tests/khook_binding_test.cpp    # PR A: NEW — injected-backend registration/lifetime tests
scripts/test-khook-binding.sh        # PR A: NEW — host bookkeeping tests, wired into ci-native.sh
scripts/test-khook-live.sh           # PR A/B/C: NEW — exact behavioral evidence gates
scripts/ci-native.sh                # PR A: register the host bookkeeping gate
shim/src/original_module.{h,cpp}    # PR B: NEW — verified original-byte image + live-address translation
shim/tests/original_module_test.cpp # PR B: NEW — identity, bounds, address translation fixtures
scripts/test-original-module.sh     # PR B: NEW — host gate, wired into ci-native.sh
shim/src/engine_calls.cpp           # PR B: original-byte descriptor resolution
shim/src/call_validate.{h,cpp}      # PR B: original-byte decoding with logical live addresses
shim/src/sigscan.{h,cpp}            # PR B: preserve original-image uniqueness/xref semantics
tools/khook-probe/                  # PR A/B/C: NEW — native MM peer and controlled-function fixtures
examples/khook-acceptance/          # PR A/B/C: NEW — reproducible JS engine-contract fixtures
shim/src/engine_hooks.cpp            # PR B: S2_HookInstall → Function::Configure
shim/src/s2script_mm.cpp             # PR B: DTA / HostSay / FOI / UserCmd → Member/Function
shim/src/detour.cpp                  # PR B: zero production call sites
docs/ARCHITECTURE.md                 # PR C
docs/INSTALL.md                      # PR A: first incompatible binary's operator floor
docs/BUILDING.md                     # PR A floor; PR C remaining build documentation
docs/PROGRESS.md                     # PR C: append finished-slice entry after live gate
scripts/cloud/install.sh             # PR A: durable, verified refresh of pre-KHook docker/metamod
scripts/gen-licenses.sh              # PR A: drop hardcoded 2.0.0.1403 label
licenses/licenses.txt               # PR A: regenerate for the new pin
```

Task map: **T1–T8 = PR A**. **T9–T11 = PR B**. **T12–T13 = PR C**. T1–T5 prepare code and host fixtures; T6 runs the full CI gate; T7–T8 prove PR A on post-223 Metamod. T9–T11 require original-byte, declarative, coexistence and live named-hook evidence. T12–T13 complete precache/docs. Required unavailable checks remain pending.

This file is the implementation plan. The PR that lands it is **docs only** — do not start T1 here.

---

## Dynamic subagent execution contract

The coordinator owns scheduling, integration and PR readiness. A worker owns only
the task/steps in its assignment. The numbered tasks are dependency milestones;
they are not thirteen independent jobs that can all start at once. Split a task
into smaller packages when useful, keeping the original T/Step IDs in the ledger
and preserving every requirement. Do not create a PR per worker: A, B and C remain
the three atomic slices.

### Dependencies and dispatch readiness

"Integrated" means the coordinator has applied and reviewed a worker's result on
the slice branch, and records the resulting commit as the next worker's baseline.
Do not dispatch dependent edits against another worker's uncommitted files.

| Task | Required input before dispatch | Output / completion evidence |
|------|--------------------------------|------------------------------|
| T1 | Current repo conventions and the exact pinned revisions in this plan | Pinned headers, CMake includes, generated licenses and operator floor; pin/license checks |
| T2 | T1 pin/header baseline | Checked binding API, receipt/removal ownership and handler declarations; host binding tests |
| T3 | T1–T2 integrated | Interface registration and teardown in `s2script_mm.cpp`; conversion inventory |
| T4 | T3 integrated | Handler bodies using the T2 contract; FireEvent/voice semantics ready for T8 |
| T5 | T1–T2 integrated | SDKHooks phase/filter ownership and validation; scoped host/core checks |
| T6 | T1–T5 integrated, plus any fixture code already included in the slice | Full CI and sniper artifacts; integration fixes reviewed |
| T7 | T1 selected host identity for installer work; T6 artifact before restart/load validation | Repeatable staged install/rollback, verified running PLAPI 18 and s2script load |
| T8 | T2 contract for fixture authoring; T6–T7 and integrated fixtures for acceptance | Suite A, full CI and real-client evidence on the final PR A revision |
| T9 | PR A acceptance complete | Verified original-byte provider and all resolver callers; typed declarative invocations; host tests and suite B declarative cases |
| T10 | T9 Step 1 integrated and its shared binding/retirement interfaces fixed | Named inline adapters and suite B named-hook cases |
| T11 | T9–T10 integrated | Retired production detours, safe process shutdown; full CI and suites A/B on final PR B revision |
| T12 | PR B acceptance complete | Precache decision and suite C evidence, including peer ownership |
| T13 | T12 decision and evidence; documentation may be drafted earlier on settled contracts | Accurate operator/architecture docs, license freshness and final PR C gates |

Useful concurrency within those dependencies:

- After T2, T3→T4 and T5 can run in separate worktrees. T3/T4 share
  `s2script_mm.{h,cpp}` and run sequentially. T5 owns the SDKHooks files.
- T7 installer preparation can run after T1 alongside shim work, with its own
  script/docs ownership. Deploy/restart/load validation waits for T6. T8 fixture
  authoring can run after T2; executing or approving its gate waits for integration.
- Within T9, freeze the original-image API before splitting provider/tests and
  resolver integration. Complete Step 1 before migrating its inline consumers.
  T9 Steps 2–5 and T10 can then run concurrently with disjoint production files;
  their shared fixture files need one owner or serialized integration.
- T11 and the PR acceptance gates are integration work. Do not overlap teardown
  changes, deployment or gate runs with workers changing their inputs.

PR A's pin and all SourceHook callers must land together. Intermediate worker
commits can be incomplete/nonbuildable while that conversion is in progress;
they are not mergeable slices. Workers run meaningful checks available for their
scope and report remaining checks. T6/T8 must produce a green integrated PR A;
PR B implementation waits for that acceptance. The same barrier applies between
PR B and PR C.

### Ownership and shared resources

Give every package an explicit file allowlist and an isolated worktree based on
its assigned commit. Workers do not change the shared checkout/branch, overwrite
another worker's files, or launch the entire plan recursively. A coordinator may
delegate a bounded subtask with the same ownership rules. Contract changes must
be integrated and communicated to all consumers before dependent work resumes.

The coordinator assigns a single writer for shared files: `shim/CMakeLists.txt`,
`scripts/ci-native.sh`, generated licenses, the acceptance runner/probe/plugin,
and overlapping sections of `s2script_mm.cpp` or operator docs. A worker needing
an unassigned file reports the required edit; the coordinator grants ownership or
integrates it. Separate worktrees do not make overlapping edits independent.

One designated operator owns the live CS2 server, its Metamod tree, deployment,
restart and RCON fixture execution. Record the deployed revision and server/host
identities before each suite. Other workers may prepare fixtures but must not
mutate that server or its shared build output during a gate. An unavailable
Linux/sniper environment or required human client leaves the affected evidence
pending; it does not lower the gate.

Commit snippets below are scoped checkpoints, not instructions to stage unrelated
work or create thirteen PRs. Stage explicit assigned paths, inspect the staged
diff, and report the commit ID. The coordinator integrates worker commits and
owns slice branch pushes/PR updates in the user's chosen workflow.

### Worker assignment and return format

Each assignment must be self-contained; workers should not need the coordinator's
conversation history. Include:

```text
Slice / package: PR A|B|C; Tn Step(s) ...; bounded objective
Baseline: exact repository commit, worktree and branch
Read: CLAUDE.md, applicable AGENTS.md, this plan's global constraints,
      canonical patterns, assigned steps and linked spec sections
Inputs: integrated prerequisite commits and agreed interface signatures
Ownership: allowed files/sections; coordinator for shared-file changes
Contract: required behavior, preserved ABI and explicit deferred work
Checks: exact commands/cases and expected outcomes from these steps;
        distinguish host tests, integrated CI and live/client evidence
Deliver: scoped commit(s), changed files, interface notes and test report
Escalate: missing prerequisite, conflicting ownership or contradicted contract
```

Worker return reports contain the package/step IDs, baseline and result commits,
changed files, what behavior changed, checks actually run with outcomes and log
locations, and every failed/pending check. Include interface changes, unresolved
findings and any follow-up dependency. "Implemented; awaiting integration/live
validation" is a valid status. "Complete" requires that package's evidence; a
worker completion never substitutes for the slice acceptance gate.

### Coordinator ledger and review

At implementation start, create a durable ledger in the selected workflow with
package ID, owner, dependencies, file ownership, baseline/result/integrated SHAs,
status and evidence links. Track planned, ready, running, awaiting integration,
blocked and verified separately. Record the specific missing input for blocked
work and continue independent ready packages. Update plan checkboxes only when
their named evidence exists; preserve pending live/client checks explicitly.

Review each returned package against the assigned spec/steps and its diff before
integration. A bounded reviewer receives that same baseline, scope and evidence.
Resolve contract or correctness findings before scheduling consumers. After
integration, run the applicable slice gates against the recorded revision;
re-run affected checks when later changes invalidate their evidence. PR C closes
with `make ci` and live suites A, B and C on its final code. Every PR must meet its
own full acceptance requirements before it is marked ready to merge.

---

## Canonical bind patterns (locked)

Read these before T3 / T5 / T9. The checked bindings are specified in T2 and the spec §5.6. The pinned high-level helpers return void from Add/Configure, so use the checked wrappers wherever host code needs a registration result.

**Interface `Virtual` (PR A)** — Metamod s2 sample, file-static because the header cannot see complete `eiface.h` types. Construct **after** `PLUGIN_EXPOSE` (same TU, so `&g_S2ScriptPlugin` exists). Ctor stores MFP + callbacks; `GetVtableIndex` only; no patch.

```cpp
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

struct InterfaceHooks {
    S2CheckedVirtual<ISource2Server, void, bool, bool, bool> gameFrame;
    // …one Virtual per interface hook…
    InterfaceHooks()
      : gameFrame(&ISource2Server::GameFrame, &g_S2ScriptPlugin,
                  &S2ScriptPlugin::Hook_GameFramePre,
                  &S2ScriptPlugin::Hook_GameFramePost) {}
};
static InterfaceHooks g_hk;

// Load, AFTER PLUGIN_SAVEVARS():
if (KHook::GetVtableIndex(&ISource2Server::GameFrame) < 0) {
    META_CONPRINTF("[s2script] KHOOK FAIL: GameFrame GetVtableIndex=-1\n");
} else if (g_S2ScriptPlugin.m_server) {
    const auto receipt = g_hk.gameFrame.Add(g_S2ScriptPlugin.m_server);
    g_S2ScriptPlugin.m_frameHookInstalled = receipt.Accepted(); // Pending or Active
    // Record receipt.reason on Failed; do not claim delivery before first fire.
}
```

`GameFrame` is **one** object with both callbacks, **one** `Add`. PRE-only / POST-only pass `nullptr` for the unused callback, matching `samples/s2_sample_mm/src/plugin.cpp`.

**SDKHooks `Virtual` (PR A)** — existing free-function handlers. Bind PRE/POST at file scope; set the slot later; share one object across phases.

```cpp
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(&Hook_StartTouch, &Hook_StartTouchPost);
// S2SdkhooksVpLoad, after slot derive, BEFORE any vp_add:
g_hkStartTouch.Configure(slot);
// vp_add: Add(p) on the first phase for this entity+kind
// vp_remove / drop: Remove(p) only when both PRE and POST rows for this entity+kind are gone
```

**Inline `Function` (PR B)** — typed to the real signature. Default-construct with callbacks; `Configure(addr)` only after `PLUGIN_SAVEVARS`. Never `Function<void>`. Never construct with an address at static init.

```cpp
static S2CheckedFunction<int64_t, void*, void*, void*, void*> g_hkDTA(&Hook_DTA_Pre, &Hook_DTA_Post);
const auto receipt = g_hkDTA.Configure(dtaAddr); // Load, after original-byte validators
// Failed: named degrade. Pending: wait for a valid invocation, never probe with a sentinel.
```

---

### Task 1: Pin Metamod past PR #223

**Files:**
- Modify: `.gitmodules` (unchanged URL; commit pointer moves)
- Modify: `third_party/metamod-source` (submodule)
- Modify: `shim/CMakeLists.txt`
- Modify: `scripts/gen-licenses.sh` and regenerate `licenses/licenses.txt` in PR A
- Modify: `docs/INSTALL.md`, `docs/BUILDING.md` (PLAPI 18 floor travels with the first incompatible binary)

**Interfaces:**
- Consumes: nothing.
- Produces: headers at `${MMS}/core/ISmmPlugin.h` with `PLUGIN_SAVEVARS` setting `KHook::__exported__khook`; `${MMS}/third_party/khook/include/khook.hpp`; no `${MMS}/core/sourcehook`.

- [ ] **Step 1: Record the current pin, then fetch**

```bash
git -C third_party/metamod-source rev-parse HEAD
# Expected today: 26c03fa192c3 (or the SHA licenses/licenses.txt prints)
git fetch origin  # not required for the submodule remote
git -C third_party/metamod-source fetch origin master
git -C third_party/metamod-source checkout 7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33
git -C third_party/metamod-source submodule update --init --recursive
git -C third_party/metamod-source/third_party/khook rev-parse HEAD
# Expected: 1e200e4cc8e0badcb7cf941525268d6977f6a4e6. A pinned commit cannot move; investigate a mismatch rather than accepting it.
test ! -d third_party/metamod-source/core/sourcehook
test -f third_party/metamod-source/third_party/khook/include/khook.hpp
grep -n "METAMOD_PLAPI_VERSION" third_party/metamod-source/core/ISmmPluginExt.h
# Expected: #define METAMOD_PLAPI_VERSION			18
```

If the exact commit cannot be fetched, resolve that availability issue. Selecting a different pin requires rechecking the helper internals, registration/removal semantics and action precedence described here; do not silently substitute newest master.

- [ ] **Step 2: Point CMake at KHook, not SourceHook**

In `shim/CMakeLists.txt`, replace the sourcehook include and add the khook include:

```cmake
target_include_directories(s2script PRIVATE
    src
    ${CMAKE_SOURCE_DIR}/src/sdk_stubs
    ${MMS}/core
    ${MMS}/third_party/khook/include
    ${AMTL}
    ${HL2SDK}/public
    # …keep every other existing include exactly as it is
)
```

Remove the line `${MMS}/core/sourcehook`. Do **not** add `KHOOK_EXPORT`. Do **not** add a `libkhook` `target_link_libraries` entry. `ISmmPlugin.h` includes `khook.hpp`; the wrappers call `__exported__khook`.

- [ ] **Step 3: Confirm the tree no longer compiles (expected)**

```bash
cmake -S shim -B build/shim -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++
cmake --build build/shim -j 2>&1 | tail -40
```

Expected: FAIL — `s2script_mm.cpp` still names `SH_DECL_HOOK*`, `SH_ADD_HOOK`, `RETURN_META`, `sourcehook.h`. That is the T2 starting gun. If cmake **configure** fails because `sourcehook` is missing from an include we already deleted, that is also expected.

- [ ] **Step 4: Regenerate notices and include the operator floor**

Change `scripts/gen-licenses.sh` to label the selected Metamod pin as PLAPI 18/KHook, regenerate `licenses/licenses.txt`, and commit both in A. Add the same floor to INSTALL/BUILDING: use the tested pinned build, or an explicitly verified PLAPI 18 drop. A build date alone is insufficient. Explain the paired runtime/Metamod upgrade and rollback; a pre-18 plugin cannot load on the new host, including other operators' Metamod plugins.

```bash
./scripts/gen-licenses.sh
```

The existing native gate regenerates this file and rejects any uncommitted difference. Its passing check belongs after the new generated artifacts are committed, not in PR C.

- [ ] **Step 5: Commit the atomic prerequisites**

```bash
git add .gitmodules third_party/metamod-source shim/CMakeLists.txt scripts/gen-licenses.sh \
  licenses/licenses.txt docs/INSTALL.md docs/BUILDING.md
git commit -m "chore(shim): pin Metamod past KHook merge; include khook.hpp"
```

---

### Task 2: Checked bindings, action helpers and handler signatures

**Files:**
- Create: `shim/src/khook_map.h`, `shim/src/khook_binding.h`
- Create: `shim/tests/khook_binding_test.cpp`, `scripts/test-khook-binding.sh`
- Modify: `scripts/ci-native.sh` (run the new host gate before the full build)
- Modify: `shim/src/s2script_mm.h` (every `void Hook_*` / `bool Hook_*` that is a SourceHook handler)

**Interfaces:**
- Consumes: `khook.hpp` via `ISmmPlugin.h`.
- Produces:
  ```cpp
  // shim/src/khook_map.h
  inline KHook::Return<void> S2_Ignore() { return { KHook::Action::Ignore }; }
  inline KHook::Return<void> S2_Supersede() { return { KHook::Action::Supersede }; }
  template <typename T>
  inline KHook::Return<T> S2_Ignore(T v) { return { KHook::Action::Ignore, v }; }
  template <typename T>
  inline KHook::Return<T> S2_Supersede(T v) { return { KHook::Action::Supersede, v }; }
  // Local skip decision only. By-value edits also need Recall;
  // CanAcquire uses its dedicated vote/engine return policy in T9.
  // collapsed JS HookResult: 0 Continue, 1 Changed, 2 Handled, 3 Stop
  inline KHook::Return<void> S2_FromHookResult(int r) {
      return r >= 2 ? S2_Supersede() : S2_Ignore();
  }
  template <typename T>
  inline KHook::Return<T> S2_FromHookResult(int r, T v) {
      return r >= 2 ? S2_Supersede(v) : S2_Ignore(v);
  }
  ```

- [ ] **Step 0: Implement and test the checked registration contract**

`khook_binding.h` defines these shim-only types:

```cpp
enum class S2HookState { Failed, Pending, Active, Removing, Removed };
struct S2HookReceipt {
    KHook::HookID_t id = KHook::INVALID_HOOK;
    S2HookState state = S2HookState::Failed;
    std::string reason;
    bool Accepted() const {
        return id != KHook::INVALID_HOOK &&
               (state == S2HookState::Pending || state == S2HookState::Active);
    }
};
```

`S2CheckedFunction<Ret, Args...>` inherits the pinned typed Function constructors;
its address `Configure` returns an S2HookReceipt. `S2CheckedVirtual<Class, Ret,
Args...>` inherits the typed Virtual constructors; `Configure(slot)` keeps its
existing behavior, while `Add(obj)` and `AddGlobal(holder)` return receipts.
After delegating installation, inspect the Function's protected associated id
or Virtual's protected slot-to-id map under its existing locks. Do not use
`IsActive()` as an installation check. Reject invalid slot/null object inputs
before calling the underlying helper. Undo a new this/global filter on failure.

Both classes provide `Observe` (Function: no argument; Virtual: the callback's
this pointer), marking that registration Active only on actual callback entry.
Both provide `BeginRemove`, which marks Removing and schedules
`RemoveHook(actualKHookId, true, completion, context)` once per owned id.
Retain binding/context in a retirement queue until completion marks Removed.
`Observe` and `BeginRemove` operate on persistent synchronized binding state;
a returned receipt is a snapshot, not independently mutable shared state.
Synchronize receipt updates; do not free them on the worker callback while
another invocation or caller still owns them. Drain retirement outside hook
callbacks before runtime teardown. If Unload is invoked on an active callback
stack, reject/defer it while preserving runtime/context ownership; do not wait
synchronously for that same stack to finish. Never substitute a declarative hook id for
the returned KHook id.

Use an injected `IKHook` backend to test bookkeeping; this does not prove native
trampoline behavior. Add `bash scripts/test-khook-binding.sh` to
`scripts/ci-native.sh`. Required cases:

| Backend condition | Assertion |
|-------------------|-----------|
| Setup returns INVALID_HOOK | Failed, named reason, no installed/active flag or retained filter |
| Valid id, callback not yet delivered | Pending; Accepted true; no active claim or engine probe |
| First matching callback | Same id becomes Active once |
| PRE/POST share a Virtual registration | One id; removing one phase retains the other |
| Unsubscribe in callback | This-filter disappears immediately without synchronous destruction |
| Removal callback delayed | Context stays alive until completion and outstanding invocation release |
| Idempotent Add/re-add | No duplicate ownership or removal; one completion per physical id |

Host-test sequence:

```bash
bash scripts/test-khook-binding.sh
```

First run must fail for the missing/new contract; after implementation it must
pass. T8 additionally exercises new and already-shared capsules through real
KHook. Preserve the public FFI conventions: SDKHooks VP add uses nonzero success;
declarative S2_HookInstall uses 0 success / -1 failure.

- [ ] **Step 1: Write `shim/src/khook_map.h`**

Create the file with the exact helpers above, plus:

```cpp
#pragma once
#include <khook.hpp>
#include "khook_binding.h"
```

No `using namespace`. No macros named `RETURN_META`.

- [ ] **Step 2: Change handler declarations in `s2script_mm.h`**

Replace the SourceHook handler block (`s2script_mm.h` L60–115) with KHook signatures. Every `Virtual`/`Member` callback takes the interface pointer first. Keep the existing parameter types (including `unsigned long long` for `uint64` under `META_NO_HL2SDK`).

```cpp
#include "khook_map.h"

    KHook::Return<void> Hook_GameFramePre(ISource2Server* server, bool simulating, bool first, bool last);
    KHook::Return<void> Hook_GameFramePost(ISource2Server*, bool simulating, bool first, bool last);
    KHook::Return<bool> Hook_FireEventPre(IGameEventManager2*, IGameEvent* ev, bool bDontBroadcast);
    KHook::Return<void> Hook_ClientCommand(ISource2GameClients*, CPlayerSlot slot, const CCommand& args);
    KHook::Return<void> Hook_DispatchConCommand(ICvar*, ConCommandRef cmd, const CCommandContext& ctx,
                                                const CCommand& args);
    KHook::Return<void> Hook_OnClientConnected(ISource2GameClients*, CPlayerSlot slot, const char* name,
                                               unsigned long long xuid, const char* netid, const char* addr, bool fake);
    KHook::Return<void> Hook_ClientPutInServer(ISource2GameClients*, CPlayerSlot slot, const char* name,
                                               int type, unsigned long long xuid);
    KHook::Return<void> Hook_ClientActive(ISource2GameClients*, CPlayerSlot slot, bool bLoadGame,
                                          const char* name, unsigned long long xuid);
    KHook::Return<void> Hook_ClientFullyConnect(ISource2GameClients*, CPlayerSlot slot);
    KHook::Return<void> Hook_ClientDisconnect(ISource2GameClients*, CPlayerSlot slot,
                                              ENetworkDisconnectionReason reason, const char* name,
                                              unsigned long long xuid, const char* netid);
    KHook::Return<void> Hook_ClientSettingsChanged(ISource2GameClients*, CPlayerSlot slot);
    KHook::Return<void> Hook_ClientVoice(ISource2GameClients*, CPlayerSlot slot);
    KHook::Return<bool> Hook_SetClientListening(IVEngineServer2*, CPlayerSlot receiver,
                                                CPlayerSlot sender, bool bListen);
    KHook::Return<void> Hook_StartupServer(INetworkServerService*, const GameSessionConfiguration_t& config,
                                           ISource2WorldSession* session, const char* unk);
    KHook::Return<void> Hook_CheckTransmit(ISource2GameEntities*, CCheckTransmitInfo** ppInfoList, int nInfoCount,
                                           CBitVec<16384>& unionTransmitEdicts, CBitVec<16384>& unionTransmitEdicts2,
                                           const Entity2Networkable_t** pNetworkables,
                                           const unsigned short* pEntityIndices, int nEntityIndices);
    KHook::Return<void> Hook_PostEvent(IGameEventSystem*, CSplitScreenSlot nSlot, bool bLocalOnly, int nClientCount,
                                       const unsigned long long* clients, INetworkMessageInternal* pEvent,
                                       const CNetMessage* pData, unsigned long nSize, NetChannelBufType_t bufType);
```

Forward-declare `IGameEventManager2`, `ICvar`, `IVEngineServer2`, `IGameEventSystem` at the top of the header next to the existing forwards. Do **not** put `S2CheckedVirtual<…>` members in this header yet (incomplete types / `META_NO_HL2SDK`). They land in Task 3 as file-static or `.cpp` members.

- [ ] **Step 3: Commit**

```bash
git add shim/src/khook_map.h shim/src/khook_binding.h shim/src/s2script_mm.h \
  shim/tests/khook_binding_test.cpp scripts/test-khook-binding.sh scripts/ci-native.sh
git commit -m "feat(shim): KHook handler signatures and Action helpers"
```

---

### Task 3: `KHook::Virtual` members, ctor, Add/Remove

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (drop every `SH_DECL_HOOK*`; add members + ctor)
- Modify: `shim/src/s2script_mm.h` only if you choose to store the `Virtual` objects as members — then the `.cpp` must include `eiface.h` **before** the header, or the members live as file-static in the `.cpp`.

**Interfaces:**
- Consumes: Task 2 handler signatures.
- Produces: one file-static `InterfaceHooks` with a `S2CheckedVirtual<Iface, Ret, Args…>` per interface hook (official context ctor), `Add`ed after `PLUGIN_SAVEVARS`.

- [ ] **Step 1: Delete every `SH_DECL_HOOK*` at the top of `s2script_mm.cpp` (L93–L170)**

Those macros will not exist. Do not leave them commented.

- [ ] **Step 2: Declare one `InterfaceHooks` struct in `s2script_mm.cpp` immediately after `PLUGIN_EXPOSE` (today L175)**

`S2ScriptPlugin g_S2ScriptPlugin;` is L174. The struct must be **after** that object so the context ctor can take `&g_S2ScriptPlugin`. Same-TU initialization order is declaration order.

Use the official context ctor (MFP + `&g_S2ScriptPlugin` + member callbacks). Same integer aliases as the deleted `SH_DECL_*` lines (`uint64` vs `unsigned long long`) so the MFP matches `eiface.h`.

```cpp
struct InterfaceHooks {
    S2CheckedVirtual<ISource2Server, void, bool, bool, bool> gameFrame;
    S2CheckedVirtual<IGameEventManager2, bool, IGameEvent*, bool> fireEvent;
    S2CheckedVirtual<IGameEventSystem, void, CSplitScreenSlot, bool, int, const uint64*,
                   INetworkMessageInternal*, const CNetMessage*, unsigned long, NetChannelBufType_t> postEvent;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, const CCommand&> clientCommand;
    S2CheckedVirtual<ICvar, void, ConCommandRef, const CCommandContext&, const CCommand&> dispatchConCommand;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, const char*, uint64, const char*, const char*, bool> onClientConnected;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, const char*, int, uint64> clientPutInServer;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, bool, const char*, uint64> clientActive;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot> clientFullyConnect;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot, ENetworkDisconnectionReason, const char*, uint64, const char*> clientDisconnect;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot> clientSettingsChanged;
    S2CheckedVirtual<ISource2GameClients, void, CPlayerSlot> clientVoice;
    S2CheckedVirtual<ISource2GameEntities, void, CCheckTransmitInfo**, int, CBitVec<16384>&, CBitVec<16384>&,
                   const Entity2Networkable_t**, const unsigned short*, int> checkTransmit;
    S2CheckedVirtual<IVEngineServer2, bool, CPlayerSlot, CPlayerSlot, bool> setClientListening;
    S2CheckedVirtual<INetworkServerService, void, const GameSessionConfiguration_t&, ISource2WorldSession*, const char*> startupServer;

    InterfaceHooks()
      : gameFrame(&ISource2Server::GameFrame, &g_S2ScriptPlugin,
                  &S2ScriptPlugin::Hook_GameFramePre, &S2ScriptPlugin::Hook_GameFramePost),
        fireEvent(&IGameEventManager2::FireEvent, &g_S2ScriptPlugin,
                  &S2ScriptPlugin::Hook_FireEventPre, nullptr),
        postEvent(&IGameEventSystem::PostEventAbstract, &g_S2ScriptPlugin,
                  &S2ScriptPlugin::Hook_PostEvent, nullptr),
        clientCommand(&ISource2GameClients::ClientCommand, &g_S2ScriptPlugin,
                      &S2ScriptPlugin::Hook_ClientCommand, nullptr),
        dispatchConCommand(&ICvar::DispatchConCommand, &g_S2ScriptPlugin,
                           &S2ScriptPlugin::Hook_DispatchConCommand, nullptr),
        onClientConnected(&ISource2GameClients::OnClientConnected, &g_S2ScriptPlugin,
                          &S2ScriptPlugin::Hook_OnClientConnected, nullptr),
        clientPutInServer(&ISource2GameClients::ClientPutInServer, &g_S2ScriptPlugin,
                          &S2ScriptPlugin::Hook_ClientPutInServer, nullptr),
        clientActive(&ISource2GameClients::ClientActive, &g_S2ScriptPlugin,
                     &S2ScriptPlugin::Hook_ClientActive, nullptr),
        clientFullyConnect(&ISource2GameClients::ClientFullyConnect, &g_S2ScriptPlugin,
                           &S2ScriptPlugin::Hook_ClientFullyConnect, nullptr),
        clientDisconnect(&ISource2GameClients::ClientDisconnect, &g_S2ScriptPlugin,
                         &S2ScriptPlugin::Hook_ClientDisconnect, nullptr),
        clientSettingsChanged(&ISource2GameClients::ClientSettingsChanged, &g_S2ScriptPlugin,
                              &S2ScriptPlugin::Hook_ClientSettingsChanged, nullptr),
        clientVoice(&ISource2GameClients::ClientVoice, &g_S2ScriptPlugin,
                    nullptr, &S2ScriptPlugin::Hook_ClientVoice),
        checkTransmit(&ISource2GameEntities::CheckTransmit, &g_S2ScriptPlugin,
                      nullptr, &S2ScriptPlugin::Hook_CheckTransmit),
        setClientListening(&IVEngineServer2::SetClientListening, &g_S2ScriptPlugin,
                           &S2ScriptPlugin::Hook_SetClientListening, nullptr),
        startupServer(&INetworkServerService::StartupServer, &g_S2ScriptPlugin,
                      nullptr, &S2ScriptPlugin::Hook_StartupServer) {}
};
static InterfaceHooks g_hk;
```

Keep the empty complete `class GameSessionConfiguration_t {};` immediately above this struct (old `SH_DECL` sizeof comment). After construction, if `KHook::GetVtableIndex` for a method is `-1`, print `[s2script] KHOOK FAIL: <name> GetVtableIndex=-1` in `Load` and skip that `Add`.

- [ ] **Step 3: Do not call `AddContext` / `Configure(mfp)` for these**

The ctor already bound callbacks and the vtable index. `PLUGIN_SAVEVARS()` must still be the first line of `Load` so later `Add` can talk to Metamod. `PLUGIN_EXPOSE` already defines `__exported__khook`.

- [ ] **Step 4: Replace every `SH_ADD_HOOK` / `SH_REMOVE_HOOK` pair**

| Old | New |
|-----|-----|
| `SH_ADD_HOOK(ISource2Server, GameFrame, m_server, … Pre)` + POST add | `g_hk.gameFrame.Add(m_server)` (one Add) |
| matching `SH_REMOVE_HOOK` ×2 | `g_hk.gameFrame.Remove(m_server)` |
| `SH_ADD_HOOK(IGameEventManager2, FireEvent, …)` | `g_hk.fireEvent.Add(s_pGameEventManager)` |
| `SH_ADD_HOOK(IGameEventSystem, PostEventAbstract, …)` | `g_hk.postEvent.Add(s_pGameEventSystem)` |
| `SH_ADD_HOOK(ISource2GameClients, ClientCommand, …)` | `g_hk.clientCommand.Add(m_gameClients)` |
| six lifecycle `SH_ADD_HOOK`s | `g_hk.onClientConnected.Add(m_gameClients)` … `g_hk.clientSettingsChanged.Add(m_gameClients)` |
| `ClientVoice` POST | `g_hk.clientVoice.Add(m_gameClients)` |
| `CheckTransmit` POST | `g_hk.checkTransmit.Add(m_gameEntities)` |
| `DispatchConCommand` | `g_hk.dispatchConCommand.Add(s_pCvar)` |
| `SetClientListening` | `g_hk.setClientListening.Add(s_pEngine)` |
| `StartupServer` POST | `g_hk.startupServer.Add(<same pointer as today's SH_ADD_HOOK>)` |

`s2_request_hook("OnGameFrame", enable)` becomes:

```cpp
if (enable && !g_S2ScriptPlugin.m_frameHookInstalled && g_S2ScriptPlugin.m_server) {
    const auto receipt = g_hk.gameFrame.Add(g_S2ScriptPlugin.m_server);
    g_S2ScriptPlugin.m_frameHookInstalled = receipt.Accepted();
} else if (!enable && g_S2ScriptPlugin.m_frameHookInstalled) {
    g_hk.gameFrame.Remove(g_S2ScriptPlugin.m_server);
    g_S2ScriptPlugin.m_frameHookInstalled = false;
}
```

Use the same receipt handling for `"GameEvent"` → `g_hk.fireEvent` and every row above. The existing installed booleans mean accepted registration (Pending or Active); record delivery readiness separately. Named failures leave the corresponding boolean false. Every real handler marks its binding observed on entry, using the callback's actual interface pointer. CheckTransmit and other first-fire-validated capabilities remain unavailable until their validator has passed. Last unsubscribe removes the this-filter; physical destruction is deferred until the binding can retire safely.

- [ ] **Step 5: Commit**

```bash
git add shim/src/s2script_mm.cpp shim/src/s2script_mm.h
git commit -m "feat(shim): bind interface hooks through KHook::Virtual"
```

---

### Task 4: Rewrite handler bodies (including FireEvent and voice Recall)

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (every `Hook_*` definition that still uses `RETURN_META*`)

**Interfaces:**
- Consumes: Task 2 signatures, Task 3 `Virtual` objects.
- Produces: handlers that return `KHook::Return<T>` and never mention `META_RES`.

- [ ] **Step 1: Mechanical notify hooks**

For every handler that today ends in `RETURN_META(MRES_IGNORED)` and does not call `SH_CALL` / `NEWPARAMS`, add the unused `this` pointer as the first parameter, replace the return with `return S2_Ignore();`. Example:

```cpp
KHook::Return<void> S2ScriptPlugin::Hook_GameFramePre(ISource2Server* server, bool simulating, bool first, bool last) {
    g_hk.gameFrame.Observe(server);
    // Existing dispatch body follows unchanged.
    return S2_Ignore();
}
```

Apply to: `Hook_GameFramePre`, `Hook_GameFramePost`, `Hook_OnClientConnected`, `Hook_ClientPutInServer`, `Hook_ClientActive`, `Hook_ClientFullyConnect`, `Hook_ClientDisconnect`, `Hook_ClientSettingsChanged`, `Hook_ClientVoice`, `Hook_StartupServer`, `Hook_CheckTransmit` (keep the in-place bitvec mutation; still `Ignore`).

- [ ] **Step 2: Collapse-to-Supersede hooks**

`Hook_ClientCommand`, `Hook_DispatchConCommand`, `Hook_PostEvent` already branch on a core `int` ≥ 2. Replace:

```cpp
if (result >= 2) RETURN_META(MRES_SUPERCEDE);
RETURN_META(MRES_IGNORED);
```

with:

```cpp
return S2_FromHookResult(result);
```

Keep every early `RETURN_META(MRES_IGNORED)` on a miss path as `return S2_Ignore();`.

- [ ] **Step 3: `Hook_FireEventPre` — CallOriginal + Supersede**

Replace L5406–5418. Keep the `s_legacyFilterActive` arming comments and the mask logic.

```cpp
KHook::Return<bool> S2ScriptPlugin::Hook_FireEventPre(IGameEventManager2* mgr, IGameEvent* ev,
                                                     [[maybe_unused]] bool bDontBroadcast) {
    g_hk.fireEvent.Observe(mgr);
    if (!ev) return S2_Ignore(true);
    IGameEvent* prev = s_currentEvent;
    s_currentEvent = ev;
    int suppress = s2script_core_dispatch_game_event_pre(ev->GetName());
    s_currentEvent = prev;
    if (suppress) {
        uint64_t allow = 0;
        const bool haveMask = s2script_core_take_event_recipients(&allow) != 0;
        s_legacyFilterActive = haveMask;
        s_legacyAllowMask = allow;
        bool ret = KHook::CallOriginal(&IGameEventManager2::FireEvent, mgr, ev, haveMask ? false : true);
        s_legacyFilterActive = false;
        s_legacyAllowMask = 0;
        return S2_Supersede(ret);
    }
    return S2_Ignore(true);
}
```

If `CallOriginal` on a member pointer fails to compile, use `g_hk.fireEvent.CallOriginal(mgr, ev, haveMask ? false : true)` instead.

- [ ] **Step 4: `Hook_SetClientListening` — Recall, not Supersede**

Replace L5663–5668:

```cpp
        if (deny) {
            KHook::Recall(&IVEngineServer2::SetClientListening,
                          KHook::Return<bool>{ KHook::Action::Ignore, bListen },
                          engine, receiver, sender, false);
            return S2_Ignore(bListen);
        }
    }
    return S2_Ignore(bListen);
}
```

The first parameter is `IVEngineServer2* engine` (the KHook `this`). Pass that pointer into `Recall`, not `s_pEngine`, so a future engine pointer mismatch cannot rewrite the wrong object.

- [ ] **Step 5: Grep the shim for leftovers**

```bash
rg -n "SH_|RETURN_META|MRES_|g_SHPtr|sourcehook" shim/src --glob '!**/third_party/**'
```

Expected: matches only in comments you are about to delete, or in `sdkhooks_vp.cpp` (Task 5). Zero `SH_ADD_HOOK` / `RETURN_META` in `s2script_mm.cpp`.

- [ ] **Step 6: Commit**

```bash
git add shim/src/s2script_mm.cpp
git commit -m "feat(shim): return KHook::Return from interface handlers"
```

---

### Task 5: SDKHooks VP — `Virtual::Configure(slot)` + `Add(entity)`

**Files:**
- Modify: `shim/src/sdkhooks_vp.cpp`
- Modify: `shim/src/sdkhooks_vp.h` (comments)
- Modify: `shim/src/call_validate.h`, `call_validate.cpp` if needed to inject original virtual-target lookup

**Interfaces:**
- Consumes: existing `s2_sdkhook_vp_add` / `s2_sdkhook_vp_remove` FFI (signatures unchanged).
- Produces: same FFI, implemented with `S2CheckedVirtual<CEntityInstance, …>`.

- [ ] **Step 1: Replace `SH_DECL_MANUALHOOK*` (L50–63) with Virtual objects**

```cpp
#include <khook.hpp>
#include "khook_map.h"

namespace {
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch;
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkTouch;
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkEndTouch;
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkBlocked;
S2CheckedVirtual<CEntityInstance, void> g_hkSpawn;
S2CheckedVirtual<CEntityInstance, void> g_hkThink;
S2CheckedVirtual<CEntityInstance, void> g_hkPreThink;
S2CheckedVirtual<CEntityInstance, void> g_hkPostThink;
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*, CEntityInstance*, int, float> g_hkUse;
S2CheckedVirtual<CEntityInstance, int> g_hkGetMaxHealth;
S2CheckedVirtual<CEntityInstance, bool, int, int> g_hkShouldCollide;
S2CheckedVirtual<CEntityInstance, void> g_hkVPhysicsUpdate;
S2CheckedVirtual<CEntityInstance, void> g_hkGroundEntChanged;
S2CheckedVirtual<CEntityInstance, bool> g_hkCanBeAutobalanced;
}
```

- [ ] **Step 2: Bind PRE/POST on the file-static objects (no `AddContext`)**

Keep the existing `Hook_StartTouch` / `Hook_StartTouchPost` free functions. Change their signatures to take `CEntityInstance* thisPtr` first and return `KHook::Return<void>` / `KHook::Return<int>` / `KHook::Return<bool>`. Replace `RETURN_META(MRES_SUPERCEDE)` with `return S2_Supersede();` and `RETURN_META(MRES_IGNORED)` with `return S2_Ignore();`. For `GetMaxHealth` / `ShouldCollide` / `CanBeAutobalanced`, replace `RETURN_META_VALUE` with `S2_Supersede(v)` / `S2_Ignore(v)`.

Construct each object with those free functions (index still invalid):

```cpp
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(&Hook_StartTouch, &Hook_StartTouchPost);
```

Where a handler today calls `SH_MCALL` (`GetMaxHealth` L302, `ShouldCollide` L315, `CanBeAutobalanced` L328):

```cpp
int orig = g_hkGetMaxHealth.CallOriginal(thisPtr);
```

- [ ] **Step 3: `SH_MANUALHOOK_RECONFIGURE` → `Configure(slot)` once, before any `Add`**

In `Reconfigure` (today L381–398), replace each `SH_MANUALHOOK_RECONFIGURE` with `g_hkStartTouch.Configure(slot);` (and the matching object). `S2SdkhooksVpLoad` already runs at Load, before JS can `vp_add`. Do not reconstruct the `Virtual` or reconfigure its slot after registration. Resolve slot comparisons and vtable-member validation through `KHook::FindOriginalVirtual(vtable, index)` before executable-range/equality checks, so a peer's JIT entry does not terminate the RTTI walk. Keep slot bounds checks. Inject the lookup into the engine-free validator, with unhooked-slot behavior preserved in its tests. T9 later also updates this resolver's signature/prologue byte source.

- [ ] **Step 4: `SH_ADD_MANUALHOOK` → `Add`; `SH_REMOVE_HOOK_ID` → `Remove` only when both phases are gone**

Core never stores the SourceHook id (`sdkhooks.rs` `vp_add` is `f(...) != 0`). FFI stays `1`/`0`. Drop the shim `hook_id` field or leave it unused.

`g_installed` is still keyed `{p, kind, post}` because core calls `vp_add`/`vp_remove` once per phase. One `Virtual` serves both phases:

```cpp
static S2HookReceipt VpAddThis(Kind kind, void* p) {
    switch (kind) {
    case Kind::StartTouch: return g_hkStartTouch.Add(static_cast<CEntityInstance*>(p));
    case Kind::Touch: return g_hkTouch.Add(static_cast<CEntityInstance*>(p));
    case Kind::EndTouch: return g_hkEndTouch.Add(static_cast<CEntityInstance*>(p));
    case Kind::Blocked: return g_hkBlocked.Add(static_cast<CEntityInstance*>(p));
    case Kind::Spawn: return g_hkSpawn.Add(static_cast<CEntityInstance*>(p));
    case Kind::Think: return g_hkThink.Add(static_cast<CEntityInstance*>(p));
    case Kind::PreThink: return g_hkPreThink.Add(static_cast<CEntityInstance*>(p));
    case Kind::PostThink: return g_hkPostThink.Add(static_cast<CEntityInstance*>(p));
    case Kind::Use: return g_hkUse.Add(static_cast<CEntityInstance*>(p));
    case Kind::GetMaxHealth: return g_hkGetMaxHealth.Add(static_cast<CEntityInstance*>(p));
    case Kind::ShouldCollide: return g_hkShouldCollide.Add(static_cast<CEntityInstance*>(p));
    case Kind::VPhysicsUpdate: return g_hkVPhysicsUpdate.Add(static_cast<CEntityInstance*>(p));
    case Kind::GroundEntChanged: return g_hkGroundEntChanged.Add(static_cast<CEntityInstance*>(p));
    case Kind::CanBeAutobalanced: return g_hkCanBeAutobalanced.Add(static_cast<CEntityInstance*>(p));
    default: return {KHook::INVALID_HOOK, S2HookState::Failed, "unknown SDKHook kind"};
    }
}
static void VpRemoveThis(Kind kind, void* p) {
    switch (kind) {
    case Kind::StartTouch: g_hkStartTouch.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::Touch: g_hkTouch.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::EndTouch: g_hkEndTouch.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::Blocked: g_hkBlocked.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::Spawn: g_hkSpawn.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::Think: g_hkThink.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::PreThink: g_hkPreThink.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::PostThink: g_hkPostThink.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::Use: g_hkUse.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::GetMaxHealth: g_hkGetMaxHealth.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::ShouldCollide: g_hkShouldCollide.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::VPhysicsUpdate: g_hkVPhysicsUpdate.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::GroundEntChanged: g_hkGroundEntChanged.Remove(static_cast<CEntityInstance*>(p)); break;
    case Kind::CanBeAutobalanced: g_hkCanBeAutobalanced.Remove(static_cast<CEntityInstance*>(p)); break;
    default: break;
    }
}

static bool OtherPhaseLive(void* p, Kind kind, int post) {
    VpKey other{ p, kind, post ? 0 : 1 };
    return g_installed.find(other) != g_installed.end();
}
```

Change `VpAddThis` to return the receipt from the selected checked binding. In `s2_sdkhook_vp_add`, call it only for the first row of an entity+kind; insert/refcount the new row only when registration is accepted. Failure returns 0 and a named diagnostic, with no orphan filter. Each matched PRE/POST handler calls the binding's Observe(thisPtr) before dispatch. Reuse the accepted registration for the other phase. In `s2_sdkhook_vp_remove` / `s2_sdkhook_vp_drop` / `S2SdkhooksVpUnload`: call `VpRemoveThis` only when the last remaining row for that `{p, kind}` (either phase) is erased. If PRE unsubscribes while POST is still live, do **not** `Remove`.

Update the comment at `core/src/sdkhooks.rs` L4–5 from `SH_ADD_MANUALHOOK` to `KHook::Virtual::Add`.

- [ ] **Step 5: Shim unit tests that mention SourceHook**

```bash
rg -n "SH_|SourceHook" shim/tests
```

Update comments and extend the binding fixtures for the shim's phase/filter contract. T8 must prove one-entity-only delivery, both unsubscribe orders, in-callback removal, entity deletion/index reuse and map-change cleanup through real KHook. We are testing our integration, not replacing KHook's own tests.

- [ ] **Step 6: Commit**

```bash
git add shim/src/sdkhooks_vp.cpp shim/src/sdkhooks_vp.h core/src/sdkhooks.rs
git commit -m "feat(shim): SDKHooks VP via KHook::Virtual"
```

---

### Task 6: Compile the shim (PR A)

**Files:** coordinator-assigned integration fixes from T1–T5; inspect failures and
assign exact file ownership before dispatching a fix worker.

**Interfaces:**
- Produces: `build/shim/s2script.so` (host) and a sniper `dist/addons/s2script/bin/linuxsteamrt64/s2script.so`.

- [ ] **Step 1: Host compile**

```bash
cmake -S shim -B build/shim -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++
cmake --build build/shim -j
```

Expected: PASS. Fix every error in this task; do not proceed with a red shim. Common failures:

- `GetVtableIndex` / MFP size: a `uint64` vs `unsigned long long` mismatch on a lifecycle hook — make the `Virtual<…>` args identical to `eiface.h`.
- Incomplete `GameSessionConfiguration_t`: keep the empty complete type immediately above the `Virtual` declaration (the old `SH_DECL` comment at `s2script_mm.h` L27–32 still applies).
- `PLUGIN_GLOBALVARS()` now exports `__exported__khook`, not `g_SHPtr`. `sdkhooks_vp.cpp` already has `PLUGIN_GLOBALVARS();` — that is now correct.

- [ ] **Step 2: Full required gates (unchanged public contract)**

```bash
make ci
```

Expected: PASS, including binding fixtures and license freshness, with core tests single-threaded via `.cargo/config.toml`. Do not replace the full gate with compilation and core tests alone. Any unavailable Docker/live requirement is recorded separately as pending.

- [ ] **Step 3: Sniper build**

```bash
sudo docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```

Expected: `dist/addons/s2script/bin/linuxsteamrt64/s2script.so` exists and `ldd` does not name `libkhook` or `libsafetyhook`.

- [ ] **Step 4: Commit any compile fixes**

```bash
# Stage only the reviewed integration-fix paths, then inspect the staged diff.
git diff --cached --check
git commit -m "fix(shim): compile clean against KHook / PLAPI 18"
```

---

### Task 7: Live-gate Metamod binary (PR A)

**Files:**
- Modify: `scripts/cloud/install.sh` (durable verified refresh lands in A)
- Modify: `docs/INSTALL.md`, `docs/BUILDING.md` if the tested build/install commands require refinement
- Operator tree: `docker/metamod/` (gitignored)

**Interfaces:**
- Produces: `docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so` built from the pinned submodule **or** a post-223 `mmsdrop` tarball.

- [ ] **Step 1: Detect whether drop is new enough**

```bash
strings docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so | rg -n "GetDetourInterface|SourceHook version|KHook" | head
```

Treat strings output only as a heuristic; stripped binaries can omit symbol names. The authoritative check is the loaded host's `meta version` plugin interface 18 plus a successful checked test-plugin registration. Compare the installed artifact with the selected pin/drop identity before deciding to refresh.

- [ ] **Step 2: Replace `docker/metamod`**

Build Metamod from the pinned submodule in the sniper/AMBuild environment, or stage a verified PLAPI 18 drop. Record its exact artifact identity. Make this path repeatable in `scripts/cloud/install.sh`; validate staged files before replacing the on-disk tree. For a drop download:

```bash
latest="$(curl -fsSL https://mms.alliedmods.net/mmsdrop/2.0/mmsource-latest-linux)"
curl -fsSL "https://mms.alliedmods.net/mmsdrop/2.0/${latest}" -o /tmp/mms.tar.gz
rm -rf /tmp/mms && mkdir -p /tmp/mms && tar xzf /tmp/mms.tar.gz -C /tmp/mms
test -f /tmp/mms/addons/metamod/bin/linuxsteamrt64/metamod.2.cs2.so
# install.sh verifies the staged build identity, preserves the previous tree for
# rollback, then replaces docker/metamod while CS2 is stopped.
test -f docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so
```

Keep `docker/s2script.vdf` in place (`scripts/package-addon.sh` copies it). If the staged drop cannot be verified as PLAPI 18, use the pinned source build; do not repeatedly download an incompatible latest drop. Test install.sh with a pre-18 tree, a current tree, an absent tree and a failed/stale download; failure must preserve the previous installation.

- [ ] **Step 3: Restart CS2 (not `--force-recreate`)**

```bash
sudo docker compose -f docker/docker-compose.yml restart cs2
sudo docker logs s2script-cs2 2>&1 | rg -n "s2script|KHOOK|SourceHook|PLAPI|Active" | tail -40
python3 scripts/rcon.py "meta version"
python3 scripts/rcon.py "meta list"
```

Expected: `meta version` prints plugin interface **18**; no “SourceHook version” line (commented out upstream). `meta list` lists `s2script`. Logs show `[plugins] '@s2script/...' Active` and no `Plugin uses old SourceHook Metamod build`.

- [ ] **Step 4: Commit only script changes** (not `docker/metamod/`)

```bash
git add scripts/cloud/install.sh
git commit -m "chore(cloud): refresh Metamod tree when GetDetourInterface is missing"
```

If you did not change the script in this task, skip the commit.

---

### Task 8: PR A behavioral acceptance

**Files:**
- Create: `tools/khook-probe/CMakeLists.txt`, `plugin.cpp`, `README.md`
- Create: `examples/khook-acceptance/package.json`, `src/plugin.ts`
- Create: `scripts/test-khook-live.sh`
- Evidence: exact commands, fixture output and any pending cases in the PR body.

The native probe is a separate Metamod test plugin compiled against the same pin.
It hooks controlled native functions with valid objects and acts as a second
KHook consumer. It is never included in the production release. Its command
`s2_khook_probe run A` emits one JSON record per case with case name, expected,
actual and pass/fail. The shell runner sends it through existing RCON, rejects
missing/duplicate case records, and exits nonzero on any required failure.
The JS fixture uses the existing public APIs; it records exact engine outcomes.

- [ ] **Step 1: Build the fixtures and assert real KHook behavior**

Provide a CMake `S2_SOURCE_DIR` option in the probe project pointing at the repo;
reuse the shim's SDK/Metamod include and SteamRT build conventions. Build in the
sniper container, install its test VDF, and build the JS fixture with the SDK.

```bash
cmake -S tools/khook-probe -B build/khook-probe -DS2_SOURCE_DIR="$PWD"
cmake --build build/khook-probe -j
(cd examples/khook-acceptance && node ../../packages/sdk/dist/cli.js build)
bash scripts/test-khook-live.sh A
```

The fixture CMake/CLI commands are run after the SDK build and inside the
sniper environment where required. The runner must assert:

| Case | Required result |
|------|-----------------|
| New-capsule and shared-capsule registration | Failed/Pending/Active receipts accurate; wait for actual valid fixture delivery, not a sentinel |
| Peer Ignore / Override / Supersede, both registration orders | Higher action wins; equal action retains the first executed callback's return; record actual order; all PRE callbacks run |
| One normal fixture invocation | Exactly one engine-original call and expected PRE/POST counts |
| Frame/client/command hooks | Actual counter increments, client lifecycle delivery and command suppression/continuation |
| FireEvent no suppression | Original exactly once, normal broadcast |
| FireEvent Handled and explicit recipient mask | Original exactly once with expected broadcast flag; intended recipients only |
| Voice Recall | Denied listen bit reaches original as false; original once; unmuted case unchanged |
| SDKHooks two entities, only one subscribed | Only selected live entity dispatches |
| SDKHooks PRE then POST removal, reverse removal, removal inside callback | Remaining phase survives; no deadlock or stale callback |
| Entity deletion/reused slot, map change and `.s2sp` teardown | No stale entity delivery; owner rows/filters are removed and shared native bindings remain resident |
| CheckTransmit | First-fire layout validation and intended recipient filtering verified |

Use real clients to observe recipient and voice behavior where required.
If unavailable, mark those cases **pending**; a first-fire log does not prove
the mute/recipient semantics. Do not approve PR A until its required matrix and
full CI are green. Compilation and an rg match never replace a behavioral check.

- [ ] **Step 2: Run the full gate on PR A alone**

```bash
make ci
bash scripts/test-khook-live.sh A
```

Record Metamod version/pin, server build, fixture revision and output. PR B does
not start until this gate is complete.

- [ ] **Step 3: Commit fixtures and open/update PR A**

Title: `shim: cut SourceHook over to KHook (PLAPI 18)`.
Include why, pin/build identity, operator upgrade/rollback, license freshness,
converted inventory and completed behavioral evidence. No production release
contains the native probe or JS acceptance plugin.

---

### Task 9: PR B — original bytes and declarative invocation contracts

**Files:**
- Modify: `shim/src/engine_hooks.cpp`, `engine_hooks.h`
- Modify: `shim/src/engine_calls.cpp`, `call_validate.h`, `call_validate.cpp`
- Modify: `shim/src/s2script_mm.cpp` (`ResolveSigValidated`)
- Modify: `shim/src/sdkhooks_vp.cpp` (`S2SdkhooksVpLoad` original-byte signature/validation source)
- Modify: `shim/src/sigscan.h`, `sigscan.cpp` (original-image inputs)
- Create: `shim/src/original_module.h`, `original_module.cpp`
- Create: `shim/tests/original_module_test.cpp`, `scripts/test-original-module.sh`
- Modify: `shim/CMakeLists.txt`, `scripts/ci-native.sh`
- Extend: `tools/khook-probe/plugin.cpp`, `examples/khook-acceptance/src/plugin.ts`,
  `scripts/test-khook-live.sh` (suite B)

**Interfaces:**
- Preserve `S2_HookInstall(hookId, shape, addr, reason, cap)`: **0 success,
  -1 named failure**, including idempotent success.
- Preserve the five shape ids, ArgView liveness gate, copied HUD strings,
  controller receiver, opaque argument widths and bypass arm/disarm ABI.
- Produce one checked Function binding per declarative id and the original-byte
  view used by both named and declarative resolution/validation.

- [ ] **Step 1: Add original-byte resolution before migrating inline sites**

Implement the spec §5.7 read-only original module image. Its inputs are the
live module mapping/identity; its output is verified original bytes indexed by
logical module-relative address, plus explicit conversion to the live address.
Keep the backing image alive for every validator read. Verify the ELF backing
the loaded mapping, its build id/identity, and PT_LOAD/file-offset mapping.
Do not trust a file merely because its pathname matches a loaded library.

Update these consumers together:

1. `ResolveSigValidated`, `S2_EngineCallResolve` and `S2SdkhooksVpLoad`: count matches and locate the
   match in the same original-byte image.
2. Direct, lea-disp, ctor-body-xref and validated-call target derivation:
   calculate relative destinations using logical engine addresses; do not use
   the copied buffer's address as a relocation base.
3. `s2validate` prologue/arg-width/xref reads: use bounded original bytes;
   live executable-range and live object/vtable validation remain separate.
4. RTTI/vtable target equivalence: query KHook's original virtual target when
   the live slot is patched; do not compare the JIT entry to the engine body.
5. Preserve per-descriptor named failures when original image identity,
   instruction range or uniqueness cannot be established.

`LookupSignature` alone returns a first match and does not supply all the
bytes our validators need. Do not combine it with live patched-byte counting.
Do not apply s2detour-only relocation restrictions to SafetyHook.

Add the host original-module test script to ci-native and drive fixtures for
matching/mismatched backing identity, nonzero load bias, PT_LOAD bounds,
relative address translation, duplicate matches, and an original image whose
live prologue has been replaced. Every bounds/identity mismatch must fail by
name; the patched-live case must still resolve the original target.

```bash
bash scripts/test-original-module.sh
bash scripts/test-sigscan.sh
bash scripts/test-call-validate.sh
```

Run the new fixture failing before implementing its contract, then green.
T11's real peer-plugin test verifies actual KHook patches in both load orders.

- [ ] **Step 2: Keep one typed binding per id and record each invocation**

Use one `S2CheckedFunction` for each installed id, typed to its shape. The
existing compile-time per-id thunk tables can become callback tables; keep the
id baked into the callback. Preserve these signatures:

| Shape | Function signature |
|-------|--------------------|
| this_void | void(void*) |
| this_f32_i32_i32_i32 | void(void*, float, int32_t, int32_t, int32_t) |
| this_f32_i32_i64_i64 | void(void*, float, int32_t, int64_t, int64_t) |
| this_i64_i32_i64 | int32_t(void*, int64_t, int32_t, int64_t) |
| this_i64_i64_i64 | void(void*, int64_t, int64_t, int64_t) |

An invocation is a stack-local record; per-id heads link records without storing
them in a reallocating container. The scope lives until synchronous Recall returns:

```cpp
struct HookInvocation {
    ArgView view;
    HookInvocation* previous = nullptr;
    const void* savedActive = nullptr;
    bool bypass = false;
    bool voted = false;
    int32_t pluginResult = 0;
};
static thread_local HookInvocation* g_invocations[S2_HOOK_MAX] = {};

struct InvocationScope {
    int id;
    HookInvocation& call;
    InvocationScope(int id_, HookInvocation& call_) : id(id_), call(call_) {
        call.previous = g_invocations[id];
        call.savedActive = g_activeView;
        g_invocations[id] = &call;
        g_activeView = &call.view;
    }
    ~InvocationScope() {
        g_activeView = call.savedActive;
        g_invocations[id] = call.previous;
    }
};
```

Only validated compile-time ids reach this scope. Mark the checked binding
observed on callback entry. Keep all accessors' `g_activeView` checks; a
completed or retained view must fail access. Observe is diagnostic registration
state, not an entity-liveness check.

- [ ] **Step 3: Forward changed values with synchronous Recall**

For every PRE: fill the shape's existing ArgView, create InvocationScope,
consume the bypass flag once, and dispatch only when not bypassed. Read the
arguments back from that view and Recall the target with them. For the wide
float/int shape, use this callback body (Id is compile-time):

```cpp
HookInvocation call;
call.view.hookId = Id;
call.view.shape = S2_HOOK_SHAPE_THIS_F32_I32_I64_I64;
call.view.self = self;
call.view.f[0] = delay;
call.view.i[0] = reason;
call.view.q[0] = opaque0;
call.view.q[1] = opaque1;
InvocationScope scope(Id, call);
call.bypass = S2Hook_BypassTake(Id);
const int result = call.bypass ? 0 : S2Hook_Dispatch(Id, &call.view);
using Fn = void (*)(void*, float, int32_t, int64_t, int64_t);
auto fn = reinterpret_cast<Fn>(static_cast<uintptr_t>(g_hooks[Id].addr));
return KHook::Recall(fn, S2_FromHookResult(result), self,
                     call.view.f[0], call.view.i[0],
                     call.view.q[0], call.view.q[1]);
```

Repeat the explicit argument mapping for each signature above. Preserve every
opaque argument at full width. Use Recall for Continue, Changed, Supersede and
bypass paths so any POST needing the record runs before scope exit.
`Recall(...).ret` is the supplied decision, not the effective engine return.
Never call `g_orig*` as well as Recall.

POST obtains `g_invocations[Id]`. No record is a named integration failure,
not permission to dereference a dead view. On bypass, return Ignore without JS
dispatch. For shapes exposing actual POST, publish the arguments passed to POST
and actual skipped state while retaining the saved vote. Shapes without a
public POST do not gain a new JS event.

The HUD-click shape retains its copied text and controller receiver. Dispatch
its existing completion notification once in PRE before Recall, as the current
thunk does; do not dispatch it again from KHook POST. Published event meaning
does not follow merely from a backend phase name.

- [ ] **Step 4: Implement CanAcquire's separate return policy**

Retain `v.i[2]` (voted) and `v.i[1]` (result) after PRE dispatch. Continue has
no vote. Changed keeps the engine enabled. Handled/Stop supplies Supersede with
the voted result or implicit InvalidItem (1). Keep this decision fixed while
Recall executes the remaining chain.

In the int-returning POST, after locating its live record:

```cpp
if (call.bypass) return S2_Ignore(int32_t{0});
const bool skipped = KHook::WasOriginalFunctionSkipped();
if (!skipped) {
    const int32_t engine = KHook::GetOriginalReturn<int32_t>();
    const int32_t folded = call.voted
        ? S2Hook_MostRestrictiveAcquire(call.pluginResult, engine) : engine;
    if (call.voted && folded != engine) {
        KHook::ManualReturn(KHook::Return<int32_t>{
            KHook::Action::Override, folded});
    }
}
call.view.i[1] = KHook::GetCurrentReturn<int32_t>();
S2Hook_DispatchPost(Id, &call.view, skipped ? 1 : 0);
return S2_Ignore(int32_t{0}); // ManualReturn already submitted any local change
```

Here `call` references the per-id record; set its voted/pluginResult fields in
PRE after dispatch. Never read GetOriginalReturn when skipped. Never claim
Override without a vote changing the engine result: an unnecessary Override
would block a later equal-priority peer result.

JS POST sees the effective result at its place in the chain. Equal/higher-priority
peer decisions can beat our proposal, and subsequent peer POST callbacks can
change the eventual process return. Preserve our local most-restrictive fold
without claiming exclusive ownership of the process result.

- [ ] **Step 5: Install and retire checked per-id bindings**

Validate id, shape, original bytes and live range before registration. Preserve
same-id/same-target idempotence and refusal of two local descriptor ids on one
address. Peer plugins can share the address through KHook.

Failed receipt: return -1 with the named reason. Accepted receipt: store the
typed binding and actual KHook id, return 0, report Pending until Observe.
Keep bypass pairing intact when an outbound call does not reach the hook.

`S2_HookResetAll` begins removal using each binding's actual KHook ids and
retains its context through completion. Do not immediately clear/free it after
RemoveHook(..., true). Do not destroy any live ArgView scope.

- [ ] **Step 6: Execute declarative and peer cases**

Extend suite B with these assertions:

| Input/case | Expected outcome |
|------------|------------------|
| this_void Continue / Handled | Original count 1 / 0; only supported notifications fire |
| Changed float/int params | Both mutable shapes forward edits to original |
| High-bit opaque arguments | Original receives identical full-width values |
| CanAcquire Changed-deny + engine-Allow | Denied; original once; POST sees denial |
| CanAcquire plugin-Allow + engine-deny | Denied; original once; POST sees denial |
| CanAcquire Handled without result | InvalidItem; original zero; skipped true |
| CanAcquire no vote plus peer Override | No unnecessary local Override blocks peer |
| Peer Supersede before/after our PRE | All PRE run; actual skipped/current result observed |
| Nested same-id/different-id calls | Each POST sees own view; outer restored; completed view rejected |
| Bypass hit / outbound early return | Hit bypasses our dispatch; next genuine invocation delivered |
| HUD click allowed/handled | Receiver/text preserved; completion once before original; suppression preserved |
| Peer loaded first/second | Both resolve; one original invocation; correct precedence |
| Delayed callback retirement / `.s2sp` unload | Callback context retained until removal and invocation completion; shared native bindings remain resident |

```bash
bash scripts/test-khook-live.sh B
```

First prove this_void, then mutable float/int and CanAcquire before breadth.
A low-level SetupHook replacement is permitted only after reproducing a typed
helper limitation in the fixture and preserving this same contract. It cannot
bypass these acceptance checks.

- [ ] **Step 7: Commit the declarative work**

Commit the original-byte provider with its resolver/validator callers atomically,
then the per-id callbacks with their fixtures. PR B also requires T10's named
sites and T11's full gates.

---

### Task 10: PR B — named inline adapters and safe damage callbacks

**Files:**
- Modify: `shim/src/s2script_mm.cpp`
- Extend: `tools/khook-probe/plugin.cpp`, `examples/khook-acceptance/src/plugin.ts`,
  `scripts/test-khook-live.sh`

**Interfaces:**
- All sites use the **same checked bindings and teardown owner** as declarative
  hooks. There is no separate damage detour engine or installer.
- Preserve the damage callback/view API through an explicit typed adapter.
  Its dedicated dispatch/view implementation is existing architectural debt;
  moving it fully onto descriptors is a follow-up, not a completed part of this
  mechanical backend cutover.

- [ ] **Step 1: Register the named signatures through checked Function bindings**

Construct with callbacks only, then register after original-byte validation:

```cpp
static S2CheckedFunction<int64_t, void*, void*, void*, void*> g_hkDTA(&Hook_DTA_Pre, &Hook_DTA_Post);
static S2CheckedFunction<void, void*, void*, bool, int, const char*> g_hkHostSay(&Hook_HostSay_Pre, nullptr);
static S2CheckedFunction<void, CEntityIOOutput*, CEntityInstance*, CEntityInstance*, const CVariant*, float, void*, char*>
    g_hkFOI(&Hook_FOI_Pre, nullptr);
static S2CheckedFunction<int, void*, void*, int, bool, float> g_hkUsercmd(&Hook_Usercmd_Pre, nullptr);
```

Record every receipt. Failed degrades by name; Pending is accepted but unproven
delivery. Lazy Usercmd stays idempotent. HostSay supersedes suppressed chat;
FOI supersedes result >= 2; Usercmd always returns Ignore after any in-place
neutralization. No handler calls an old g_orig pointer.

- [ ] **Step 2: Scope both damage pointers around each callback**

Bind separately during PRE and POST instead of leaving globals armed across
the engine/peer chain:

```cpp
struct DamageDispatchScope {
    void* previousInfo;
    void* previousVictim;
    DamageDispatchScope(void* victim, void* info)
        : previousInfo(s_currentDamageInfo), previousVictim(s_currentDamageVictim) {
        s_currentDamageInfo = info;
        s_currentDamageVictim = victim;
    }
    ~DamageDispatchScope() {
        s_currentDamageInfo = previousInfo;
        s_currentDamageVictim = previousVictim;
    }
};

static KHook::Return<int64_t> Hook_DTA_Pre(
    void* victim, void* info, void*, void*) {
    g_hkDTA.Observe();
    DamageDispatchScope scope(victim, info);
    s2script_core_dispatch_damage();
    return S2_Ignore(int64_t{0});
}
static KHook::Return<int64_t> Hook_DTA_Post(
    void* victim, void* info, void*, void*) {
    DamageDispatchScope scope(victim, info);
    s2script_core_dispatch_damage_post();
    return S2_Ignore(int64_t{0});
}
```

Forward-declare callbacks before the binding and define them after it. Keep
the existing entity-book checks inside the adapter. Ignore(0) does not replace
the original return. Nested callbacks restore enclosing info/victim; each POST
uses the arguments delivered to that phase. Peer Supersede still permits POST;
record the actual original-skip diagnostic without inventing a new JS result.

- [ ] **Step 3: Remove the invalid-pointer engine probe**

Delete `kDtaSelfTest`, its sentinel PRE branch and the install-time call to
dtaAddr with dummy arguments. Do not retain raw offset-probing diagnostics as
proof of a typed callback. A valid KHook id cannot make sentinel pointers safe
for peer PRE callbacks.

Use the probe's controlled function/valid object to assert installation failure,
pending/shared registration, nesting, return preservation and actual diversion.
Separately drive real engine damage on a valid target and assert PRE/POST and
victim selection. Periodic core `S2_DAMAGE_SELFTEST` is core-liveness evidence
only; it does not satisfy the engine-detour gate.

- [ ] **Step 4: Verify the named engine paths**

```bash
bash scripts/test-khook-live.sh B
```

Require valid damage delivery, nested-state fixture assertions, chat and output
suppression/continuation, and Usercmd neutralization while original processing
still runs. Missing human-client input stays pending.

- [ ] **Step 5: Record the damage follow-up boundary**

The migration unifies installation and lifetime. A follow-up slice should extend
the generic inbound-hook vocabulary with borrowed typed views and move damage
onto descriptors, preserving writable PRE/read-only POST, nested state and
victim liveness. Game-specific field/class mappings belong in the game package
and gamedata; core supplies generic lifetime and dispatch.

Apply the same rule to future features: ammo reads/writes use schema accessors;
intercepting ammo consumption uses shared hook infrastructure with a typed
descriptor/view. Do not copy the old damage-specific installer pattern.

- [ ] **Step 6: Commit named adapters and fixtures**

```bash
git add shim/src/s2script_mm.cpp tools/khook-probe examples/khook-acceptance scripts/test-khook-live.sh
git commit -m "feat(shim): migrate named inline adapters to checked KHook bindings"
```

---

### Task 11: PR B — retire production `s2detour`

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (`s2detour::RemoveAll()` in the process-shutdown cleanup established by PR A)
- Modify: `shim/CMakeLists.txt` only if `detour.cpp` is no longer linked into `s2script` (keep it for `detour_reloc_test`)

**Interfaces:**
- Produces: checked binding retirement, completion-aware removal and `S2_HookResetAll` without freed callback contexts.

- [ ] **Step 1: Process shutdown**

Remove `s2detour::RemoveAll()` once T9–T10 own every inline intercept. Extend PR A's validated process-shutdown boundary to retire the new checked bindings before freeing their callback contexts. Preserve ordinary native-unload refusal without mutation and keep shared native hooks resident across `.s2sp` reload. Stock Metamod forces plugin removal during shutdown and does not retry a pending `Unload` response; no host-managed retry or native library unmapping may be assumed. Never synchronously remove the capsule currently executing. Test real script reload and clean process shutdown on stock Metamod, and assert zero production `s2detour::Install` sites.

- [ ] **Step 2: Host + sniper + core tests**

```bash
make ci
bash scripts/test-khook-live.sh A
bash scripts/test-khook-live.sh B
```

- [ ] **Step 3: Commit + PR B**

```bash
# Stage only the reviewed retirement changes and their test/build callers.
git diff --cached --check
git commit -m "chore(shim): drop production s2detour::Install"
```

Title: `shim: put inline detours through KHook`.
Body: Why (issue #215 / we were the private detour). List the four named sites + declarative `S2_HookInstall`. State that the relocator remains only in test targets, if retained.

---

### Task 12: PR C — precache

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (`WriteVtableSlot` / `InstallPrecacheHook`)
- Extend: `tools/khook-probe`, `examples/khook-acceptance`, `scripts/test-khook-live.sh` (suite C)

**Interfaces:**
- Consumes: existing RTTI vtable + gamedata index.
- Produces: `KHook::Virtual` on that class, or a named leftover.

- [ ] **Step 1: `Virtual` on the RTTI vtable + gamedata index (slot rewrite, not prologue)**

Prefer the checked Virtual slot binding because it matches the current slot interception. The old relocator's refusal alone does not prove SafetyHook cannot relocate that function. Keep the selected slot approach and test its coexistence rather than switching mechanisms without evidence.

```cpp
struct PrecacheObj {};   // never name CGameRulesGameSystem
static KHook::Return<void> Hook_Precache_Pre(PrecacheObj* self, void* pManifest) {
    auto* previous = s_currentPrecacheManifest;
    s_currentPrecacheManifest = pManifest;
    s2script_core_dispatch_precache();
    s_currentPrecacheManifest = previous;
    return S2_Ignore();   // original still runs
}
static S2CheckedVirtual<PrecacheObj, void, void*> g_hkPrecache(&Hook_Precache_Pre, nullptr);

// InstallPrecacheHook, after RTTI + .text check:
g_hkPrecache.Configure(s_precacheVtblIdx);
struct { void** vptr; } holder{ vt };
const auto receipt = g_hkPrecache.AddGlobal(reinterpret_cast<PrecacheObj*>(&holder));
// Record receipt; retain vt for later removal, never the temporary holder pointer.
```

`AddGlobal` filters by vtable. Mark observed on real callback entry. On unload, construct a fresh holder with the retained vtable for RemoveGlobal, then retire physical registration via its actual KHook id. Delete WriteVtableSlot if the slot path passes suite C.

Pending first fire is not failure. If a proven failure requires fallback, drain the KHook registration and establish that no peer owns the slot before any private rewrite. Otherwise degrade precache by name without touching the peer trampoline.

- [ ] **Step 2: Live**

Run `bash scripts/test-khook-live.sh C`. Assert resource paths precache on a real map change, original precache once, nested manifest restoration, removal/reload, and a peer owning the same slot. A failed or unavailable case stays pending; a segfault is a failed gate. Apply fallback only under Step 1's ownership/removal requirements.

- [ ] **Step 3: Commit**

```bash
git add shim/src/s2script_mm.cpp
git commit -m "feat(shim): precache slot hook via KHook (or named leftover)"
```

---

### Task 13: PR C — finalize architecture and precache documentation

**Files:**
- Modify: `docs/ARCHITECTURE.md` §1 (issue #215 paragraph)
- Modify: `docs/INSTALL.md` only for final precache/coexistence notes (floor already in A)
- Modify: `docs/BUILDING.md` (final build notes; minimum host already in A)
- Modify: `docs/PROGRESS.md` (append the finished-slice entry **after** the live gate, not before)
- Verify: `scripts/cloud/install.sh` (durable verified refresh already in A)
- Verify: `scripts/gen-licenses.sh` (PLAPI 18 label already in A)
- Verify: `licenses/licenses.txt` (regenerated/committed with the pin in A)
- Modify: `CLAUDE.md` / `AGENTS.md` Current state / live-gate Metamod note if they still say 1403

**Interfaces:**
- Produces: operators know they need Metamod ≥ PR #223; architecture thesis names two composition layers.

- [ ] **Step 1: `ARCHITECTURE.md` §1**

Replace the sentence that says frameworks “each ship their own detour engine” and that s2script is the sole arbiter of the detour. New text must say:

- Metamod/KHook owns the one process-wide trampoline (PR #223 / issue #215).
- s2script owns the JS `HookResult` contract among `.s2sp` plugins.
- The shim is a KHook consumer, not a second detour library.
- Higher action wins; the first executed callback's return wins ties. Peer POST can change the eventual return.
- Typed damage dispatch is a transitional adapter using shared installation/lifetime; a descriptor/borrowed-view follow-up removes the remaining dedicated path. Future ammo events do not get private installers.

- [ ] **Step 2: `INSTALL.md` / `BUILDING.md`**

```markdown
Metamod:Source with verified plugin API 18 (KHook), using the tested pinned
build or an identified compatible drop. Upgrade Metamod and s2script together;
rollback both together. A build date or filename alone is not a compatibility check.
```

- [ ] **Step 3: `install.sh`**

Re-run A's staged refresh/rollback checks and verify loaded PLAPI 18. Preserve a valid installation on failed download/validation; strings output is only a heuristic. No new mandatory installer work is deferred to C.

- [ ] **Step 4: Licenses**

```bash
./scripts/gen-licenses.sh
./scripts/check-licenses-generated.sh
```

Expected: clean artifacts matching the new pin and PLAPI 18 label already committed in A.

- [ ] **Step 5: Final gates, then commit + PR C**

The coordinator runs these on the integrated PR C code, with the designated
operator controlling the live server. Record the revision and all required
evidence; pending client cases prevent declaring the slice complete.

```bash
make ci
bash scripts/test-khook-live.sh A
bash scripts/test-khook-live.sh B
bash scripts/test-khook-live.sh C
```

```bash
git add docs/ARCHITECTURE.md docs/INSTALL.md docs/BUILDING.md docs/PROGRESS.md \
        scripts/cloud/install.sh scripts/gen-licenses.sh licenses/licenses.txt
git commit -m "docs: KHook cutover — Metamod floor, two-layer composition"
```

Title: `shim: finish KHook precache and architecture`.
Body: Why. Link A + B. Precache leftover yes/no.

---

## Self-review and execution status

| Requirement | Implementing task |
|-------------|-------------------|
| Observable accepted/failed/pending registration, safe teardown | T2, T3, T5, T9, T11 |
| No sentinel engine-pointer probe | T10 |
| Mutable by-value arguments and per-invocation lifetime | T9 Steps 2–3 |
| CanAcquire vote/engine fold and current POST result | T9 Step 4 |
| Nested damage info/victim restoration | T10 Step 2 |
| Original-byte lookup, uniqueness and validation | T9 Step 1 |
| License/operator prerequisites in first incompatible PR | T1, T7 |
| Real behavioral gates; unavailable checks pending | T8, T9, T10, T12 |
| Correct process action precedence | T2 fixture, T8, T9, T13 |
| Shared infrastructure; damage adapter/declarative follow-up boundary | T10 Step 5, T13 |
| Dynamic dispatch, dependency barriers, scoped ownership and worker evidence | Dynamic subagent execution contract; all tasks |

All steps remain unchecked until implementation supplies evidence. This document
does not certify the adapter, original-module view or live behavior as proven.

Before handing off implementation, verify no contradictory sentinel probe,
unconditional Changed mapping, immediate context free, or deferred PR A
prerequisite survives in the spec/research companion. For the documentation
revision itself, inspect the three-file diff and run `git diff --check`;
native/live gates belong to the implementation PRs.

## Execution handoff

Use the dynamic execution contract above to dispatch T1, then T2 on its pinned
baseline. After T2, dispatch the independent interface, SDKHooks and fixture
packages with explicit ownership; prepare the host as T7 permits. Integrate and
complete PR A (T1–T8) before PR B (T9–T11), then PR C (T12–T13). Each PR is
independently mergeable on its parent after its own full CI and behavioral
acceptance. Missing required live/client evidence remains pending rather than
silently reducing the gate. This documentation commit does not start that work.
