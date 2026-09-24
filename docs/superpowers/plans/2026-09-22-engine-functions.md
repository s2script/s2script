# Engine Functions Implementation Plan

**Foundation merge status — 2026-09-24:** The user explicitly authorized merging tested foundation PRs #220–224 into `main`, with S2 and S3 still unfinished. This authorization supersedes earlier draft/non-mergeable instructions only for that foundation integration; it does not waive remaining live, client, peer, map or reload acceptance gates, authorize deployment, or declare this plan complete. S2's accepted scalar/entity checkpoint is `8aac96a9eee05f577cad94ea9fc21c9b53efb6ed`, with exact JavaScript run 35961320531 and native run 35961320527 passing. Public runtime `Engine.function` and loader activation remain unimplemented. Remaining projections/lifetimes, trusted POST effects, compatibility adapters, migration, worked example, S3 integration and live acceptance still require implementation and validation.

**SDK release hold:** The SDK already contains `Engine.function` declarations and enabled function-authoring/build tooling, while the runtime does not expose that method. Do not publish the next SDK release with that mismatch: first supply and validate the matching runtime, or explicitly gate/remove the unfinished public declarations and tooling from the release. Existing pending changesets already include SDK updates, so omitting a new changeset does not prevent these files from entering the next release. The changesets workflow may create/update the separate release PR #218; merging #218, creating release tags and publishing/deploying a runtime are outside the foundation merge request. Keep that release hold explicit until resolved.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace v1 plugin calls/hooks authoring with one typed engine-function declaration whose validated call, pre-hook, and post-hook surfaces share one stock-KHook binding, remain operator-repairable, and preserve existing archives and APIs for one deprecation window.

**Architecture:** The SDK parses `gamedata/functions.jsonc` into a normalized ABI/projection/policy contract, emits the plugin-local TypeScript augmentation, derives permissions and risk metadata, and packs `engine-functions.json`. At load, core validates that normalized contract again, applies deterministic target-only overrides, asks S1's shared resolver for checked targets, and interns compatible physical targets in a generic function registry. On Linux x86_64, the shim builds a bounded scalar libffi CIF and four closures for stock KHook's low-level PRE, POST, make-return, and make-original callbacks; KHook remains the sole detour owner. One physical registration fans out to compatible plugin/package projections, while owner/generation gates, dispatch epochs, and the ledger own all callable and subscription lifetime.

**Tech Stack:** TypeScript/Node.js SDK and CLI, Rust/V8 core, C++17 Metamod shim, pinned unmodified stock KHook, pinned private/static libffi 3.7.1, JSONC, esbuild/TypeScript, CMake, Docker/sniper Steam Runtime 3 live gate.

**Spec:** `docs/superpowers/specs/2026-09-22-engine-functions-design.md`

## Baseline and required reading

- Commit `0e78515e` on `codex/engine-bindings-s3-design` is the approved design-source baseline only. Implement on the dedicated S2 branch, never from the S3 planning tip. Task 1's isolated ABI/provider/V8 proof may start before the S1 parent merge after verifying its consumed checked-binding contract matches the reviewed S1 contract. Integrate the accepted S1 implementation through an ordinary merge before any S2 deployment or resolver-dependent Task 5, and record the exact parent SHA and any integration rechecks in the ledger. “Accepted S1 prerequisite” here means its code and interface are integrated for dependent work; pending peer/real-client/map/reload evidence still keeps merge/release gates open.
- Read `CLAUDE.md`, root `AGENTS.md`, the S2 spec, `docs/superpowers/specs/2026-09-22-engine-bindings-design.md`, and `docs/superpowers/specs/2026-09-22-game-package-boundary-design.md` before editing.
- Read S1's integrated `shim/src/engine_resolver.h`, `shim/src/engine_resolver.cpp`, `shim/src/khook_binding.h`, and its native evidence. This plan consumes `s2resolve::Resolve(const TargetRecipe &, Resolution &, std::string &reason)` where `Resolution` retains its logical live address, `std::shared_ptr<const s2original::Image>`, and validation receipt. Read module identity through `Resolution.image->identity()`; there is no separate direct identity field. If S1 lands a different signature, update this plan's adapter seam before dispatching work; do not hide an S1 change in S2.
- Read the actual pinned header at `third_party/metamod-source/third_party/khook/include/khook.hpp`, especially low-level `SetupHook`, `DoRecall`, `SaveReturnValue`, `GetCurrentValuePtr`, `DestroyReturnValue`, and `FindOriginal`. `SetupHook` accepts PRE, POST, make-return, and make-original callback addresses with the target's native signature. The typed helpers prove expected sequencing but cannot supply a new runtime prototype.
- Read libffi 3.7.1's primary `doc/libffi.texi`, `include/ffi.h.in`, `LICENSE`, and x86-64 backend before implementing. `ffi_prep_cif` retains its type vector, `ffi_call` uses caller-owned aligned value storage, and `ffi_closure_alloc`/`ffi_prep_closure_loc` require the closure and CIF to outlive every possible callback. Pin `third_party/libffi` to `v3.7.1` commit `5c1c43091ed611fdea774374355eb938c73a9157`, preserve its MIT license, build PIC static-only, and hide its symbols in the shim.
- Preserve `S2_EngineCallResolve`/address C ABI and current checked-binding classes for v1 while the compatibility window is active. New v2 code consumes the S1 C++ resolver directly inside the shim and adds append-only engine ops for core.
- Current stock-host/source verification permits isolated production development to begin even if peer, real-client, map or reload evidence is still pending. Record those missing runtime observations and rework risk in the ledger, keep affected PRs draft/non-mergeable, and require the runtime gates below on integrated code. The user explicitly made the known shutdown-only SIGSEGV/139 non-blocking. Preserve that observation truthfully; do not repeat quit tests, investigate shutdown, or wait for exit 0. Native callback retirement and closure lifetime tests remain mandatory because they protect normal use and script reload.

## Global constraints

- The bounded native ABI functional proof in **S2-EF-01** is a hard predecessor of every public API, loader, and migration task: real stock-KHook/libffi callbacks must prove the bounded atom set, new-signature path, peer results, recall, suppression, removal-before-closure-free, and real-V8 busy-owner-A to owner-B synchronous re-entry. Failure of that path stops S2 and returns to design review. Known whole-process shutdown-only 139 is diagnostic/non-blocking under the explicit user disposition; never relabel it as a pass. Do not ship a finite prototype catalog or reduced call-only release.
- Use pinned, unmodified stock KHook. Do not add a private detour backend, patch KHook, use an unchecked function-pointer cast as fallback, or claim arbitrary FFI. The S2 adapter is Linux x86_64 SysV only and rejects varargs, aggregates/struct-by-value, vectors, long double, references, platform-specific calling conventions, and every atom outside the bounded set. `u8` exists only for the proven native-bool projection; general signed/unsigned 8/16-bit integer authoring remains unsupported.
- ABI, projection, and policy remain independent fields and hashes. An ABI vector never selects acquisition, HUD, damage, or any other semantic adapter.
- Core and the generic shim service stay engine-generic. S2 may carry `legacy.acquire.v1` and `legacy.hud-click.v1` only as explicit compatibility adapters with locked hashes; S3 moves those implementations into CS2 ownership without changing ids or behavior.
- Default authoring is optional, direct resolution, generic projection, `surfaces: ["call"]`, standard generic policy, and `bypass-own-hooks`. Parameters carry name/type/mutability together. `self` and `returnValue` are reserved.
- Generic non-void suppression requires `{ action: HookResult.Handled | HookResult.Stop, returnValue: R }`. Never invent a zero/default return. Generic POST observes the effective typed return and cannot override it.
- Same resolved address plus equal ABI fingerprint shares one physical target. A conflicting ABI fails before another record is interned. State-changing subscribers share the exact policy id/version; generic POST and observe-only PRE may coexist with named adapters.
- `bypass-own-hooks` filters only the calling owner generation inside s2script's fan-out. The call still traverses the live KHook target so other plugins, game-package subscribers, and external KHook peers observe it. Bypass state is nested and thread-local.
- Operator overrides repair targets only. They cannot change ABI, projection, surfaces, policy, requirement, generated types, permissions, or the base contract hash. A replacement target supplies a complete validator. A prepared override rebinds the candidate to a new target record and never retargets a live shared record.
- Permissions remain existing names and default-deny: call derives `engine:calls`; pre/post derives `engine:hooks`. Mutation and suppression are install-visible risk metadata, not new permission names. Reserved package trust comes from a host-created owner kind, never manifest text.
- Raw pointers never enter JavaScript. Entities use live handle adoption; opaque values use registered host-invalidated handles; strings/vectors are copied; borrowed views carry a dispatch epoch and reject every access after callback return or across `await`.
- Required preparation failure preserves the running generation. Optional failure creates a named unavailable binding. Once activation begins, baseline lifecycle applies: unload the old generation first; a failed candidate start cleans the candidate and does not restore arbitrary old JavaScript effects.
- All bindings, subscriptions, and shared target references are ledgered. Teardown does not depend on plugin cleanup.
- Keep v1 accepted for one published deprecation window. Migration preserves the declared validators, timing, permissions, public null behavior, and explicit adapters for correctly declared supported contracts, or stops with a named structural ambiguity without writing a weakened partial output. Neither v1 nor v2 can infer an undeclared native argument; structural conversion is not native ABI certification.
- No task checkbox is complete until its named evidence exists. Host/unit evidence does not substitute for stock-provider or live CS2 evidence.

---

## Locked design contracts

### Bounded scalar runtime ABI adapter

The normalized ABI is a vector, not a catalog key:

```text
receiver    := none | entity
native atom := u8 | i32 | u32 | i64 | u64 | f32 | f64 | ptr
return      := void | u8 | i32 | u32 | i64 | u64 | f32 | f64 | ptr
fingerprint := linux-x86_64-sysv:<receiver>:<return>(<ordered native atoms>)
```

The capability limits are 32 authored parameters, 33 CIF arguments including a member receiver, and 256 stack-copy bytes. `u8` is legal only with the author/runtime `bool` projection; inputs are canonicalized to 0 or 1 and outputs reject noncanonical values.

`shim/src/engine_function_abi.{h,cpp}` maps those atoms to libffi primitive types, prepares an `ffi_cif`, allocates four `ffi_closure` objects, and passes their executable addresses to stock `KHook::SetupHook`. All four closures share the exact target CIF but carry distinct callback phase data. PRE/POST copy arguments through `void **args` into bounded `NativeValue` storage and save the generic action/typed return with `KHook::SaveReturnValue`. Because all allowed returns are trivial 1-byte, 4-byte, or 8-byte scalars, the adapter supplies audited `Copy1`, `Copy4`, `Copy8`, and no-op scalar-destroy callbacks; it never passes a null init/destroy operation for a non-void saved value. Make-original obtains `KHook::GetOriginalFunction()`, invokes it with `ffi_call`, and saves the original result. Make-return copies `KHook::GetCurrentValuePtr(true)` into libffi's aligned result storage and calls `KHook::DestroyReturnValue()`. Changed arguments enter a nested recall through `KHook::DoRecall`, followed by exactly one `ffi_call` on its continuation with the modified argument vector. Mutation-only recall uses `Action::Ignore`, size zero and null return operations; a permitted typed override or suppression instead supplies its matching value width and non-null copy/destroy operations. Closures and their retained CIF/type vector are freed only after KHook's asynchronous removal receipt says no callback can enter.

