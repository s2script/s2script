# Shared Engine Bindings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The user has selected execution in this session with varied-model subagents; do not ask for another execution choice.

**Goal:** Resolve all engine targets through one checked service and move the remaining owned intercepts to stock Metamod KHook.

**Architecture:** Preserve the existing script APIs and five legacy shape policies while replacing duplicated resolution and private interception. Verified original instruction images carry logical live addresses into recipe-specific validation. Checked KHook bindings retain per-invocation state and follow the resident-shim lifecycle.

**Tech Stack:** Linux x86_64 SysV, C++17, Rust/V8, existing JSONC gamedata, official Metamod PLAPI 18, pinned KHook `1e200e4cc8e0badcb7cf941525268d6977f6a4e6`, sniper release toolchain.

**Spec:** [Shared engine bindings design](../specs/2026-09-22-engine-bindings-design.md). Read the [old invocation contracts](../specs/2026-09-14-khook-migration-design.md) and [stock-host amendment](../specs/2026-09-15-khook-stock-host-decision.md) as well. This plan replaces the old T9–T13 execution grouping, not its behavioral evidence obligations.

## Global Constraints

- Operators use the official AlliedModders Metamod distribution providing the required PLAPI/KHook API. No patched Metamod or KHook, private upstream fork, shadow provider, or separately linked detour engine is required.
- Only `.s2sp` hot reload is required. The native s2script module stays resident; native upgrades happen on process restart.
- No raw pointer survives a synchronous invocation in plugin code. Entities use generation-checked handles; borrowed native views expire at callback return and cannot cross `await` or be retained for later calls.
- S1 preserves the five legacy shape IDs, existing JS APIs, HUD compatibility timing, acquisition fold and named-hook behavior. S2 owns the new runtime ABI adapter and authoring format.
- Precache must use stock KHook. A remaining private slot patch is not S1 completion.
- Preserve candidate-stage versus target-stage validators. `validated-call` requires one validated call site before following E8; never replay its caller-relative validator against the callee.
- Pending registration is not observed delivery. Missing required client or runtime evidence remains pending. The known shutdown-only SIGSEGV/139 is non-blocking by explicit user direction; no extra quit tests or shutdown investigation. Preserve incidental child status truthfully, separately from the Docker wrapper status.
- Native and JavaScript CI run at each integrated slice. Ship deployable binaries only through the sniper release path, with all default plugins.
- Do not alter the user's dirty main checkout, Nebula control checkout, operator data/configs, or unrelated HUDlab container. One coordinator-designated operator owns all server mutations.

## Baseline, prerequisite and branch discipline

Source baseline is PR #221 `b4acdd5e7b64eb82e26db8fdd7018b683e10dd2d`.
Design stack is #222 → #223 → #224 above #221; each implementation belongs to its
corresponding slice. Keep later branch bases aligned by ordinary merges while
preserving remote history, unless the user separately requests a history rewrite.
Named-file staging only. Worker commits are not independently mergeable releases.

The 2026-09-22 comparison reproduces shutdown-only 139 with s2script enabled and
disabled on official Metamod 1467 and 1469 with MAM 1.6. This does not establish
fault attribution or a clean engine exit. The user explicitly made that known
symptom non-blocking and directed us to stop extra quit tests and shutdown
investigation. S1-0 preserves the recorded failure without turning it into a pass.
Isolated development uses the verified baseline; startup, normal plugin use,
map transitions, script reload, peer coexistence and required client observations
remain release gates. Callback lifetime and removal safety tests remain required.
Later runtime findings can still require rework.

## File and dependency map

| Work package | Creates/owns | Depends on |
|--------------|-------------|------------|
| S1-0 prerequisite evidence | this plan's ignored ledger, host/control evidence | existing PR A receipts |
| S1-1 original image | `shim/src/original_module.{h,cpp}`, its host test/script | S1-0 source/host baseline recorded |
| S1-2 shared resolver | `engine_resolver.{h,cpp}`, scanner/validator adapters and tests | S1-1 |
| S1-3 consumer integration | `engine_calls.cpp`, `sdkhooks_vp.cpp`, resolver section of `s2script_mm.cpp` | S1-2 |
| S1-4 declarative KHook | `engine_hooks.{h,cpp}`, invocation fixtures | S1-3 |
| S1-5 named hooks/precache | named sites in `s2script_mm.{h,cpp}`, focused fixtures | S1-3, checked invocation contract from S1-4 |
| S1-6 acceptance inventory | probe/controller suites B/C, retirement, docs and CI | S1-4 and S1-5 |