Every value copy uses explicit fixed-width aligned storage plus `std::memcpy`; the C++17 build cannot use `std::bit_cast`. In particular, libffi widens integral returns narrower than a machine register: `ffi_call` uses zero-initialized, aligned `ffi_arg` return storage for `u8`, narrows it explicitly to a canonical 0/1 byte before `Copy1`, and POST/bridge conversion zero-extends only that byte into `NativeValue`. It never reads neighboring bytes or copies pointer-sized garbage. The closure return path also clears register-sized result storage before writing canonical 0/1. The adapter checks `FFI_DEFAULT_ABI` resolves to the expected Linux x86_64 SysV ABI, `FFI_CLOSURES` is enabled, every atom has the expected width/alignment, `ffi_prep_cif`/`ffi_prep_closure_loc` return `FFI_OK`, and executable closure allocation succeeds. Any mismatch is a named unavailable binding, never a cast or guessed call.

Compute `SetupHook`'s stack-copy byte count from the normalized vector using the SysV scalar classification used by this bounded contract: the optional member receiver consumes the first GP slot; `u8`, integer, and pointer atoms consume the six GP slots; float/double atoms consume the eight SSE slots; exhausted classes spill in original argument order to aligned eight-byte stack slots. Pass `max(128, align16(spilled_bytes))`, where 128 is the pinned provider's `STACK_SAFETY_BUFFER`, so even a no-spill signature never relies on the special zero input. Reject a result over 256 bytes or any class the helper does not understand. S2-EF-01 compares this computation with compiler-authored typed probes for no spill, mixed GP/SSE spill, and order before any public API consumes it.

The proof corpus includes native bool (`u8` with 0/1 projection) and every other atom as argument and return, free/member receivers, void/non-void, controlled S2/S3 compatibility signatures and the audited scalar CanAcquire signature, no-spill calls, mixed classes, more than six GP arguments, more than eight SSE arguments, and a previously absent native signature assembled from the bounded atoms after the runtime adapter is built. That new signature must call and hook without adding a signature-specific thunk, prototype, or regenerated callback. The legacy Ignite demo is not a scalar proof: its actual ABI ends in a by-value Vector omitted by the v1 descriptor. A failure of any mandatory case stops at design review.

ABI types and projections are separate. For example, author type `entity?` normalizes to native `ptr` plus generic `entity?` projection. `string`, `vector`, registered opaque handles, and borrowed records also use pointer-class ABI atoms but different explicit projection codecs. No projection can change the ABI fingerprint.

### Normalized archive contract

`engine-functions.json` is deterministic JSON with this top-level shape:

```ts
interface NormalizedBundle {
  schemaVersion: 2;
  ownerId: string;
  bundleHash: string; // sha256 of canonical JSON excluding bundleHash
  functions: NormalizedFunction[]; // sorted by localName
}

interface NormalizedFunction {
  localName: string;
  canonicalId: string; // `${ownerId}::${localName}`
  contractHash: string; // sha256 of canonical ABI + projection + policy; excludes target
  target: NormalizedTarget;
  abi: {
    platform: "linux-x86_64-sysv";
    receiver: "none" | "entity";
    fingerprint: string;
    parameters: { name: string; native: NativeAtom; projection: ProjectionSpec; mutable: ("pre")[] }[];
    returns: { native: NativeReturn; projection: ProjectionSpec };
  };
  policy: {
    id: "generic.v2" | string;
    version: 1;
    contractHash: string;
    surfaces: ("call" | "pre" | "post")[];
    selfCall: "bypass-own-hooks";
    suppression: "generic" | "none" | "adapter";
  };
  requirement: "optional" | "required";
}
```

The manifest contains derived `permissions` and an inspectable summary, not a second authored contract:

```ts
interface EngineFunctionsManifestSummary {
  schemaVersion: 2;
  bundleHash: string;
  functions: Array<{
    canonicalId: string;
    contractHash: string;
    surfaces: Array<"call" | "pre" | "post">;
    mutates: boolean;
    suppresses: boolean;
    requirement: "optional" | "required";
  }>;
}
```

### Core owner, adapter, status, and lifetime contract

`core/src/engine_functions/contract.rs` defines:

```rust
pub(crate) enum OwnerKind { Plugin, GamePackage }
pub(crate) struct OwnerKey { pub id: String, pub generation: u64, pub kind: OwnerKind }
pub(crate) struct PackageInstanceKey {
    pub parent: OwnerKey,        // enclosing plugin candidate context
    pub package_owner: OwnerKey, // host-minted reserved GamePackage owner
}
pub(crate) struct ImplementationManifestHash(pub String); // validated lowercase sha256 hex

pub(crate) fn prepare_owner(
    owner: OwnerKey,
    bundle: NormalizedBundle,
    overrides: OverrideSet,
) -> Result<PreparedOwnerReceipt, FunctionError>;
pub(crate) fn activate_owner(
    receipt: PreparedOwnerReceipt,
) -> Result<ActiveOwnerReceipt, FunctionError>;
pub(crate) fn drop_owner(owner: &OwnerKey);
pub(crate) fn status(owner: &OwnerKey, local_name: &str) -> FunctionStatus;
```

`GamePackage` owners and `PackageInstanceKey` values are minted by host bootstrap only. `policy.rs` owns `AdapterId`, `AdapterContract { id, version, contract_hash, visibility }`, and the process-wide semantic registry. `contract_hash` digests the canonical id/version plus the `ProjectedFrame`, `SubscriberDelivery`, `PreDecision`, phase, and timing contract; it does not digest JavaScript/Rust implementation bytes and therefore remains fixed when S3 relocates an adapter. A separate `implementation_manifest_hash` belongs to per-instance provenance. `policy.rs` contains no acquisition/HUD enum, `match` branch, integer-vote descriptor, policy DSL, or JSON interpreter. `generic.v2` is the only community-selectable policy in S2. `legacy.acquire.v1` and `legacy.hud-click.v1` require exact locked semantic contract hashes and reserved/internal normalized input.

S2 checks in canonical, sorted-key/minified contract documents under `core/src/engine_functions/contracts/`. They are conformance/hash fixtures only; runtime code never interprets them as policy. The frozen digests are:

```text
legacy.acquire.v1   69247dc63a6200f5bb8c8ff651b8dd632e8d6af9ad4a0b8d2933199800bc48c0
legacy.hud-click.v1 28c0c9833d521cadd4eb03254f63ef7dcb1cd8b728f48ebfa8835b82ff03ecdd
```

`legacy.acquire.v1.json` is exactly:

```json
{"frame":{"post":["player:entity?","defIndex:u16-number","method:i32","result:i32","skipped:bool"],"pre":["player:entity?","defIndex:u16-number","method:i32","result:i32:mutable"]},"id":"legacy.acquire.v1","preDecision":"s2.pre-decision.v1","semantics":{"allowed":0,"deny":"nonzero","denyTie":"first","engineResult":"only-if-original-ran","implicitDeny":1,"stableWithinStrength":true,"voteOrder":["handled-stop","changed"]},"subscriberDelivery":"s2.subscriber-delivery.v1","timing":{"post":"after-effective-return","pre":"before-original"},"version":1}
```

`legacy.hud-click.v1.json` is exactly:

```json
{"frame":{"delivery":["player:entity?","buttonId:copied-string"]},"id":"legacy.hud-click.v1","preDecision":"s2.pre-decision.v1","semantics":{"buttonIdCopy":"before-javascript","finalAction":"continue","reentry":"synchronous-nested"},"subscriberDelivery":"s2.subscriber-delivery.v1","timing":{"deliveryLabel":"post","nativePhase":"pre","relativeToOriginal":"before"},"version":1}
```

Their canonical content locks acquisition's pre/post public fields, stable vote ordering, `Allowed=0`, implicit deny `1`, first-deny fold, and engine-result timing; and locks HUD's `player`/copied `buttonId` delivery, compatibility label `post` during native PRE before original, synchronous re-entry, and final Continue action. S3 copies these two documents byte-for-byte. Changing either digest is a contract version change, not an implementation-source update.

`projection.rs` owns `NativeValue`, `ProjectedFrame`, `DispatchEpoch`, and internal `ProjectionCodec::{project_pre, apply_pre, project_post, invalidate}`. Generic codecs cover scalars, copied strings/vectors, entity refs, registered opaque handles, and `borrowed-record.v1`. A borrowed-record instance carries a package-owned field schema/layout hash in data; the codec has no damage or game catalog. S3 supplies CS2 damage field layout in its package data and registers that instance under its reserved owner; community descriptors cannot select a reserved instance.

`package_adapter.rs` is the relocation seam for executable game semantics. It defines one implementation-neutral contract used by both S2's temporary built-ins and S3's package JavaScript:

```rust
pub(crate) enum PreDecision {
    Continue,
    Changed,
    Suppress {
        action: SuppressAction, // Handled | Stop
        // None is legal only for a void ABI; validation rejects it otherwise.
        return_value: Option<NativeValue>,
    },
}

pub(crate) struct SubscriberDelivery {
    pub action: HookResult, // Continue | Changed | Handled | Stop
    pub return_value: Option<NativeValue>,
    pub frame_revision: u64,
}

pub(crate) trait SubscriberCursor {
    fn invoke_next(&mut self) -> Result<Option<SubscriberDelivery>, AdapterError>;
}

pub(crate) struct AdapterDispatch<'a> {
    pub phase: AdapterPhase,
    pub frame: &'a mut ProjectedFrame,
    pub cursor: &'a mut dyn SubscriberCursor,
}

pub(crate) trait DispatchAdapter {
    fn pre(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<PreDecision, AdapterError>;
    fn post(&self, dispatch: &mut AdapterDispatch<'_>) -> Result<(), AdapterError>;
}

pub(crate) struct PackageAdapterRegistration {
    pub instance: PackageInstanceKey,
    pub id: AdapterId,
    pub contract_hash: ContractHash,
    pub implementation_manifest_hash: ImplementationManifestHash,
    pub callbacks: PackageJsCallbacks,
}

pub(crate) struct PackageJsCallbacks {
    pub pre: Option<v8::Global<v8::Function>>,
    pub post: Option<v8::Global<v8::Function>>,
}

pub(crate) struct PackageAdapterReceipt {
    pub registration_id: u64,
    pub instance: PackageInstanceKey,
    pub id: AdapterId,
    pub contract_hash: ContractHash,
}
```

`PreDecision` and `SubscriberDelivery` are the same generic hook actions and typed native values already required by the public function contract; they are not a game-policy instruction set. Host-only `__s2_function_adapter_register(id, contractHash, {pre, post})` exists only while reserved package code executes in each plugin candidate context. The host derives `PackageInstanceKey` and `implementation_manifest_hash` from its hidden bootstrap token; JavaScript cannot supply or spoof them. It returns a frozen, ledgered `PackageAdapterReceipt` facade and rejects direct plugin calls, a duplicate `(instance,id)`, a process-wide id/version with a conflicting semantic hash, async functions/promises, and registration after that instance activates. The same id/hash is expected across different instances. The callback receives an `AdapterDispatch` facade with a callback-scoped safe `ProjectedFrame` and a `SubscriberCursor`. `cursor.invokeNext()` synchronously enters the next live subscriber's own context, runs its package-supplied wrapper, applies only declared frame mutations, and returns `SubscriberDelivery`; `None` means the cursor is exhausted. The adapter's JavaScript code owns mapping, vote folding, timing, and final `PreDecision`; core only enforces liveness, stable registration order, phase, value types, and the final generic native decision. No raw pointer, V8 handle from another context, or policy opcode crosses this facade.

The host-only JavaScript shape is fixed for S3:

```ts
interface AdapterDispatchFacade {
  readonly phase: "pre" | "post";
  readonly frame: ProjectedFrameFacade; // safe named projected fields, callback epoch scoped
  readonly cursor: { invokeNext(): SubscriberDeliveryFacade | null };
}
interface SubscriberDeliveryFacade {
  readonly action: HookResult;
  readonly returnValue?: unknown; // projected and checked against the locked binding return type
  readonly frameRevision: number;
}
interface PackageAdapterReceiptFacade {
  readonly status: "active" | "disposed";
  dispose(): boolean;
}
type PreDecisionFacade =
  | HookResult.Continue | HookResult.Changed | void
  | { action: HookResult.Handled | HookResult.Stop; returnValue?: unknown };
type PackageAdapterCallbacks = {
  pre?: (dispatch: AdapterDispatchFacade) => PreDecisionFacade;
  post?: (dispatch: AdapterDispatchFacade) => void;
};
declare function __s2_function_adapter_register(
  id: string, contractHash: string, callbacks: PackageAdapterCallbacks,
): PackageAdapterReceiptFacade;
declare function __s2_function_adapter_subscribe(
  bindingId: bigint, adapterId: string, phase: "pre" | "post",
  wrapper: (view: ProjectedFrameFacade) => unknown,
): FunctionSubscription;
```

Every callback return is checked synchronously before control leaves its context. A returned Promise or thenable is an adapter error and cannot retain `frame` or `cursor`.

Game-package wrappers call host-only `__s2_function_adapter_subscribe(bindingId, adapterId, phase, wrapper)` from the current plugin context. The host verifies the binding's locked adapter contract and current `PackageInstanceKey`, records the parent plugin owner/generation, and returns the ordinary ledgered `FunctionSubscription`. For each mutating domain, dispatch selects the first active eligible adapter instance in stable registration order. It excludes unloading and currently busy contexts; under `bypass-own-hooks` it also excludes the instance whose parent is the caller owner-generation. It then filters that caller's subscriber before cursor iteration, so another plugin/package instance can still run synchronously. If live subscribers remain but no eligible instance exists, dispatch fails by name; it never defers or changes timing. Teardown by parent or package owner drains only that exact instance and its receipts, leaving peers intact. S2-EF-06 must prove owner A calling the live hooked target while owner A is busy/bypassed still invokes owner B's adapter instance and wrapper in owner B's real V8 context. No dedicated game V8 context is introduced. If this cannot be proven in S2-EF-06/10, the adapter remains a failed gate rather than falling back to deferral.

`binding.rs` owns `FunctionBindingId`, `FunctionSubscriptionId`, `BindingReceipt { id, owner, target_id }`, `SubscriptionReceipt { id, owner, binding_id, phase }`, and:

```rust
pub(crate) struct AppliedOverrideReceipt {
    pub relative_path: String,
    pub sha256: String,
}
pub(crate) struct Provenance {
    pub archive_hash: String,
    pub base_contract_hash: ContractHash,
    pub applied_overrides: Vec<AppliedOverrideReceipt>,
    pub final_target_hash: String,
    pub resolver_receipt: String, // diagnostic receipt only; never an address
    pub required: bool,
}
pub(crate) struct BindingStatus {
    pub availability: Availability,       // Available | Unavailable(reason)
    pub hook_observation: HookObservation, // NotRequested | Pending | Active | Failed(reason)
    pub canonical_id: String,
    pub provenance: Provenance,
}
```

The plugin ledger adds `Resource::FunctionBinding(u64)` and `Resource::FunctionSubscription(u64)`. Every call, property access, subscribe, status query, and dispose checks the exact `OwnerKey`. An old generation can never address a new generation's binding.

### Native bridge contract

Append generated engine ops for a handle-based bridge; do not pass native addresses to core or JavaScript:

```c
typedef long long s2_function_target_id;
typedef struct {
    unsigned char kind;   /* bool/u8,i32,u32,i64,u64,f32,f64,ptr/null */
    unsigned char flags;
    unsigned short reserved;
    unsigned int aux;
    unsigned long long bits;
} S2FunctionValue;

s2_function_target_id S2_FunctionPrepare(
    const char *canonical_id, const char *target_json, const char *abi_json,
    const char *abi_fingerprint, char *reason, int reason_cap);
int S2_FunctionCall(s2_function_target_id target, unsigned long long owner_token,
    const S2FunctionValue *args, int argc, S2FunctionValue *ret,
    char *reason, int reason_cap);
long long S2_FunctionHookAcquire(s2_function_target_id target, char *reason, int reason_cap);
int S2_FunctionHookRelease(s2_function_target_id target);
int S2_FunctionTargetRelease(s2_function_target_id target);
```

The shim calls the core export `s2script_core_dispatch_function(target_id, frame, phase)` from the common libffi closure handler. `frame` is callback-scoped internal FFI state; core never converts its address to a V8 value. Read/write ops validate the target id, ABI fingerprint, parameter index, projection request, active thread, and dispatch epoch.

### Public TypeScript shape

`packages/sdk/unsafe.d.ts` defines the base empty augmentation map and stable shell:

```ts
export interface EngineFunctions {}

export interface FunctionStatus {
  readonly canonicalId: string;
  readonly availability: "available" | "unavailable";
  readonly reason: string | null;
  readonly hookObservation: "not-requested" | "pending" | "active" | "failed";
  readonly provenance: {
    readonly archiveHash: string;
    readonly baseContractHash: string;
    readonly appliedOverrides: readonly { readonly path: string; readonly sha256: string }[];
    readonly finalTargetHash: string;
    readonly resolverReceipt: string;
    readonly required: boolean;
  };
}

export interface FunctionSubscription extends Disposable {
  readonly status: "pending" | "active" | "failed" | "disposed";
  readonly reason: string | null;
  dispose(): boolean;
}

export declare const Engine: {
  function<K extends keyof EngineFunctions>(name: K): EngineFunctions[K];
  // v1 call/status/hook/hookStatus remain for the deprecation window.
};
```

The generated file emits concrete binding/view interfaces per local function. Optional functions are a discriminated available/unavailable union; required functions are the available branch because unresolved required functions prevent evaluation. Unsupported surfaces are absent rather than runtime no-ops. Observe-only PRE accepts only a readonly view and a `void` callback. Generic non-void mutable PRE accepts only `Continue | Changed | void | { action: Handled | Stop; returnValue: R }`.

---

## Dependency DAG and ownership

```text
S2-EF-01 (hard ABI proof)
  ├── S2-EF-02 (SDK parser/model/normalizer)
  │     └── S2-EF-03 (types/build/archive/inspect)
  │            └── S2-EF-04 (runtime parse + overrides + provenance)
  └── S2-EF-05 (native bridge + physical target registry; also needs accepted S1)
                 └──────────────┐
S2-EF-04 ───────────────────────┴── S2-EF-06 (core function registry/projection/policy)
                                          └── S2-EF-07 (V8 API + ledger + activation)
                                                 ├── S2-EF-08 (v1 migration/compat adapters)
                                                 └── S2-EF-09 (worked v2 fixture)
S2-EF-08 + S2-EF-09 ──────────────────────────────── S2-EF-10 (CI/live acceptance/docs)
```

The S2-EF-01 dependency edge is released only by the functional stock-provider/live evidence named above. Known shutdown-only 139 does not hold that edge or its checkbox open; missing required runtime, re-entry or callback-retirement evidence does.

Each worker receives one task id, exact baseline/integrated prerequisite SHAs, and the file allowlist below. One coordinator owns shared files: `core/engine-ops.jsonc`, its generated mirrors, `shim/CMakeLists.txt`, `scripts/ci-native.sh`, `scripts/ci-js.sh`, `packages/sdk/src/build.ts`, `packages/sdk/src/commands/index.ts`, `core/src/loader.rs`, and `core/src/v8host/natives.rs`. Workers needing an unlisted shared edit report it rather than editing opportunistically. Do not run multiple workers against the same checkout or shared live server.

---

### Task 1: S2-EF-01 — Prove the bounded stock-KHook ABI adapter

**Files:**
- Modify: `.gitmodules`
- Add pinned submodule: `third_party/libffi` at `5c1c43091ed611fdea774374355eb938c73a9157` (`v3.7.1`)
- Create: `shim/engine-function-abi.jsonc`
- Create: `scripts/gen-engine-function-abi.py`
- Create: `shim/src/engine_function_abi.h`
- Create: `shim/src/engine_function_abi.cpp`
- Create: `shim/src/engine_function_abi.generated.inc` (atom/platform bounds only; no prototypes)
- Create: `shim/tests/engine_function_abi_test.cpp`
- Create: `shim/cmake/Libffi.cmake`
- Create: `tools/engine-function-probe/CMakeLists.txt`
- Create: `tools/engine-function-probe/plugin.cpp`
- Create: `tools/engine-function-probe/README.md`
- Create: `core/src/v8host/engine_function_adapter_v8.rs` (test-only feasibility harness)
- Modify: `core/src/v8host.rs` (adjacent `cfg(test)` module include only)
- Create: `scripts/test-engine-function-abi.sh`
- Create: `scripts/test-engine-function-v8-adapter.sh`
- Create: `scripts/test-engine-function-live.sh`
- Modify: `scripts/gen-licenses.sh`
- Modify (generated): `licenses/licenses.txt`
- Modify: `shim/CMakeLists.txt` (coordinator-owned integration)
- Modify: `scripts/ci-native.sh` (coordinator-owned integration)
- Modify: `.github/workflows/ci-native.yml` (dedicated gate path filters only)

**Allowlist:** Only the files above. The Rust file is a test-only real-V8 feasibility harness; do not add production Rust, SDK, existing KHook-wrapper, S1-resolver, or game-package changes in this task.

**Interfaces:**
- Consumes: pinned low-level `khook.hpp`, S1 `S2HookReceipt`/Observe/retirement contract, and pinned libffi.
- Produces: `s2fn::AbiSignature`, `s2fn::AbiFingerprint`, `s2fn::NativeValue`, `s2fn::DispatchFrame`, and `s2fn::RuntimeBinding::{Create,Call,Configure,BeginRemove,RemovalComplete}`. The capability source supplies the same platform/receiver/atom/return bounds to generated C++, TypeScript, and Rust artifacts; it contains no prototype list.

- [ ] **Step 1: Pin and license the private static libffi dependency**

Add `third_party/libffi` at commit `5c1c43091ed611fdea774374355eb938c73a9157`. Add its readable `third_party/libffi/LICENSE` directly to `scripts/gen-licenses.sh`'s native inventory, regenerate `licenses/licenses.txt`, and run `scripts/check-licenses-generated.sh`. `Libffi.cmake` must configure only the pinned source with `--disable-shared --enable-static --with-pic --disable-multi-os-directory`, import the resulting archive, and link it privately with `--exclude-libs,ALL`; do not discover or `dlopen` a host libffi. Add checks that `ldd build/shim/s2script.so` has no libffi dependency and `nm -D` exports no `ffi_*` symbol. Keep the repository's C++17 standard.

```bash
./scripts/gen-licenses.sh
./scripts/check-licenses-generated.sh
```

Expected: PASS and generated notices name libffi 3.7.1 plus the exact submodule revision.

- [ ] **Step 2: Write red bounded-signature, CIF, and lifetime tests**