Only S1-4 and S1-5 may have concurrent production writers, after their common
contracts are integrated. A single writer owns `s2script_mm.cpp`. The coordinator
owns CMake, CI wiring, acceptance registry/probe files and cross-cutting docs;
workers request those edits rather than racing. Use a stronger worker for image
identity, KHook invocation and lifecycle changes, and lighter workers for fixture
execution or mechanical documentation. Each worker gets an exact baseline, spec,
task, file allowlist and covering checks, and returns a commit plus a test report.

### Task 0: S1-0 — Re-establish the prerequisite evidence

**Files:** Read PR A remediation plan and `.gate/remote-khook-pr221/` receipts.
Create this plan's own `.superpowers/sdd/2026-09-22-engine-bindings/progress.md` and
task-specific reports. No source behavior changes in this package.

**Interfaces:** Produces an explicit prerequisite verdict with source/artifact/host
identities, passing cases, pending cases and failures. Later tasks consume that
verdict; no worker interprets a previous summary as a fresh runtime pass.

- [ ] Read and validate the four-cell comparison summary, fixed hashes, per-cell native lists and child exits. Confirm it tested MM+MAM without s2script, not vanilla CS2.
- [ ] Run actual SSH and read-only identity checks; record the source checkout separately from installed artifact identity. Do not build out of an older dirty remote checkout.

```bash
ssh -o BatchMode=yes -o ConnectTimeout=15 ghirakawa@nebula.gkh.dev 'hostname; docker ps --format "{{.Names}} {{.Status}}"'
gh pr view 221 --json headRefOid,baseRefName,state,isDraft
```

- [ ] Record the user's non-blocking disposition for the known shutdown-only 139. Do not repeat the shutdown comparison, add quit tests, or require clean process exit. Investigate any new crash during startup or normal plugin use using controlled inputs and preserved evidence.
- [ ] Inventory outstanding A map/slot-reuse/script-reload, both peer load orders and human witnesses for client-command, voice and recipient/visibility. Assign their execution to the S1-6 release gate; do not synthesize observations or silently drop the cases.
- [ ] Record existing judge results and source/host identities. A missing mandatory input blocks release readiness, not isolated implementation after this baseline is recorded. Run the judge again at S1-6 against fresh capture.

```bash
bash scripts/test-khook-live.sh A --judge --run-dir "$S1_A_RUN_DIR" --observations "$S1_A_OBSERVATIONS"
```

`S1_A_RUN_DIR` and `S1_A_OBSERVATIONS` are operator-provided paths to genuine
captured evidence, never generated pass files. Expected release prerequisite:
all required runtime A cases pass. Whole-process terminal-only observations are
diagnostic under the explicit user disposition; keep missing or failed evidence
truthful without requiring another quit or manufacturing a passing result.

### Task 1: S1-1 — Verified original instruction images

**Create:** `shim/src/original_module.h`, `original_module.cpp`,
`shim/tests/original_module_test.cpp`, `scripts/test-original-module.sh`.
**Modify:** `shim/CMakeLists.txt`, `scripts/ci-native.sh` via coordinator.

**Interfaces:** The image is immutable, owns its backing bytes, and addresses reads
by logical engine addresses. Separate pure image construction from Linux loaded
module discovery so host tests do not require a game process.

```cpp
namespace s2original {
struct Identity {
    uint64_t device = 0, inode = 0;
    uintptr_t load_bias = 0;
    std::string build_id;
};
struct Segment {
    uintptr_t live_begin = 0;
    std::vector<uint8_t> bytes;
};
class Image {
public:
    const Identity& identity() const noexcept;
    const std::vector<Segment>& executable_segments() const noexcept;
    bool read(uintptr_t live, void* out, size_t length) const noexcept;
    bool executable(uintptr_t live, size_t length = 1) const noexcept;
};
// The production loader validates mapping/file identity and returns owned bytes.
std::shared_ptr<const Image> OpenLoadedModule(
    const char* module, std::string& reason);
// Fixture builder runs the same PT_LOAD/identity validation with supplied inputs.
struct Mapping { uintptr_t begin, end; uint64_t file_offset, device, inode; };
std::shared_ptr<const Image> FromElf(
    const std::vector<uint8_t>& file, const Identity& expected,
    const Identity& observed, const std::vector<Mapping>& mappings,
    std::string& reason);
}
```

- [ ] Add meaningful identity/bounds tests before implementation. Construct a minimal ELF fixture with one nonzero-bias executable PT_LOAD; its original bytes differ from a separately simulated patched live buffer. Assert reads use the owned original image, not the patched buffer.

```cpp
auto image = s2original::FromElf(fixture.file, fixture.identity,
                               fixture.identity, fixture.mappings, reason);
assert(image);
uint8_t opcode = 0;
assert(image->read(fixture.function_live_address, &opcode, 1));
assert(opcode == 0x55);
assert(!image->read(UINTPTR_MAX - 1, &opcode, 4));
auto wrong = fixture.identity;
++wrong.inode;
assert(!s2original::FromElf(fixture.file, fixture.identity, wrong,
                          fixture.mappings, reason));
assert(reason.find("identity") != std::string::npos);
```

`ElfFixture` in the test builds a complete ELF64 header/program-header table,
declared build-id note, PT_LOAD offsets and backing bytes; test code must not mock
the identity comparison itself. Cases include truncated headers/tables, arithmetic
overflow, wrong build-id/device/inode, nonzero file offsets, multiple executable
segments, gaps, file-size versus memory-size bounds and writes to the live copy.

- [ ] Run `bash scripts/test-original-module.sh`; initially it must fail because the provider is absent or rejects/accepts the wrong fixture, then implement the pure provider until the cases pass.
- [ ] Implement Linux discovery using loaded mappings plus `fstat` of the opened backing file and ELF build-id/PT_LOAD correspondence. Require exact mapping identity, support nonzero load bias, and reject missing/replaced/unverifiable backing files by name. Read no arbitrary pointer on an identity failure. Original code bytes come from verified executable file ranges; unsupported text relocations fail explicitly.
- [ ] Keep data reads distinct: relocated vtable/data pointers and live mapped strings remain range-checked live facts. Instruction decoding uses original bytes and logical live addresses. Never relocate a copied buffer's address as though it were an engine PC.
- [ ] Add a Linux loaded-module fixture using a tiny test shared library; replace its pathname after loading and prove the mapping identity prevents using the wrong new file. Run the test in the Linux build environment and retain output.
- [ ] Wire the host test before dependent resolver changes, run it with sanitizers when available, and commit only the six named paths. Report platform-skipped Linux discovery tests honestly.

### Task 2: S1-2 — One recipe-aware resolver and validator byte source

**Create:** `shim/src/engine_resolver.h`, `engine_resolver.cpp`,
`shim/tests/engine_resolver_test.cpp`, `scripts/test-engine-resolver.sh`.
**Modify:** `shim/src/sigscan.{h,cpp}`, `call_validate.{h,cpp}` and their tests.

**Interfaces:** Preserve current C ABI in `engine_calls.h`; this is the internal
C++ service used underneath it. Freeze these types before assigning consumers:

```cpp
namespace s2resolve {
enum class Kind { Signature, Virtual };
enum class TargetUse { Executable, MappedAddress };
struct TargetRecipe {
    Kind kind = Kind::Signature;
    TargetUse use = TargetUse::Executable;
    std::string module, pattern, strategy = "direct";
    std::string class_name, validate_json;
    int vtable_index = -1;
};
struct Resolution {
    uintptr_t address = 0;
    std::shared_ptr<const s2original::Image> image;
    std::string recipe, validation_receipt;
};
bool Resolve(const TargetRecipe& recipe, Resolution& out, std::string& reason);
}
```

The source unit's pure recipe evaluator accepts injected image, bounded live-data
reads and vtable/original-virtual callbacks. Production `Resolve` obtains them from
the module provider and Metamod; tests supply actual fixture images and a tiny
vtable. Do not expose KHook itself or game classes in the pure evaluator.