Write `engine_function_abi_test.cpp` first. It must accept arbitrary vectors composed only of the locked atoms, distinguish receiver/return/order/width in the fingerprint, and reject the first unsupported platform, receiver, atom, varargs flag, aggregate, non-bool `u8` projection, or argument-count/byte overflow by name. Add `u8` call/closure tests that poison surrounding/result storage and prove canonical one-byte input plus zero-extended output, stack-copy tests for the 128-byte no-spill floor and GP/SSE overflow up to the 256-byte bound, CIF/type-vector ownership, aligned result storage, four closure allocation failures, partial-construction cleanup, and removal-before-free. Add a generator `--check` test for the capability bounds rather than a row list.

Run:

```bash
python3 scripts/gen-engine-function-abi.py --check
bash scripts/test-engine-function-abi.sh
```

Expected: FAIL because the runtime CIF/closure adapter is absent; unsupported inputs already fail with the first named feature rather than a compiler crash.

- [ ] **Step 3: Implement the bounded CIF and four stock-KHook callbacks**

Implement one `RuntimeBinding` per physical target, with one retained CIF/type vector and four phase-tagged closures:

```cpp
class RuntimeBinding final : private S2CheckedBindingOps {
 public:
  static Result<std::unique_ptr<RuntimeBinding>> Create(
      AbiSignature signature, DispatchSink &sink);
  Result<NativeValue> Call(const NativeValue *args, std::size_t argc);
  S2HookReceipt Configure(const void *address);
  void BeginRemove();
  bool RemovalComplete() const;
 private:
  static void ClosureEntry(ffi_cif *, void *result, void **args, void *phase);
  static void OnKHookRemoved(KHook::HookID_t);
  ffi_cif cif_;
  std::vector<ffi_type *> argument_types_; // retained for cif_ lifetime
  Closure pre_, post_, make_return_, make_original_;
  KHook::HookID_t hook_id_ = KHook::INVALID_HOOK;
  const void *target_ = nullptr; // retained live entry, never provider original
};
```

Map only the bounded primitive `ffi_type_*` objects. For a member ABI, prepend the receiver pointer to the CIF while omitting it from authored parameters. Use `std::memcpy` into width-checked aligned storage and select only `Copy1`/`Copy4`/`Copy8` plus the no-op scalar destructor for non-void saved values. Handle libffi's narrow integral-return rule with aligned `ffi_arg` storage and explicit bool narrowing/zero-extension; never reinterpret the low byte of an uninitialized larger slot. PRE/POST dispatch through the phase tag, retain `ObserveOwned(hook_id_)` for the complete callback/recall scope, and call `KHook::SaveReturnValue`. Changed PRE calls `KHook::DoRecall`, then invokes its returned continuation through `ffi_call` using the edited vector. Make-original calls `KHook::GetOriginalFunction()` via `ffi_call` and saves its result as original. Make-return copies `KHook::GetCurrentValuePtr(true)` to the cleared closure result and then destroys KHook's saved value. `OnKHookRemoved` is a fixed `void(HookID_t)` callback that obtains the binding from KHook context; it is not target-signature-specific. Calls use the live target address, not `FindOriginal`, so owner-only bypass remains fan-out filtering rather than bypassing peers.

Compute and bounds-check KHook stack-copy bytes from SysV scalar GP/SSE classification, including the member receiver. Compare the helper against compiler-authored probes with no spill, GP-only spill, SSE-only spill, and interleaved independent-class spill. Do not reuse KHook's conservative compile-time template helper as runtime evidence.

`Configure` installs synchronously with explicit `async=false` from an off-callback preparation boundary; a valid ID alone is not asynchronous readiness evidence. A mutation-only recall uses `Action::Ignore`, size zero and null return operations. A permitted typed override/suppression uses its matching value width and non-null copy/destroy operations. Retain the observation guard across exactly one recall continuation call, then return without a second independent save. Make-return copies the effective value before calling `DestroyReturnValue` exactly once on every void/non-void path; no exception may escape a closure.

`OnKHookRemoved` records provider detachment but does not free closures, phase data, the CIF/type vector or the binding. `BeginRemove` delegates to the checked asynchronous retirement path. `RemovalComplete` requires both provider detachment and the generic checked receipt before the owner may destroy the retained binding. Test duplicate removal, queued-not-yet-inserted removal, and refusal to destroy before both completions; do not busy-wait on the game thread.

Run the same commands. Expected: generator freshness PASS; host runtime-CIF tests PASS.

- [ ] **Step 4: Prove arbitrary bounded vectors against the real stock provider**

The probe must register controlled native targets through the real stock KHook low-level API and assert: free/member receiver delivery; native bool/u8 with false/true arguments and returns; every signed/unsigned 32/64-bit, `f32`, `f64`, and pointer atom as argument and return; void; no-spill signatures; mixed classes; more than six GP and more than eight SSE arguments; interleaved spill order; PRE mutation through `DoRecall`; void and typed suppression; POST effective return; skipped-original state; nested re-entry; two concurrent s2script subscribers; and cleanup completion. Build the adapter first, then add a previously absent signature composed from supported atoms only to the probe data. It must call and hook without adding a signature-specific thunk, generated prototype, or adapter source change.

Run:

```bash
bash scripts/test-engine-function-abi.sh --stock-provider
```

Expected: PASS with KHook/libffi commit identities, each vector/fingerprint, computed stack bytes, and closure/removal states printed. A fake KHook result or libffi-only call test is insufficient.

The dedicated script builds a test-only, `EXCLUDE_FROM_ALL` stock provider from the pinned, unmodified KHook/SafetyHook sources and the checked-in Zydis amalgamation, following upstream's AMBuilder recipe. It asserts Linux x86_64 and prints all source revisions. Compile the focused executable and a shared V8 test bridge from `shim/tests/engine_function_abi_test.cpp` with a macro excluding `main` for the bridge. `KHOOK_STANDALONE` belongs only to these test targets; production `s2script.so` must still use Metamod's single provider and contain no test provider dependency/export. The source-bound deployable bundle builder is a later live artifact gate, not this engine-free provider proof.

Add `scripts/*engine-function*` and `tools/engine-function-probe/**` to both native workflow event path filters, and invoke both dedicated ABI and V8 proof scripts from `ci-native.sh`.

- [ ] **Step 5: Prove busy-caller cross-context delivery in real V8**

Before SDK or public API work, add a narrow Rust/V8 integration harness over the real runtime binding probe. Create two real V8 plugin contexts, each with the same host-minted package contract instance and one test wrapper. Give owner A a test-only private global that calls the live hooked native target with A's owner token while A's JavaScript context is busy. The KHook callback must re-enter the isolate synchronously, exclude A's busy/bypassed package instance and subscriber, select owner B's per-context instance, run B's wrapper before the native call resumes, and return B's typed decision/result. Prove a second A generation can unload/reload without retiring B. Do not emulate this with a native-only callback, deferred queue, or dedicated game context.

```bash
bash scripts/test-engine-function-v8-adapter.sh --spike --stock-provider
```

Expected: PASS with owner/package generation keys, selected B instance, synchronous callback order, typed result, and independent A/B teardown. Failure is a design stop before S2-EF-02.

Include the harness as a `cfg(test)` child of `v8host`, beside its existing private unit-test module. A file under `core/tests` would also become an independent Cargo integration crate and cannot access the required private host state; do not widen visibility or disable Cargo autotests. The dedicated script passes an absolute bridge path and selects one exact ignored Rust test. Ordinary core tests must not substitute a missing provider.

Use the actual host context/generation installation and `nest::with_outbound` path. A's test-only V8 native calls the live target; stock KHook enters the real closure and Rust sink, which uses the published nest token, a real `CallbackScope`, liveness checks and B's `ContextScope` to synchronously call B. Assert `A-before → KHook-PRE → B-wrapper → original/return → A-after`, then repeat after reloading A while B stays live. Holding a host mutable borrow and observing a skipped/deferred callback is insufficient. The test-only typed decision parser proves transport without claiming today's generic `fan_out_inner` already transports typed return values.

- [ ] **Step 6: Prove peer-result and load-order coexistence**

Extend the companion probe to install an external typed KHook peer before and after the runtime binding. Run both orders for a no-spill bool-return signature and a mixed stack-spill signature. Verify s2script POST sees the typed, zero-extended effective peer return, s2script suppression is visible to the peer, cleanup removes only s2script's registration, and the result does not depend on assumed callback order.

Run:

```bash
bash scripts/test-engine-function-live.sh --fixture-only --orders peer-first,s2-first
```

Expected: both orders PASS with real callbacks and effective result observations.

- [ ] **Step 7: Run the deployable bullseye/live proof and record the gate**

Build through `scripts/build-sniper.sh`; deploy the probe plus a minimal internal fixture. Label evidence separately: (1) real-engine `CCSPlayer_ItemServices::CanAcquire(CEconItemView*, AcquireMethod, void*) -> AcquireResult`, whose bounded member ABI is receiver pointer plus pointer, i32, pointer arguments and i32 return, using actual bot ItemServices/CEconItemView objects and bot item actions, with public acquisition PRE/POST observations, non-skipped outer/nested completion, main/peer POST current-return observations, and separately labeled final-caller result; (2) controlled compatibility and newly assembled scalar signatures, including direct original-once body counters, recall, peer effective-return observations, suppression and cleanup; and (3) real-V8 owner-A-to-owner-B synchronous re-entry and lifetime evidence. An explicitly armed, owned-bot CanAcquire PRE may make a one-shot nested live-target call only with its unchanged callback-scoped arguments, a depth latch and checked observation guard. It must never replay saved pointers later or run against human activity. No fake pointer, function body, or original-once counter may stand in for real engine objects or controlled original execution. Trigger script unload/reload and leave the server running after runtime tests. Record `ldd`, exported-symbol, license freshness, and GLIBC evidence for the statically linked dependency. The deployed CanAcquire recipe and validators must be derived from the audited exact server binary; an illustrative or stale pattern is not a deployable target.

Run:

```bash
sudo docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
bash scripts/test-engine-function-live.sh --docker docker/docker-compose.yml --rcon scripts/rcon.py
```

Expected functional gate: each labeled real-engine, controlled, and real-V8 vector reports observed; the real CanAcquire row uses actual bot objects and shows non-skipped completion and effective POST return without equating it to the later final-caller result, while controlled rows prove original-once counters and peer result semantics. Nested owner-only bypass leaves peer observations intact, and reload drains KHook before closure free. Store evidence under `.gate/engine-functions/<commit>/`; do not commit binary/log artifacts. A necessary deployment stop may incidentally record child status, but the known shutdown-only 139 is diagnostic/non-blocking and needs no additional quit or clean-exit test.

- [ ] **Step 8: Enforce the hard decision gate**

If any mandatory compatibility/example vector, newly introduced signature, peer effective result, recall mutation, typed suppression, stack classification, re-entry, or removal-before-free case fails, write the precise failed fingerprint/provider behavior in the task report and stop. Return to design review; do not silently replace the runtime adapter with a finite prototype list, delete the case, add a hand-written target thunk, patch KHook, bypass it with another detour, or continue to public API work. Runtime crashes, script-reload failures and unsafe callback retirement remain functional stops; the known whole-process shutdown-only symptom does not.

- [ ] **Step 9: Commit the proven adapter gate**

```bash
git add .gitmodules third_party/libffi scripts/gen-licenses.sh licenses/licenses.txt \
  shim/engine-function-abi.jsonc scripts/gen-engine-function-abi.py shim/cmake/Libffi.cmake \
  shim/src/engine_function_abi.h shim/src/engine_function_abi.cpp \
  shim/src/engine_function_abi.generated.inc shim/tests/engine_function_abi_test.cpp \
  tools/engine-function-probe core/src/v8host/engine_function_adapter_v8.rs core/src/v8host.rs \
  scripts/test-engine-function-abi.sh scripts/test-engine-function-v8-adapter.sh \
  scripts/test-engine-function-live.sh shim/CMakeLists.txt scripts/ci-native.sh .github/workflows/ci-native.yml
git commit -m "test: prove bounded runtime engine function ABI"
```

---

### Task 2: S2-EF-02 — Parse, normalize, and validate v2 authoring

**Files:**
- Create: `packages/sdk/src/engine-functions/model.ts`
- Create: `packages/sdk/src/engine-functions/parse.ts`
- Create: `packages/sdk/src/engine-functions/normalize.ts`
- Create: `packages/sdk/src/engine-functions/canonical-json.ts`
- Create: `packages/sdk/src/engine-functions/abi.generated.ts`
- Create: `packages/sdk/test/engine-functions-parse.test.mjs`
- Create: `packages/sdk/test/engine-functions-normalize.test.mjs`
- Modify: `packages/sdk/src/gamedata/jsonc.ts`

**Allowlist:** Only these SDK model/parser files and tests. Do not wire `build.ts` yet.

**Interfaces:**
- Consumes: exact proven platform/receiver/atom/return bounds generated by S2-EF-01.
- Produces: `parseFunctionFile(path, text)`, `normalizeFunctions(ownerId, parsed) -> NormalizedBundle`, canonical JSON/hashing, and the normalized contract above.

- [ ] **Step 1: Write red schema/default/duplicate tests**

Test the spec example, no-file behavior, defaults, local target references, canonical id, deterministic sorting/hash, and duplicate JSON keys at every object depth. Use TypeScript's JSON AST (`ts.parseJsonText`) to retain property occurrences; do not call `JSON.parse` first and lose duplicates.

Also test errors for `self`/`returnValue`, duplicate parameter names, missing/empty validator, unknown resolver, incomplete target, unsupported platform/ABI atom/varargs/aggregate, ABI argument/count byte bounds, illegal projection, reserved adapter selection, mutation outside PRE, POST suppression, non-void bare `Handled`, and unsupported surface/policy combinations.

Run:

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test \
  test/engine-functions-parse.test.mjs test/engine-functions-normalize.test.mjs
```

Expected: FAIL with missing modules/functions.

- [ ] **Step 2: Implement the simpler authoring model and defaults**

Accept only:

```ts
interface FunctionFileV2 {
  schemaVersion: 2;
  targets?: Record<string, AuthorTarget>;
  functions: Record<string, {
    target: AuthorTarget | { ref: string };
    receiver?: { type: "none" | "entity" };
    parameters?: Array<{
      name: string;
      type: AuthorType;
      mutable?: "pre";
    }>;
    returns?: AuthorType | "void";
    surfaces?: Array<"call" | "pre" | "post">;
    requirement?: "optional" | "required";
    resolve?: "direct" | "ctor-body-xref" | "lea-disp" | "validated-call";
  }>;
}
```

Default receiver to `none`, parameters to `[]`, returns to `void`, surfaces to `["call"]`, requirement to `optional`, direct resolution, generic projection/policy, and owner-only bypass. Flatten local target refs without permitting cross-owner refs. Hash ABI/projection/policy but exclude target so an operator repair leaves the public contract hash stable.

Normalize author `bool` to native `u8` plus the locked bool projection. No other author type may select `u8`, and no raw `i8/u8/i16/u16` type is exposed in v2. Emit at most 32 authored parameters and reject any normalized ABI whose computed stack-copy requirement exceeds 256 bytes.

- [ ] **Step 3: Validate staged resolver semantics**

Represent `validated-call` candidate-stage validators separately from derived-target validators. Add a fixture with two call sites where only one candidate passes string-xref, then derive E8 exactly once; assert normalization does not move/replay the call-site validator onto the callee. Unknown derivations fail by name.

- [ ] **Step 4: Run focused and existing gamedata SDK tests**

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test \
  test/engine-functions-*.test.mjs test/gamedata-validate.test.mjs test/gamedata-gen.test.mjs
```

Expected: PASS; existing v1 tests remain unchanged.

- [ ] **Step 5: Commit the normalized contract**

```bash
git add packages/sdk/src/engine-functions packages/sdk/src/gamedata/jsonc.ts \
  packages/sdk/test/engine-functions-parse.test.mjs \
  packages/sdk/test/engine-functions-normalize.test.mjs
git commit -m "feat: normalize engine function declarations"
```

---

### Task 3: S2-EF-03 — Generate editor types and pack the derived archive contract

**Files:**
- Create: `packages/sdk/src/engine-functions/emit-dts.ts`
- Create: `packages/sdk/src/engine-functions/archive.ts`
- Create: `packages/sdk/src/commands/inspect.ts`
- Create: `packages/sdk/test/engine-functions-types.test.mjs`
- Create: `packages/sdk/test/engine-functions-build.test.mjs`
- Create: `packages/sdk/test/engine-functions-inspect.test.mjs`
- Modify: `packages/sdk/unsafe.d.ts`
- Modify: `packages/sdk/src/build.ts` (coordinator-owned)
- Modify: `packages/sdk/src/typecheck/typecheck.ts`
- Modify: `packages/sdk/src/commands/index.ts` (coordinator-owned)
- Modify: `packages/sdk/src/cli.ts`
- Modify: `packages/sdk/src/create/create.ts`
- Modify: `packages/sdk/test/create*.test.mjs`

**Allowlist:** SDK/package files above. Do not edit Rust or shim code.

**Interfaces:**
- Consumes: `NormalizedBundle` and exact public API contract.
- Produces: `.s2script/engine-functions.d.ts`, archive member `engine-functions.json`, manifest summary/derived permissions, and `s2s inspect` rendering.

- [ ] **Step 1: Add red declaration/typecheck fixtures**

Build temporary valid and invalid plugins that declare one optional non-void function and one required void function. Assert exact call argument types, available discrimination, PRE mutability, observe-only readonly behavior, typed suppression return, POST readonly/effective return, unsupported surface absence, and reserved-field rejection. Invalid fixtures must fail with normal TS diagnostics, not `any` widening.

Run:

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test test/engine-functions-types.test.mjs
```

Expected: FAIL because the augmentation/emitter does not exist.

- [ ] **Step 2: Emit concrete binding and view types**

For each function emit named `FooPreView`, `FooPostView`, `FooAvailableBinding`, and optional unavailable union. Emit writable setters only for parameters marked `mutable: "pre"`; all other fields are readonly. For non-void PRE, exclude bare `Handled`/`Stop` from the callback return and require the typed object. For observe-only PRE emit only:

```ts
onPre(options: { observeOnly: true }, handler: (view: Readonly<FooPreView>) => void): FunctionSubscription;
```

Required functions map directly to `FooAvailableBinding`; optional functions map to `{available:false,status}|FooAvailableBinding`.

- [ ] **Step 3: Wire discovery, typecheck, permissions, and archive packing**

In `buildPlugin`, discover exactly `<plugin>/gamedata/functions.jsonc`; absence means no v2 capability. Do not read `s2script.gamedata`, package-derived gamedata filenames, authored engine permission strings, `requiresGamedata`, or manual tsconfig entries for v2. Build order is parse → v1 normalize if present → validate → emit declarations → typecheck/lint → derive manifest → pack.

Pack deterministic `engine-functions.json`. Add `manifest.engineFunctions` summary and union its derived permissions into `manifest.permissions`; reject authored `engine:calls`/`engine:hooks` when they merely duplicate v2 derivation with an actionable removal message, while continuing to accept them for v1 during the deprecation window. Print derived permissions and risks during build.

Build emits current editor declarations and deletes stale `.s2script/engine-functions.d.ts` when the file disappears. Standalone typecheck derives declarations from current validated source in memory, overriding stale disk declarations without emitting new gamedata files, following S1's shared type-preparation pattern. Add declarations to the typecheck root internally so authors need no tsconfig include. Test clean checkouts and changed/removed functions through standalone typecheck as well as build.

- [ ] **Step 4: Add inspect and scaffold behavior**

`s2s inspect <archive>` must show canonical id, contract hash, requirement, surfaces, mutation/suppression risk, and derived permissions without loading. `s2s create` should not create a functions file by default; when one is later added it is auto-discovered.

Run:

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test \
  test/engine-functions-types.test.mjs test/engine-functions-build.test.mjs \
  test/engine-functions-inspect.test.mjs test/create*.test.mjs
npm run build
```

Expected: PASS and archive members include normalized `engine-functions.json` with no package path metadata.

- [ ] **Step 5: Commit the public build contract**

```bash
git add packages/sdk/unsafe.d.ts packages/sdk/src packages/sdk/test
git commit -m "feat: generate and pack engine function contracts"
```

---

### Task 4: S2-EF-04 — Revalidate archives, load deterministic overrides, and record provenance

**Files:**
- Create: `core/src/engine_functions/mod.rs`
- Create: `core/src/engine_functions/contract.rs`
- Create: `core/src/engine_functions/abi.generated.rs`
- Create: `core/src/engine_functions/overrides.rs`
- Create: `core/src/engine_functions/provenance.rs`
- Create: `shim/src/plugin_function_overrides.h`
- Create: `shim/src/plugin_function_overrides.cpp`
- Create: `shim/tests/plugin_function_overrides_test.cpp`
- Create: `scripts/test-plugin-function-overrides.sh`
- Modify: `core/src/lib.rs`
- Modify: `core/src/loader_worker.rs`
- Modify: `core/src/loader.rs` (coordinator-owned preparation seam only)
- Modify: `core/engine-ops.jsonc` and generated mirrors (coordinator-owned)
- Modify: `shim/CMakeLists.txt` (coordinator-owned)

**Allowlist:** Contract/override/provenance files plus exact loader/op integration. Do not implement registry dispatch or V8 API yet.

**Interfaces:**
- Consumes: packed `engine-functions.json`, manifest summary/hash, operator addon root from shim.
- Produces: `OverrideSet`, `Provenance`, runtime `NormalizedBundle` parser/validator, and a prepared immutable candidate input.

- [ ] **Step 1: Write red archive parity and override tests**

Test SDK/runtime rejection parity for unknown keys, bad schema/hash, manifest/member mismatch, unsupported ABI, forbidden adapters, and unsafe projections. Test exact RFC 4648 URL-safe unpadded mapping (`@demo/fire` → `id-QGRlbW8vZmlyZQ`), exact UTF-8/case preservation, sorted files, path/hash provenance, missing directory success, malformed attributed-file isolation, unattributable directory failure, symlink/path escape refusal, and bounded file/count/byte limits.

Test override rules: exact base contract hash, target-only fields, complete validator on a replacement target, explicit `supersedes` for a later file touching the same function, stale hash refusal, and rejection of ABI/projection/policy/surface/requirement edits.

Run:

```bash
cargo test -p s2script-core engine_functions::
bash scripts/test-plugin-function-overrides.sh
```

Expected: FAIL because runtime parser/override snapshot is absent.

- [ ] **Step 2: Parse and validate the normalized bundle independently**

Use `#[serde(deny_unknown_fields)]` on v2 runtime structs. Recompute canonical bundle and contract hashes rather than trusting the SDK. Compare the manifest summary exactly. Preserve candidate-stage versus target-stage validators.

- [ ] **Step 3: Implement the override snapshot reader**

The shim owns addon-root discovery and reads:

```text
addons/s2script/gamedata/plugins/id-<base64url(plugin id)>/custom/*.jsonc
```