`GameEventManager` (`ctor-body-xref`) and `IGameSystem_InitAllSystems_pFirst`
(`lea-disp`) currently produce data addresses, including writable/BSS storage.
Their instruction operands still come from the verified original image, but their
derived addresses require bounded live mapped-data validation. `MappedAddress`
is an explicit internal caller choice; function/call/hook consumers retain the
default `Executable` and must reject data targets. Keep this choice in receipts
and any cache key. No game-name switch belongs in the generic resolver. A
non-empty instruction validator cannot silently pass against a data target.

- [ ] Add tests demonstrating the current divergent paths: a peer-patched prologue still resolves, two raw matches with one valid call site select that site, two valid sites fail ambiguous, unknown strategy fails, and a patched virtual resolves/validates its original target.
- [ ] Prove data-address recipes resolve valid mapped data/BSS only when `MappedAddress` is explicitly requested; executable consumers reject those same addresses, and mapped gaps/out-of-range derivations fail without dereferencing them.

```cpp
// Two E8 candidates; the string-xref validator matches only the second caller.
auto result = ResolveFixture(fixture.with_two_call_sites(), "validated-call");
assert(result.ok);
assert(result.address == fixture.second_callee_live_address);
auto duplicate = ResolveFixture(fixture.with_two_valid_call_sites(), "validated-call");
assert(!duplicate.ok && duplicate.reason.find("ambiguous") != std::string::npos);
```

`ResolveFixture` constructs the production `TargetRecipe` and invokes the pure
evaluator, not a second test resolver. Include direct, `ctor-body-xref`, `lea-disp`
and `validated-call` recipes from the current accepted vocabulary.

- [ ] Extend the validator's module view with a bounded original-code read facility. Keep default live-buffer operation only for existing engine-free fixtures that explicitly supply it; production consumers supply the verified image. Translate RIP destinations using the logical live instruction address. Range-check live strings/vtables separately.
- [ ] Implement candidate enumeration, recipe-specific candidate validation, uniqueness, derivation and target validation. Preserve the existing vtable prologue requirement and closed validator vocabulary. Query KHook original-virtual before prologue/vtable-member comparisons.
- [ ] Run red then green: `bash scripts/test-engine-resolver.sh`, `bash scripts/test-sigscan.sh`, `bash scripts/test-call-validate.sh`. Add the resolver script to native CI only after its cases exercise the shipped implementation.
- [ ] Cache only immutable verified images by mapping identity, not raw pathname. If recipe results are cached, keys include all normalized recipe/validator inputs and the effective descriptor revision; otherwise leave result caching out. Per-owner diagnostic attribution is retained by consumers even when bytes are shared.
- [ ] Commit provider-compatible resolver/validator changes as one task, with an explicit interface note for S1-3 and S2. No private upstream edit is an acceptable implementation shortcut.

### Task 3: S1-3 — Route all current consumers through the service

**Modify:** `shim/src/engine_calls.cpp`, `sdkhooks_vp.cpp`,
`s2script_mm.cpp` (`ResolveSigValidated` only), and relevant tests.

**Consumes:** `s2resolve::Resolve` and current gamedata structs.
**Produces:** Existing call IDs, SDKHooks slots and built-in addresses, all with
one recipe/validation interpretation. `S2_EngineCallResolve` still returns id>=0 or
-1 with a reason; no C ABI or JS change in this task.

- [ ] Add regression coverage for current descriptors in each consumer: signature, validated-call, virtual, malformed strategy, ambiguous target and peer-patched vtable. Assert a failed recipe produces the descriptor's own name and reason.
- [ ] Replace duplicate scan/derive/validate blocks with recipe construction and shared service invocation. Preserve consumer-specific call-id caching, SDKHooks kind/phase routing and built-in degrade accounting.
- [ ] Preserve the existing two data-producing built-in recipes with explicit `MappedAddress` use selected from their resolver semantics. Calls, declarative hooks, and virtual function slots stay `Executable`; no data pointer becomes callable merely because a built-in needs address resolution.