Return sorted `(relative_path, sha256, content)` records through an append-only engine op. The Rust merge applies target replacement only and creates the final target hash/provenance. Do not mutate global gamedata or v1's custom merge.

- [ ] **Step 4: Add the pre-unload preparation seam**

Extend `PreparedPlugin` with the normalized bundle member. In `apply_prepared`, parse/revalidate overrides and construct candidate provenance before `unload_plugin(old_id)`. Do not resolve/install yet; S2-EF-06 completes preparation. A failure calls `refuse_prepared(..., keeping the running version)`.

Run:

```bash
cargo test -p s2script-core engine_functions::
cargo test -p s2script-core loader::
bash scripts/test-plugin-function-overrides.sh
python3 scripts/gen-engine-ops.py --check
```

Expected: PASS.

- [ ] **Step 5: Commit immutable override/provenance preparation**

```bash
git add core/src/engine_functions core/src/lib.rs core/src/loader_worker.rs core/src/loader.rs \
  core/engine-ops.jsonc core/src/engine_ops.generated.rs \
  shim/include/s2script_engine_ops.generated.h shim/src/s2script_engine_ops_fill.generated.inc \
  shim/src/plugin_function_overrides.* shim/tests/plugin_function_overrides_test.cpp \
  scripts/test-plugin-function-overrides.sh shim/CMakeLists.txt
git commit -m "feat: prepare engine function overrides and provenance"
```

---

### Task 5: S2-EF-05 — Integrate S1 resolution and intern physical native targets

**Files:**
- Create: `shim/src/engine_function_bridge.h`
- Create: `shim/src/engine_function_bridge.cpp`
- Create: `shim/tests/engine_function_bridge_test.cpp`
- Create: `scripts/test-engine-function-bridge.sh`
- Modify: `core/engine-ops.jsonc` and generated mirrors (coordinator-owned)
- Modify: `shim/CMakeLists.txt` (coordinator-owned)
- Modify: `scripts/ci-native.sh` (coordinator-owned)

**Allowlist:** Native bridge files and append-only ops integration. Do not edit S1 resolver or SDK/core policy code.

**Interfaces:**
- Consumes: accepted S1 `s2resolve::Resolve`, proven `s2fn::RuntimeBinding::Create`, checked binding lifecycle.
- Produces: the handle-based native bridge contract above and a bridge-owned injectable dispatch sink. Task 6 wires that sink to the real core export atomically with its Rust definition and C header declaration; Task 5 does not reference a missing core symbol or supply a successful no-op dispatcher.

- [ ] **Step 1: Write red physical-record tests**

Inject a fake resolver and the real runtime-CIF binding boundary. Test resolution order, module identity/address/ABI fingerprint, repeated equal declaration sharing, address/ABI conflict naming both canonical ids, failed resolve creating no record, unsupported ABI rejection before libffi allocation, target release refcounts, lazy hook acquire, one physical KHook registration for multiple logical consumers, removal completion before closure free, and no retarget operation.

The injectable resolver seam belongs to the bridge and defaults to S1's free `s2resolve::Resolve`; do not change S1 to make it mockable. Use a typed test dispatch sink for native component tests. Without a configured production sink, hook acquisition fails by name before installing a hook. Task 6 supplies the real sink; these native component tests do not claim core/JS dispatch proof.

Include `validated-call` fixtures proving candidate validators run before uniqueness and target validators run after one derivation.

Run:

```bash
bash scripts/test-engine-function-bridge.sh
```

Expected: FAIL with missing bridge symbols.

- [ ] **Step 2: Implement immutable target records**

Key the conflict map by `(module identity, logical live address)` and store one ABI fingerprint. Key reusable records by `(module identity, logical live address, ABI fingerprint)`. Each record retains S1 resolution/provenance, the runtime CIF binding, hook receipt/observation state, refcount, and active subscription count. `FunctionPrepare` parses normalized target/ABI data, calls S1 once, and returns an opaque monotonic target id.

- [ ] **Step 3: Implement calls and owner-only bypass**

`FunctionCall` resolves/copies projected inputs, validates receiver/entity/opaque liveness, pushes `{target_id, owner_token}` on a TLS stack, calls the **live hooked target** through `RuntimeBinding::Call`, and pops with RAII. The callback supplies the suppressed owner token to its dispatch sink, wired to core in Task 6; it does not bypass KHook or other subscribers. Nested calls preserve outer state.

- [ ] **Step 4: Implement lazy hook acquisition and receipt state**

First logical subscriber configures the four-closure stock-KHook registration. Registration success is `Pending`; only Observe changes it to `Active`. A registration failure is named and leaves call availability separate. Last subscriber begins checked retirement; last target reference releases CIF/closures only after removal completion.

Run:

```bash
bash scripts/test-engine-function-bridge.sh
python3 scripts/gen-engine-ops.py --check
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release -DS2_CORE_LIB_DIR=debug
cmake --build build/shim -j
```

Expected: PASS; no S1 source changes and no second resolver/detour backend.

- [ ] **Step 5: Commit the native shared-target service**

```bash
git add shim/src/engine_function_bridge.* shim/tests/engine_function_bridge_test.cpp \
  scripts/test-engine-function-bridge.sh core/engine-ops.jsonc core/src/engine_ops.generated.rs \
  shim/include/s2script_engine_ops.generated.h shim/src/s2script_engine_ops_fill.generated.inc \
  shim/CMakeLists.txt scripts/ci-native.sh
git commit -m "feat: add shared native engine function targets"
```

---

### Task 6: S2-EF-06 — Build the core registry, projections, and executable adapter fan-out

**Files:**
- Create: `core/src/engine_functions/registry.rs`
- Create: `core/src/engine_functions/binding.rs`
- Create: `core/src/engine_functions/projection.rs`
- Create: `core/src/engine_functions/policy.rs`
- Create: `core/src/engine_functions/package_adapter.rs`
- Create: `core/src/engine_functions/runtime.rs`
- Create: `core/src/engine_functions/legacy_acquire.rs`
- Create: `core/src/engine_functions/legacy_hud_click.rs`
- Create: `core/src/engine_functions/contracts/legacy.acquire.v1.json`
- Create: `core/src/engine_functions/contracts/legacy.hud-click.v1.json`
- Create: `core/src/v8host/function_adapter.rs`
- Modify: `core/src/v8host/engine_function_adapter_v8.rs`
- Modify: `scripts/test-engine-function-v8-adapter.sh`
- Modify: `core/src/engine_functions/mod.rs`
- Modify: `core/src/ffi.rs`
- Modify: `core/src/v8host.rs`
- Modify: `core/src/v8host/natives.rs` (coordinator-owned narrow bootstrap seam)
- Modify: `shim/include/s2script_core.h`
- Modify: `shim/src/engine_function_bridge.h` and `.cpp` (real core dispatch-sink wiring only)

**Allowlist:** These engine-function modules, the new core callback ABI and matching native sink wiring, and the narrow package-bootstrap V8 seam/test. No loader or public SDK/API edits. This task first owns the executable legacy adapters and canonical contract fixtures; Task 8 later wires v1 migration/facades to them.

**Interfaces:**
- Consumes: prepared immutable contract/provenance and native target handles.
- Produces: `prepare_owner`, `activate_owner`, `drop_owner`, `status`, call/subscribe/dispose operations, dispatch callback, projection registry, and executable built-in/package-JS adapter registry.

- [ ] **Step 1: Write red registry/policy tests**

Cover required versus optional resolve outcomes, exact owner-generation lookup, compatible projection fan-out, conflicting ABI refusal, exact mutating policy compatibility, coexistence of named adapter with generic POST and observe-only PRE, stable registration order, edits flowing to the next callback, domain-local Stop, observe-only after final edits, and generic strongest-action/first-return-at-strength folding.

For non-void suppression, test valid typed values and invalid bare/mismatched values. Invalid decisions must log and continue without inventing a return.

- [ ] **Step 2: Write red projection/lifetime tests**

Test copied strings/vectors; live/stale/null entity input and returned-pointer adoption; registered opaque handle kind/liveness; borrowed record access inside dispatch; read/write after callback, after epoch replacement, from another thread, and after `await` all fail before native access; mutation of an undeclared field/phase fails.

Run:

```bash
cargo test -p s2script-core engine_functions::
```

Expected: FAIL with missing registry/projection modules.

- [ ] **Step 3: Prove per-context package adapters with real V8 before the broad API**

Replace the S2-EF-01 test harness's local registration shim with the two production host-only bootstrap globals behind an unforgeable native package-bootstrap token. The register global derives `PackageInstanceKey { parent, package_owner }` and implementation manifest hash from that token. Registering the same semantic id/hash from owner A and owner B succeeds; repeating it inside A or changing its process-wide semantic hash fails. The subscription global requires the current instance and returns a ledgered subscription owned by its parent generation.

The focused integration fixture creates two real plugin V8 contexts that evaluate the same package bootstrap. Through a test-only private call global, owner A calls a real stock-KHook target with owner-only bypass while A's V8/package instance is busy. The native callback must select owner B's eligible instance, synchronously enter B, invoke B's wrapper through `SubscriberCursor`, and return the typed result before the engine call resumes; A's adapter and wrapper must not run. Repeat with nested calls, then unload/reload A and prove B remains registered while every A receipt/global is gone. A deferred callback, a native-only mock, or a dedicated third game context does not satisfy this test.

Run:

```bash
bash scripts/test-engine-function-v8-adapter.sh --stock-provider
```

Expected: PASS with parent/package generation keys, selected instance, busy/bypass exclusion, synchronous B entry, result, and per-instance teardown recorded. Failure stops before S2-EF-07 and returns to design review.

- [ ] **Step 4: Implement preparation and activation receipts**

`prepare_owner` validates permissions, adapter visibility/hash, policy compatibility, and resolves every function to a native target handle. Any required failure drops all candidate handles and returns Err. Optional failures remain in the receipt as named unavailable functions. `activate_owner` inserts the exact owner key and binding ids; it does not evaluate JavaScript.

- [ ] **Step 5: Implement generic projections and dispatch epochs**

Keep the active frame in thread-local Rust state only for the synchronous callback. Each V8-facing view operation later added by S2-EF-07 will carry `{binding_id, dispatch_epoch, field_index}` and use these helpers. Entity/opaque adoption is performed before V8 conversion; pointers are never stored in a JS object.

- [ ] **Step 6: Implement compatible policy domains and effective returns**

Group mutating/suppressing subscribers by exact adapter contract. Run state-changing callbacks in stable host registration order, then observe-only PRE, then let KHook/original/peers execute, then POST with KHook's effective return. Do not promise ordering against external peers. Dispatch skips only `owner_token` from the native TLS call frame.

- [ ] **Step 7: Implement executable adapter registration without a semantic DSL**

Register `generic.v2` publicly. Register compatibility ids `legacy.acquire.v1` and `legacy.hud-click.v1` with locked contract hashes and `InternalReserved` visibility; selection requires a trusted normalized compatibility input, never matching ABI shape. S2's compatibility implementations satisfy `DispatchAdapter` as temporary executable Rust callbacks; no core dispatch site branches on either id.

Create the two named implementation modules and exact canonical contract files in this task, with focused vote, timing, string-copy and hash tests. Existing v1 entry points remain on their current path until Task 8 connects the compatibility facade; do not claim that passing old v1 tests already proves v2 routing. Wire the bridge sink to `s2script_core_dispatch_function` in the same integration as the Rust export and C declaration, preserving the core-symbol gate and the real-V8 proof above. No placeholder core export or silent no-op sink may satisfy that gate.

Complete the reserved bootstrap natives `__s2_function_adapter_register` and `__s2_function_adapter_subscribe` described above. Unit-test that only a host-minted package bootstrap token can register; callbacks must be synchronous; duplicates are scoped to `(PackageInstanceKey,AdapterId)`; semantic hash conflicts are process-wide; implementation manifest hashes remain provenance only; instance selection excludes busy/unloading/caller-bypassed parents; subscriber cursor invocation switches to each subscriber context in stable order; and parent/package unload drains only the matching instance before disposing its callback globals. Register the generic `borrowed-record.v1` codec separately from its package-owned field schema/layout instance.

Run:

```bash
cargo test -p s2script-core engine_functions::
cargo test -p s2script-core gamedata_calls::
cargo test -p s2script-core gamedata_hooks::
cargo test -p s2script-core acquire::
```

Expected: PASS, including existing v1 behavior tests.

- [ ] **Step 8: Commit the engine-generic function registry**

```bash
git add core/src/engine_functions core/src/v8host/function_adapter.rs \
  core/src/v8host/engine_function_adapter_v8.rs core/src/v8host.rs core/src/v8host/natives.rs \
  core/src/ffi.rs shim/include/s2script_core.h shim/src/engine_function_bridge.h \
  shim/src/engine_function_bridge.cpp scripts/test-engine-function-v8-adapter.sh
git commit -m "feat: add engine function registry and policy fanout"
```

---

### Task 7: S2-EF-07 — Expose `Engine.function` with ledgered lifecycle and reload preparation

**Files:**
- Create: `core/src/v8host/engine_functions.rs`
- Create: `core/src/v8host/tests/engine_functions.rs`
- Modify: `core/src/v8host.rs`
- Modify: `core/src/v8host/natives.rs` (coordinator-owned)
- Modify: `core/src/plugin.rs`
- Modify: `core/src/v8host/lifecycle.rs`
- Modify: `core/src/loader.rs` (coordinator-owned activation seam)
- Modify: `core/js/prelude.js`

**Allowlist:** V8 bridge, ledger, lifecycle, and exact loader activation seam. No SDK/shim changes.

**Interfaces:**
- Consumes: core registry and generated SDK API.
- Produces: runtime `Engine.function`, available/unavailable bindings, subscription handles/status, exact ledger teardown.

- [ ] **Step 1: Write red in-isolate API tests**

Test optional discrimination/status/provenance, required available branch, supported/absent surfaces, call marshalling, mutable PRE access, observe-only readonly enforcement, typed suppression, POST effective return, lazy Pending→Active/Failed subscription status, explicit dispose idempotence, and captured binding/subscription failure after reload.

Declare the test file explicitly in `core/src/v8host.rs` with `#[cfg(test)]`, `#[path = "v8host/tests/engine_functions.rs"]`, and `mod engine_function_tests;`. The existing `v8host/tests.rs` is exposed as `frame_tests`, not `tests`; a new file alone is not compiled. Include a named regression `optional_binding_reports_unavailable_reason`, list the module's tests before running them, and require that regression plus a nonzero executed test count in every recorded result. Zero matching tests is a failed gate.

Run:

```bash
cargo test -p s2script-core v8host::engine_function_tests:: -- --list
cargo test -p s2script-core v8host::engine_function_tests::
```

Expected: FAIL because natives/prelude do not expose the API.

- [ ] **Step 2: Add owner- and generation-gated binding objects**

Install one host object in `__s2pkg_unsafe`. `Engine.function(name)` asks core for the calling context's binding id and returns a frozen facade whose getters/methods close over `{OwnerKey,binding_id}`. Do not expose target ids, hook ids, addresses, frame pointers, adapter ids, or cross-plugin lookup.

- [ ] **Step 3: Ledger every persistent resource**

Add `FunctionBinding` and `FunctionSubscription` resource variants. Record bindings during activation and subscriptions at registration. Explicit disposal releases one ledger entry; unload walks remaining entries in reverse order, detaches logical subscribers, releases physical target refs, and waits through the existing checked retirement contract.

- [ ] **Step 4: Complete two-phase reload behavior**

Before old unload, `apply_prepared` calls `prepare_owner` using archive plus current override snapshot. Required failure keeps old generation and disposes candidate native handles. On success, unload old, activate candidate, then evaluate JS. Factory/start failure calls `drop_owner(candidate)` and partial-ledger teardown; it does not resurrect old JS.

- [ ] **Step 5: Test lifecycle failure boundaries**

Add tests for required override/resolve failure preserving old generation, optional failure reaching Active with unavailable binding, candidate activation failure cleaning all handles/subscriptions, stale captured call unable to touch replacement, and reload rebinding to a new target record while the old record remains unchanged until teardown.

Run:

```bash
cargo test -p s2script-core v8host::engine_function_tests::
cargo test -p s2script-core loader::
cargo test -p s2script-core plugin::
```

Expected: PASS.

- [ ] **Step 6: Commit the public runtime surface**

```bash
git add core/src/v8host/engine_functions.rs core/src/v8host/tests/engine_functions.rs \
  core/src/v8host.rs core/src/v8host/natives.rs core/src/plugin.rs \
  core/src/v8host/lifecycle.rs core/src/loader.rs core/js/prelude.js
git commit -m "feat: expose ledgered engine function bindings"
```

---

### Task 8: S2-EF-08 — Preserve v1 behavior and add a structurally lossless migration command

**Files:**
- Create: `packages/sdk/src/engine-functions/migrate-v1.ts`
- Create: `packages/sdk/src/commands/migrate.ts`
- Create: `packages/sdk/test/engine-functions-migrate.test.mjs`
- Create: `core/src/engine_functions/compat.rs`
- Modify: `core/src/engine_functions/legacy_acquire.rs` and `legacy_hud_click.rs` only for v1 integration; preserve Task 6's semantic contract
- Read/test: `core/src/engine_functions/contracts/legacy.acquire.v1.json` and `legacy.hud-click.v1.json`, created and locked in Task 6
- Modify: `packages/sdk/src/build.ts` (coordinator-owned v1 normalize seam)
- Modify: `packages/sdk/src/commands/index.ts` (coordinator-owned)
- Modify: `packages/sdk/src/cli.ts`
- Modify: `core/src/gamedata_calls.rs`
- Modify: `core/src/gamedata_hooks.rs`
- Modify: `core/src/v8host.rs`
- Modify: `games/cs2/js/pawn.js`
- Modify: `games/cs2/js/ui.js`
- Modify: existing Acquire/HUD tests only where needed to prove unchanged public behavior

**Allowlist:** Migration/compatibility files and the named legacy adapter call sites. Do not relocate ownership to S3 yet or change public CS2 types.

**Interfaces:**
- Consumes: v1 gamedata, source references, generated declarations, generic function registry.
- Produces: `s2s migrate engine-functions`, v1-to-v2 internal normalization, and compatibility facades for `Engine.call`/`Engine.hook`.

- [ ] **Step 1: Write red migration success/ambiguity tests**

Success fixtures must zip `args`/`argNames`, carry every validator with its stage, join identical call/hook targets and ABIs, decode supported shapes, convert `bypassWith`, remove plugin-only `expose.ctx`, derive permissions, and emit a report that identifies success as lossless conversion of the *declared* contract only. A fixture with four declared scalar arguments must not gain an inferred fifth argument or receive a claim that its actual native ABI is complete. Neither v1 nor v2 has a general oracle for omitted native arguments.

Failure fixtures: missing names, empty validators, mismatched call/hook targets, lossy shape projection, unsupported ABI feature, ambiguous receiver hop, source usage that cannot prove timing/null behavior, and an existing destination file. Assert no output file is written on any ambiguity.

Run:

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test test/engine-functions-migrate.test.mjs
```

Expected: FAIL because the command is absent.

- [ ] **Step 2: Implement analysis-first migration**

Parse all inputs, build a complete migration report in memory, and write `gamedata/functions.jsonc` only if every function is structurally lossless relative to its declared v1 contract. Use atomic temp+rename and refuse overwrite without an explicit `--force` whose report still names replaced files. The report includes old names, new canonical ids, validator stages, permissions removed from package authoring, compatibility adapter selection, source references requiring manual edits, and the scope of its structural guarantee. It does not certify undeclared native arguments or impose a new attestation step on all authors. The known incomplete Ignite demo remains historical and unavailable on the current build; do not treat its four-scalar conversion as evidence of a valid v2 native call or repair it by replacing only the stale pattern.

- [ ] **Step 3: Normalize v1 through the same v2 intermediate representation**

For the deprecation window, `s2s build` may read `s2script.gamedata` only on the v1 path and convert its declared contract to `NormalizedBundle` before common validation/type generation/packing. Do not drop a validator or coerce an unsupported shape to make it pass. Archive summary marks `compatibilityInput: "v1"` for diagnostics. Common schema validation cannot discover arguments omitted from source; a successful v1 normalization must not be described as native ABI certification, including for the known truncated Ignite descriptor. This is a documentation and fixture boundary, not a name-specific runtime exception or alternate backend.

Preserve every v1 native `bool` argument/return as native `u8` plus bool projection. Never widen a bool return to `i32`: upper return-register bits are not part of the C++ bool result. Any other legacy small-integer shape remains an explicit unsupported migration unless separately added to the proven capability contract.

- [ ] **Step 4: Preserve `Engine.call` and `Engine.hook` facades**

Map v1 call/hook lookup to the v2 binding while keeping the old `call -> callable|null`, `hook -> subscribe|null`, status strings, timing, and null behavior for correctly declared supported contracts. `bypassWith` becomes explicit owner-only bypass policy on the paired normalized function. The stale Ignite recipe resolves unavailable on the current build; no compatibility claim promises a working call from that known incomplete ABI, even if an operator replaces only its target pattern.

- [ ] **Step 5: Lock acquisition and HUD adapters by name/hash**

Move shape-selected behavior behind exact ids:

```rust
const LEGACY_ACQUIRE_ID: &str = "legacy.acquire.v1";
const LEGACY_ACQUIRE_CONTRACT_HASH: &str =
    "69247dc63a6200f5bb8c8ff651b8dd632e8d6af9ad4a0b8d2933199800bc48c0";
const LEGACY_HUD_CLICK_ID: &str = "legacy.hud-click.v1";
const LEGACY_HUD_CLICK_CONTRACT_HASH: &str =
    "28c0c9833d521cadd4eb03254f63ef7dcb1cd8b728f48ebfa8835b82ff03ecdd";
```

Verify the two canonical contract files created in Task 6 remain byte-for-byte equal to the specified documents and assert their sorted-key/minified SHA-256 values. Wire v1 compatibility to Task 6's `DispatchAdapter` implementations. Acquisition preserves item-services-to-player mapping, synthetic result field, vote order, implicit deny, engine-result fold, and post override. HUD preserves current copied string boundary, re-entry behavior, and its compatibility callback labelled post running before the engine original. Do not add JSON fold kinds, parse these fixture documents into behavior, or `match adapter_id` in generic dispatch. Add a package-JS conformance fixture that registers the same callback contract through `__s2_function_adapter_register` and produces byte-for-byte equivalent decisions, proving S3 can relocate code rather than ask core for a new semantic primitive. Generic functions with identical ABIs must receive none of those semantics.

- [ ] **Step 6: Run compatibility tests and build old archives**

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test \
  test/engine-functions-migrate.test.mjs test/gamedata-*.test.mjs \
  test/cs2-engine-calls.test.mjs test/cs2-ui.test.mjs
cd ../..
cargo test -p s2script-core gamedata_calls::
cargo test -p s2script-core gamedata_hooks::
cargo test -p s2script-core acquire::
cargo test -p s2script-core v8host::engine_function_tests::
bash scripts/build-base-plugins.sh
bash scripts/check-plugins-typecheck.sh
```