```cpp
s2resolve::TargetRecipe recipe;
recipe.kind = s2resolve::Kind::Signature;
recipe.module = module;
recipe.pattern = pattern;
recipe.strategy = resolve;
recipe.validate_json = validateJson ? validateJson : "{}";
s2resolve::Resolution resolved;
std::string why;
if (!s2resolve::Resolve(recipe, resolved, why)) {
    // Copy why to the existing bounded reason buffer; keep this owner's identity.
    return -1;
}
```

- [ ] Retain resolution image/receipt lifetime through any validation and registration that reads it. Remove redundant module-lookup/scan helpers only after the last production caller migrates; do not remove raw scanners still used as pure utilities.
- [ ] Run resolver/validator tests, `bash scripts/test-hook-dispatch.sh`, `bash scripts/check-call-descriptors.sh`, and compile the whole shim on Linux. A grep inventory is supporting evidence, not a substitute for behavior.
- [ ] Commit the three consumer migrations together with any indispensable test changes. Freeze their interface commit before dispatching S1-4/S1-5.

### Task 4: S1-4 — Declarative hooks through checked KHook callbacks

**Modify:** `shim/src/engine_hooks.{h,cpp}`, `hook_dispatch.{h,cpp}` only if
required for invocation storage; add `shim/tests/engine_hook_invocation_test.cpp`
and `scripts/test-engine-hook-invocation.sh`. Coordinator owns shared probe files.

**Interfaces:** Preserve `S2_HookInstall(hookId, shape, addr, reason, cap)` return
contract (0 success, -1 named failure), current five shapes, accessor liveness,
per-owner bypass mapping, and reset API. S1 still refuses incompatible duplicate
local descriptor IDs; S2 adds compatible-owner fan-out.

| Shape | Native signature |
|-------|------------------|
| this_void | `void(void*)` |
| this_f32_i32_i32_i32 | `void(void*, float, int32_t, int32_t, int32_t)` |
| this_f32_i32_i64_i64 | `void(void*, float, int32_t, int64_t, int64_t)` |
| this_i64_i32_i64 | `int32_t(void*, int64_t, int32_t, int64_t)` |
| this_i64_i64_i64 | `void(void*, int64_t, int64_t, int64_t)` |

- [ ] Write invocation tests before conversion: nested same/different IDs, full high-bit opaque arguments, mutable floats/ints, bypass-hit and early-return restoration, stale view rejection, acquisition votes and existing HUD dispatch order.
- [ ] Introduce stack-owned invocation records linked per hook ID, and RAII save/restore of the active ArgView. Never keep a pointer to a vector element that another nested call can invalidate. Checked `Observe()` guards live through the complete callback/Recall scope.

```cpp
HookInvocation invocation;
InvocationScope scope(Id, invocation);
auto observed = binding.Observe();
if (!S2Hook_EnterDispatch(observed)) return S2_Ignore();
// Fill the existing typed view; consume this ID's bypass flag exactly once.
const int action = invocation.bypass ? 0 : S2Hook_Dispatch(Id, &invocation.view);
return KHook::Recall(target, S2_FromHookResult(action), self,
                     invocation.view.f[0], invocation.view.i[0],
                     invocation.view.q[0], invocation.view.q[1]);
```

This snippet is the wide-float shape's control flow; `HookInvocation`,
`InvocationScope`, `binding` and typed `target` are local production definitions
introduced in this task. Each other shape gets explicit positional mapping from
the table, not a cast to the widest prototype. Void/nonvoid callbacks must use
their corresponding return type.

- [ ] Prove Recall keeps the invocation alive through post. Preserve the acquisition voted/result fields through peer callbacks, read original return only if it ran, and publish current effective return at our callback's position. Use the existing restrictive fold; submit a post Override only when our vote changes the engine result. Never claim control over a later peer's post result.
- [ ] Preserve HUD's copied text, controller receiver and compatibility completion event in PRE before the original, exactly once. Do not normalize it to actual KHook POST.
- [ ] Replace `s2detour::Install` with checked Function configuration and actual receipt IDs. Preserve named failure/idempotence, pending observation and resident binding lifetime. Reset follows terminal permission/removal and never destroys the active callback capsule.
- [ ] Run the new invocation tests, existing hook-dispatch/binding/shutdown tests and Linux compile. Extend the real probe with a first vertical `this_void` case, then mutation/acquire/nesting cases; real peer evidence completes in S1-6.
- [ ] Commit only this package's implementation and fixtures after task review. Runtime ABI breadth remains S2 work.

### Task 5: S1-5 — Named hooks and precache on the same backend

**Modify:** `shim/src/s2script_mm.{h,cpp}`; coordinator grants probe fixture sections.
**Consumes:** S1-3 resolution and S1-4's reviewed invocation/lifetime pattern.
**Produces:** Named hook behavior with no separate installer or shared-slot write.

- [ ] Add controlled cases before edits for TakeDamageOld pre/post nesting, chat suppression, entity-output suppression, usercmd neutralization, precache original-once and nested manifest restoration.
- [ ] Replace the four named private installers with checked native signatures. Keep original ABI widths and existing view conversions; use one typed instance per site.

```cpp
S2CheckedFunction<void, void*, void*, void*> damage(&DamagePre, &DamagePost);
S2CheckedFunction<void, void*, void*, bool, int, const char*> chat(&ChatPre, nullptr);
S2CheckedFunction<void, CEntityIOOutput*, CEntityInstance*, CEntityInstance*,
                  const CVariant*, float, void*, char*> output(&OutputPre, nullptr);
S2CheckedFunction<int, void*, void*, int, bool, float> usercmd(&UsercmdPre, nullptr);
```

`DamagePre/Post`, `ChatPre`, `OutputPre`, `UsercmdPre` are the converted current
handlers with matching `KHook::Return<T>` result types. Their semantic bodies stay
as before: usercmd neutralizes in place then Ignore; chat/output suppress at their
existing threshold; damage pointers are bound only within each callback by RAII.

Task 6 fix-round-1 correction: independent exact old/new binary audits supersede the
inherited damage ABI. `CBaseEntity_TakeDamageOld` is `void(victim*, mutable info*,
optional result*)`; both phases Ignore, and the optional result is opaque pass-through.
The old `DispatchTraceAttack` recipe identified unrelated output logic and has no alias.
Require exact three-argument forwarding, null/non-null result storage and original
writes, nested victim/info restoration, and per-phase original/peer/skipped observations.
The new recipe retains exact prologue and TakeDamageOld diagnostic string-xref,
including its newline. Static evaluator success is separate from required real damage
callbacks. The five declarative shapes above and public damage semantics are unchanged.


- [ ] Scope both damage-info and victim pointers per callback and restore outer state after nested callbacks. Validate the callback lifetime guard in accessors; no persistent raw pointer from PRE into unrelated POST work. S3 owns the later semantic extraction.
- [ ] Configure a checked virtual precache binding from the RTTI vtable and validated index. Use the existing `AddGlobal` holder pattern and retain the vtable identity, not the temporary holder address.

```cpp
struct PrecacheReceiver {};
S2CheckedVirtual<PrecacheReceiver, void, void*> precache(&PrecachePre, nullptr);
precache.Configure(index);
struct { void** vptr; } holder{vtable};
const auto receipt = precache.AddGlobal(reinterpret_cast<PrecacheReceiver*>(&holder));
```

- [ ] The real callback observes the real receiver, scopes/restores the current manifest, dispatches once and returns Ignore so the original runs once. Remove `WriteVtableSlot`, its manual restoration and obsolete original pointers only when all callers migrate. If stock KHook cannot support the slot semantics, stop this task with the demonstrated limitation; no private fallback.
- [ ] Run binding/shutdown/observer and invocation fixtures, Linux compile, and controlled probe cases. Ensure FireEvent's already-converted explicit original-call behavior is untouched.
- [ ] Commit the named and precache conversion, with a task reviewer checking ABI/receiver/filter/lifetime against the shipped code and spec.

### Task 6: S1-6 — Retire private interception and prove the integrated release