Expected: PASS; correctly declared supported old base plugins/archives keep public behavior. The retained Ignite demo's historical observations do not count as a current-build call/ABI pass; its unavailable target and incomplete declared ABI remain documented separately.

- [ ] **Step 7: Commit migration and compatibility**

```bash
git add packages/sdk/src/engine-functions/migrate-v1.ts packages/sdk/src/commands \
  packages/sdk/src/cli.ts packages/sdk/src/build.ts packages/sdk/test/engine-functions-migrate.test.mjs \
  core/src/engine_functions core/src/gamedata_calls.rs core/src/gamedata_hooks.rs core/src/v8host.rs \
  games/cs2/js/pawn.js games/cs2/js/ui.js packages/sdk/test
git commit -m "feat: migrate legacy engine calls and hooks"
```

---

### Task 9: S2-EF-09 — Convert the worked example to one v2 declaration

**Files:**
- Create: `examples/engine-function-demo/gamedata/functions.jsonc`
- Create: `examples/engine-function-demo/package.json`
- Create: `examples/engine-function-demo/tsconfig.json`
- Create: `examples/engine-function-demo/src/plugin.ts`
- Create: `examples/engine-function-demo/README.md`
- Create: `packages/sdk/test/engine-function-demo.test.mjs`
- Modify: `examples/engine-call-demo/README.md` to label the retained v1 source and logs as historical
- Retain: other `examples/engine-call-demo/**` source as the historical v1 example
- Modify: `scripts/check-examples-coverage.sh` only if discovery requires it

**Allowlist:** Worked example, historical v1 README correction, and its contract test. No runtime changes.

**Interfaces:**
- Consumes: v2 SDK/runtime API and the audited scalar `CBasePlayerPawn_CommitSuicide` base target.
- Produces: one ordinary plugin that declares typed call+PRE+POST only in `gamedata/functions.jsonc` and exercises a deliberate per-function failure.

- [ ] **Step 1: Write the red example contract test**

Assert package.json has no `s2script.gamedata`, engine permissions, `requiresGamedata`, or generated include; archive has derived permissions and normalized member; source uses only `Engine.function("commitSuicide")`; generated types reject wrong call args, invalid PRE mutation, bare non-void suppression in a returning fixture, and POST return override. Assert the v1 Ignite source remains present, its README labels the old logs as historical, states the current target is unavailable and the declared ABI omits the native Vector, and makes no current safe/working claim.

- [ ] **Step 2: Author the v2 CommitSuicide function**

Use a source recipe and complete validator derived from the audited exact deployed binary, not the illustrative pattern in the spec or the old Ignite recipe. Declare local `commitSuicide` with `receiver: {type:"entity"}`, parameters `{explode:bool}` and `{force:bool mutable:pre}`, return `void`, and surfaces call/pre/post. The direct target is `CBasePlayerPawn_CommitSuicide(this, bool, bool)`; verify the base-class entry and scalar ABI. It invokes the base implementation directly on the pawn, as existing `pawn.slay()` does. It does not claim virtual `CCSPlayerPawn` override behavior: the audited forwarding thunk writes an additional pawn byte before jumping to the base body. Include a second stale-validator function that degrades independently.

- [ ] **Step 3: Exercise binding/status/subscriptions**

At start, print structured status/provenance and register PRE and POST, with PRE mutating `force` and POST observing the void call. Dispose/re-subscribe one handle. An explicit command then selects an actual live bot pawn, calls the same binding with `explode=false, force=true`, and proves that pawn was alive before and dead afterward. Do not trigger the kill at startup, substitute callback counts for the engine effect, or claim the direct base call reproduces the subclass override's extra side effect. The example must not depend on a package path or manual archive/type step.

Correct `examples/engine-call-demo/README.md` in this task: place the stale current-binary pattern and deliberately omitted trailing Vector caveat beside the descriptor, label old output and flame counts as historical observations on the old build, and remove present-tense instructions that imply `sm_ec_burn` is safe or operative now. Preserve the source and historical evidence. Explain that replacing only the pattern does not make its four-scalar ABI complete. Stage this README with the new example at the Task 9 commit checkpoint.

- [ ] **Step 4: Build and test**

```bash
cd packages/sdk
node --experimental-strip-types --no-warnings --test test/engine-function-demo.test.mjs
cd ../../examples/engine-function-demo
npm run build
```

Expected: PASS and `dist/_demo_engine-function.s2sp` contains manifest, plugin, and normalized functions.

- [ ] **Step 5: Commit the v2 acceptance example**

```bash
git add examples/engine-function-demo examples/engine-call-demo/README.md \
  packages/sdk/test/engine-function-demo.test.mjs \
  scripts/check-examples-coverage.sh
git commit -m "examples: demonstrate unified engine functions"
```

---

### Task 10: S2-EF-10 — Run integrated CI, live acceptance, and publish the handoff

**Files:**
- Create: `docs/ENGINE_FUNCTIONS.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/INSTALL.md`
- Modify: `docs/PROGRESS.md` only after evidence is complete
- Modify: `README.md` if operator commands changed
- Modify: `scripts/ci-native.sh`, `scripts/ci-js.sh` only for final gate wiring

**Allowlist:** Documentation and final CI wiring. Fixes found by gates return to the owning earlier task rather than being hidden here.

**Interfaces:**
- Consumes: integrated S2-EF-01..09 result on top of accepted S1.
- Produces: reviewable acceptance evidence and the exact S3 handoff.

- [ ] **Step 1: Add static boundary and drift gates**

Gate generated ABI platform/atom bounds across C++/SDK/Rust, pinned libffi source/license/static-symbol checks, reserved adapter ids/semantic hashes, archive schema, derived permission mapping, core→game import boundary, absence of game strings in generic function modules, and absence of `s2detour`/private hook fallback in S2 paths.

Run:

```bash
python3 scripts/gen-engine-function-abi.py --check
python3 scripts/gen-engine-ops.py --check
bash scripts/check-core-boundary.sh
bash scripts/check-call-descriptors.sh
```

Expected: PASS.

- [ ] **Step 2: Run focused SDK/native/core gates**

```bash
( cd packages/sdk && npm test )
bash scripts/test-engine-function-abi.sh --stock-provider
bash scripts/test-engine-function-bridge.sh
bash scripts/test-plugin-function-overrides.sh
cargo test -p s2script-core
bash scripts/build-base-plugins.sh
bash scripts/check-plugins-typecheck.sh
```

Expected: all PASS with counts recorded.

- [ ] **Step 3: Run the full repository gates**

```bash
make ci-native
CI=1 make ci-js
```

Expected: PASS. If the environment lacks Docker for the final JS gate, record that single infrastructure limitation and run it in the live-gate environment before acceptance; do not mark S2 complete from the reduced local run.

- [ ] **Step 4: Prove operator override and reload behavior live**

Deploy the sniper artifact and v2 demo. Through RCON:

1. On an explicit command against a live bot, verify `commitSuicide` call/PRE/POST, typed `force` mutation, and observed alive-to-dead transition on the base target. Do not claim virtual override side-effect parity.
2. Add a valid namespaced override and reload; verify archive hash stays fixed, final target/provenance changes, and no archive rebuild occurs.
3. Add a stale-contract override; verify reload refuses and old generation still answers commands.
4. Add two conflicting files without `supersedes`; verify fail-closed. Add explicit `supersedes`; verify deterministic repair.
5. Force optional failure; verify Active+unavailable. Force required failure; verify old generation retained.
6. Trigger candidate JS start failure after activation; verify candidate cleanup and no old-generation claim.

Use:

```bash
bash scripts/test-engine-function-live.sh --docker docker/docker-compose.yml --rcon scripts/rcon.py --suite overrides,reload
```

Expected: all cases PASS with archive/base/override/final hashes in logs.

- [ ] **Step 5: Prove sharing, peers, safety, and cleanup live**

Load two plugin fixtures with the same address/ABI but different safe projections, plus the peer probe. Verify one physical KHook registration, compatible fan-out, explicit incompatible ABI refusal, owner-only bypass (caller skipped; other plugin/package/peer observed), nested re-entry, effective peer result in POST, stale borrowed/entity/binding/subscription refusal, repeated script reload and map transition. Leave the server running afterward; do not add a whole-process quit gate.

```bash
bash scripts/test-engine-function-live.sh --docker docker/docker-compose.yml --rcon scripts/rcon.py \
  --suite sharing,peer,lifetime,reload,map
```

Expected: PASS with deployed revision, sniper GLIBC manifest, Metamod/KHook identity, server build and actual runtime observations recorded under `.gate/engine-functions/<commit>/`. Preserve existing shutdown-only 139 as non-blocking diagnostic evidence; do not manufacture a passing terminal record.

- [ ] **Step 6: Document the operator/author contract and S3 seam**

Document authoring defaults, bounded Linux x86_64 scalar ABI contract/exclusions, generated API, migration command, default-deny permissions, override path/contract/supersedes rules, status/provenance, and update-day recovery. Explain that Ignite's four declared scalars could convert structurally but cannot establish a complete native ABI: its descriptor omits a trailing by-value Vector, the old recipe misses the current binary, and the v2 scalar ABI rejects aggregates. State the direct-base CommitSuicide example's subclass-forwarder caveat. State the S3 handoff exactly:

- keep `core/src/engine_functions/*` and `shim/src/engine_function_*` game-generic;
- preserve `OwnerKind::GamePackage`, `__s2_function_adapter_register`, `__s2_function_adapter_subscribe`, exact ids/hashes, subscriber cursor semantics, receipt/status/provenance, and owner-generation teardown;
- relocate `legacy.acquire.v1` and `legacy.hud-click.v1` executable callbacks into S3-owned game-package JavaScript without changing public behavior or adding a policy DSL;
- express ammo through generic ABI/projection and damage through the generic `borrowed-record.v1` codec with CS2 field schema/layout in package data, never a damage-named core codec or bespoke installer.

- [ ] **Step 7: Update progress only with real evidence and commit docs/gates**

```bash
git add docs/ENGINE_FUNCTIONS.md docs/ARCHITECTURE.md docs/INSTALL.md docs/PROGRESS.md \
  README.md scripts/ci-native.sh scripts/ci-js.sh
git commit -m "docs: publish engine function acceptance contract"
```

---

## Coordinator handoff and evidence ledger

At implementation start create a ledger with task id, owner, exact baseline SHA, prerequisite/integrated SHAs, file allowlist, status, commits, checks actually run, and evidence paths. Use statuses `planned`, `ready`, `running`, `awaiting integration`, `blocked`, and `verified`. A worker report must include changed files, interface deviations, host/unit/live checks with results, pending evidence, and any requested lower-layer change.

Integrate in DAG order. Review S2-EF-01's actual stock-provider evidence before allowing S2-EF-02+ work. Review normalized contract/hash/type names before dispatching consumers. After every shared-file integration, rerun its generated freshness check. Only one operator owns the Docker CS2 server and deployed artifacts during live gates.

S1 changes discovered during implementation return to the S1 owner as prerequisites. S3 consumes only the locked handoff above; S2 does not move CS2 layout/data or redesign package bootstrap. Do not check off S2-EF-10 or append a COMPLETE progress entry while any mandatory stock-KHook, full CI, live reload/override, map or real-client evidence is pending. Known whole-process shutdown-only 139 is non-blocking by the user's explicit acceptance change.