**Modify:** `shim/CMakeLists.txt`, `scripts/ci-native.sh`,
`scripts/khook_acceptance.py`, `test-khook-acceptance.py`, `test-khook-live.sh`,
`tools/khook-probe/{plugin.cpp,controlled_evidence.h}`, corresponding testdata,
`examples/khook-acceptance/src/plugin.ts`, `docs/{ARCHITECTURE,BUILDING,INSTALL,PROGRESS}.md`.
Remove production detour compilation/calls after inventory; keep a test-only
relocator only if existing standalone tests still need it.

**Interfaces:** Extend the existing controller's authenticated source-bound evidence
model for suites B and C. They currently explicitly report not-authored; merely
running `test-khook-live.sh B` is not useful evidence until this task implements
their registries, collection and judge. Reuse A's identity/run isolation rather
than inventing free-form PASS strings.

- [ ] Add judge regressions that reject missing, stale, fabricated, mismatched-owner and incomplete B/C records. A fully populated synthetic record can test the parser, but never count as live acceptance.
- [ ] Implement B cases for all S1-4 shapes and S1-5 named hooks, both peer orders, suppression/mutation, effective result, bypass, nesting, script generations and lifetime. Implement C for precache map transition, manifest nesting, original-once and a peer holding the same slot. Require observed real callbacks.
- [ ] Keep fixture-owned asynchronous removal and peer-survival observations mandatory. Classify whole-process terminal-only records as diagnostic/non-blocking under the user disposition; missing or known-failed terminal evidence cannot be fabricated as passed or force an extra quit.
- [ ] Remove `s2detour::Install` and direct interception vtable writes from production. Audit includes indirect wrappers, CMake linkage and production binaries; broad symbol absence alone cannot prove a hook works.

```bash
bash scripts/test-original-module.sh
bash scripts/test-engine-resolver.sh
bash scripts/test-engine-hook-invocation.sh
python3 scripts/test-khook-acceptance.py
bash scripts/test-khook-live.sh --self-test
make ci-native
CI=1 make ci-js
```

- [ ] Build a fresh sniper release from the exact reviewed commit in an isolated remote source directory, preserving operator configs/data. Verify artifact hashes, PLAPI18 stock Metamod identity, default plugin count and absence of synthetic fixtures in the production release. The acceptance bundle is separate.
- [ ] Run suites A, B and C from fresh identities in both peer orders. Run map transitions, entity delete/index reuse and repeated archive reload. Leave the server running; no separate quit gate. A necessary deployment restart may incidentally capture child status. Real-client witnesses remain required where the existing A registry needs them.

```bash
bash scripts/test-khook-live.sh A --collect --run-dir "$S1_A_RUN_DIR"
bash scripts/test-khook-live.sh B --collect --run-dir "$S1_B_RUN_DIR"
bash scripts/test-khook-live.sh C --collect --run-dir "$S1_C_RUN_DIR"
```

Run directories are source-bound, operator-prepared captures. B/C preparation
must be added to the existing CLI/controller in this task before these commands
can pass. Missing stages remain pending and cannot be waived by test code.

- [ ] Update the durable docs: one shared resolver/backend, stock-host floor, resident native module, normal `.s2sp` reload and remaining S2/S3 work. Record actual evidence and failures rather than forecasting a pass. Mark S1 complete only after all mandatory gates and whole-branch review pass; keep the PR draft otherwise.

## Plan self-review and handoff

| Spec requirement | Task coverage |
|------------------|---------------|
| Stock host, resident native lifetime, prior acceptance | S1-0, S1-4, S1-6 |
| Original bytes, identity, multi-match, validator stages | S1-1, S1-2 |
| Shared resolution across all consumers | S1-3 |
| Legacy declarative semantics and live borrowed scopes | S1-4 |
| Named hooks/precache, no private patch | S1-5, S1-6 |
| Peer order, map/entity/reload, real runtime evidence | S1-0, S1-6 |
| S2 ABI/policy dependency boundary and game extraction | S1-4/S1-5 preserve; separate S2/S3 plans implement |

Each implementation report includes the exact baseline/result commits, changed
paths, red/green commands and results, interface changes, and outstanding live
evidence. The coordinator creates a task diff package for independent spec/quality
review and records every disposition in this plan's ledger. Do not reuse another
plan's completion marks, leave a failed acceptance task checked, or modify shared
server state while a gate is measuring it.
