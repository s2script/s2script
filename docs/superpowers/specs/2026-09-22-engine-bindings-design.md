# Shared engine bindings and stock KHook — design and stack contract

**Status:** Written design approved for implementation on 2026-09-22; no implementation or acceptance claimed by this document.
**Baseline:** PR [#221](https://github.com/s2script/s2script/pull/221),
`b4acdd5e7b64eb82e26db8fdd7018b683e10dd2d`.
**Decision:** Use one shared engine-binding service, simpler plugin-owned gamedata,
and game packages that own gameplay semantics. Breaking authoring changes are
allowed with an explicit migration path. This document specifies slice S1 and
the dependency contract for S1–S3.

**Execution:** [Implementation plan](../plans/2026-09-22-engine-bindings.md), with
task ownership, interface contracts, test commands and acceptance gates.

## 1. Why and scope

The current cutover replaces SourceHook, but the remaining inline intercepts use
`s2detour`; precache still writes a shared vtable slot. Built-in functions,
declarative calls/hooks, and SDKHooks also resolve targets through different paths.
Those paths can disagree about signatures, original vtable entries, and validation
after another framework has installed a hook.

Meanwhile, plugin authors describe calls and hooks separately, and hook shape IDs
also select acquisition and HUD behavior. Simplifying only the JSON would hide
these architectural problems rather than remove them.

The destination has five responsibilities:

| Component | Owns |
|-----------|------|
| Runtime | V8, plugin generations, scheduling, handles, permissions, subscriptions, ledger cleanup |
| Source 2 bridge | Engine interfaces, entity identity, schema access, native value conversion |
| Shared binding service | Resolution, ABI validation, native calls, KHook subscriptions, status and diagnostics |
| Game package | Gameplay models, field mappings, damage/acquisition/ammo behavior, game-facing API composition |
| Plugin | Private function definitions and subscriptions through the same shared facilities |

These are module boundaries inside the existing distribution initially. They do
not introduce a native extension loader, require a separate shared library per
capability, or promise that a new game never needs native interface support.

## 2. The stack and existing migration work

The new design PRs are documentation only and can be reviewed now. Their future
implementation slices form the following dependency chain above accepted PR A:

```text
#220 migration plan → #221 PR A → S1 shared bindings → S2 function authoring → S3 game packages
```

| Slice | Atomic result | Scope inherited from the old migration |
|-------|---------------|-----------------------------------------|
| S1 | Every remaining owned interception uses stock KHook; one validated resolver serves all consumers; existing script behavior remains intact | PR B/T9–T11 and PR C/T12–T13's precache and documentation obligations |
| S2 | One function declaration drives calls and hooks, with simpler authoring, plugin overrides, explicit ABI/policy separation and migration | Extends S1; it does not repeat the backend cutover |
| S3 | Game packages own gameplay policies and bootstrap from a package manifest; the shared runtime has no CS2-selected behavior | Architectural work beyond the original B/C plan |

S2's written contract will be `2026-09-22-engine-functions-design.md`; S3's will
be `2026-09-22-game-package-boundary-design.md`, both in this directory. They
are introduced by their own stacked design PRs, so S1 has no file dependency on
the later branches.

S1 replaces the old B/C dispatch map. The old
spec remains a reference for concrete invocation behavior and evidence cases;
do not execute two competing B/C plans. PR A's remediation and acceptance rules
remain authoritative. Each slice updates its own operator and architecture docs.

**Baseline and release prerequisite:** PR A acceptance is incomplete at this baseline;
required runtime and human/client evidence remains outstanding. A controlled
comparison reproduced the known shutdown-only SIGSEGV/139 with s2script enabled
and disabled on official Metamod 1467 and 1469, with MAM 1.6 in all four cells.
That does not establish fault attribution or a clean engine exit. The user has
explicitly made this known shutdown-only symptom non-blocking and directed us
to stop additional quit tests and shutdown investigation. Preserve the recorded
failure without relabeling it as a pass or treating a wrapper exit as child success.
Isolated development proceeds from the verified source/stock-host baseline.
Startup, plugin use, map transitions, script reload, peer coexistence and required
client observations remain release gates; callback lifetime and removal safety
tests remain mandatory. Later runtime findings can still require rework.

Each implementation slice must build, pass its applicable full CI and live gates,
and preserve a usable release at its tip. Stacking does not make an incomplete
parent acceptable. A dependent PR may be drafted while its parent is reviewed,
but release acceptance must be tied to the actual integrated commits.

## 3. Non-negotiable host and lifecycle contract

- Operators use the official AlliedModders Metamod distribution providing the
  required PLAPI/KHook API. No patched Metamod or KHook, private upstream fork,
  shadow provider, or separately linked detour engine is required.
- s2script consumes Metamod's KHook interface. KHook owns native interception;
  s2script owns validation, bindings, routing, and plugin subscriptions.
- Only `.s2sp` hot reload is required. The native s2script module stays resident;
  native upgrades happen on process restart. Native unload refusal must retain
  resident state safely, following the existing stock-host remediation contract.
- Distinguish resident native binding lifetime, entity/map routing lifetime, and
  plugin-generation subscription lifetime. Reload removes the outgoing owner's
  subscriptions through the ledger; it cannot invalidate another owner.
- No raw pointer survives a synchronous invocation in plugin code. Entities use
  generation-checked handles; borrowed native views expire at callback return
  and cannot cross `await` or be retained for later calls.
- Cleanup never frees callback storage while KHook or an in-flight invocation
  can use it. Shutdown disables dispatch and follows the proven stock-provider
  retirement sequence. A mock removal receipt alone is not proof of this.

## 4. S1 shared resolver

Built-in bindings, SDKHooks, declarative calls, and declarative hooks use the same
resolution/validation service. Existing author-facing descriptors and semantic
adapters remain intact during S1.

The service accepts a module identity, target recipe, platform, and validators.
Its successful result contains the logical live target address, module identity,
original-byte provenance, recipe, and validation receipt. Consumers do not repeat
module lookup, pattern scanning, RIP-relative decoding, or vtable-original lookup.

Resolution is ordered:

1. Select and identify the loaded module and its mapped ranges.
2. Obtain verified original instruction bytes, including bytes hidden by an
   existing KHook detour, with a checked mapping back to logical live addresses.
3. Enumerate candidates in the intended ranges and apply the recipe's
   candidate-stage validators before its uniqueness decision where required.
   In particular, `validated-call` validates all candidate call sites, requires
   exactly one survivor, then follows its E8 call. Its validator offsets describe
   the caller, not the derived callee. Direct recipes retain their own match rules.
4. Apply the named target derivation exactly once, validating its bounds and
   destination. Unknown strategies are errors, never an implicit direct address.
   For virtual recipes, obtain the original function using KHook's supported
   facility before instruction validation; do not validate a patched slot target.
5. Run the recipe's target-stage validators against original instruction bytes
   with live logical addresses; validate live target ranges and ownership too.
6. Preserve candidate/target validation stages in the normalized recipe and
   diagnostics. Do not relocate a validator across derivation as a cleanup.
7. Return a named success or failure before attempting registration or a call.

KHook's first-match signature lookup alone does not establish uniqueness. The
implementation must retain s2script's multi-match checks and semantic validators.
The original-byte provider must prove module/build identity and translation; an
unverified file on disk is not an acceptable substitute for loaded-module facts.
If identity or original bytes cannot be established safely, fail the affected
descriptor with a reason rather than scan a peer's jump stub as original code.

Caching is scoped to the module identity, normalized recipe, validation inputs,
and effective gamedata revision. Descriptor diagnostics retain owner/name and
provenance even when resolution work is shared. Failed lookups do not poison an
unrelated owner. A changed image or effective descriptor invalidates its receipt.

S1 does not invent a second KHook target catalog. Any s2script bookkeeping is a
consumer of Metamod's process-wide catalog and must be compatible with peer
registration before or after s2script.

## 5. S1 interception and preserved behavior

Move the declarative engine-hook backend and named CBaseEntity_TakeDamageOld, HostSay,
FireOutputInternal, and ProcessUsercmds intercepts to checked stock KHook bindings.
Move precache to the supported KHook virtual path, proving its slot and receiver
semantics. Delete production `s2detour` installation and direct owned-vtable
writes once their replacements pass acceptance.

The independently audited damage ABI is `void(victim*, mutable damageInfo*,
optional damageResult*)`. This corrects the inherited four-pointer/integer-return
assumption: retain Ignore callbacks, schema-based damage fields and scoped victim/info;
forward optional result storage unchanged. The internal recipe key is
`CBaseEntity_TakeDamageOld`, with exact entry and semantic diagnostic validation.
The retired `DispatchTraceAttack` pattern must not alias this target. Controlled
output forwarding and static identity gates do not replace actual damage delivery.


Precache may not silently remain a fallback private patch. If the stock API
cannot preserve its required behavior, S1 stays incomplete and the concrete
limitation is brought back for a design decision. Do not patch upstream to turn
the gate green. This tightens the old PR C option that allowed retaining a slot
write while calling the migration complete.

Carry forward the old spec's invocation contracts, including:

- In-place native object edits and usercmd neutralization use the appropriate
  Ignore behavior; by-value argument mutation uses Recall as required.
- Acquisition preserves its explicit votes, denial values, engine-result fold,
  and post override. S1 does not reinterpret it as ordinary numeric voting.
- FireEvent keeps its explicit original-call and suppression behavior.
- The legacy HUD callback ordering remains unchanged, including the existing
  callback labelled post that runs before the original. It must not acquire
  generic post-return meaning accidentally during backend replacement.
- Original-call/bypass behavior, effective results from peers, nested callbacks,
  receiver filtering, and SDKHooks phase routing retain their tested semantics.

KHook registration acceptance is distinct from observing a real callback. Keep
Pending/Active/Failed status and observation evidence; do not use fake engine
pointers, synchronous callback-time destruction, or polling merely to claim Active.
Do not assume callback registration order determines execution order.

## 6. Boundary required by the later slices

S1 may retain the current five hook shapes and their behavior. S2 must separate
three different facts before providing a generic authoring interface:

1. **ABI:** machine-level parameter and return representation and calling convention.
2. **Projection:** generic conversion between native values and safe JS values.
3. **Policy:** game-specific vote folding, receiver selection, and event meaning.

An ABI matching today's acquire or HUD signature must never implicitly activate
that game behavior for a plugin-defined function. S2 translates old descriptors
into explicitly named compatibility adapters; S3 relocates their game ownership.
This is a real dependency: postponing all policy separation until S3 would make
S2's supposedly generic functions inherit hidden CS2 behavior.

KHook's template helpers are not a runtime FFI generator. S2 must first prove a
bounded ABI adapter against the actual pinned, unmodified provider and publish
its support matrix. This stack does not promise arbitrary C++ signatures,
struct-by-value marshalling, varargs, or extra platforms.

## 7. S1 acceptance

Evidence is recorded with baseline/result commit, host and Metamod identities,
commands, actual observations, and pending or failed cases. Required unavailable
evidence remains pending. The implementation plan will give executable commands
and fixture ownership; these are its mandatory outcomes:

- Resolver tests cover wrong module identity, range/translation failures,
  uniqueness, target derivation, validators, detoured bytes, and patched vtables.
- Real stock KHook fixtures prove both peer load orders and actual callbacks;
  mocked bookkeeping tests alone cannot establish coexistence.
- Declarative and named hooks cover mutation, suppression, return observation,
  explicit original calls/bypass, nesting, and existing acquisition/HUD semantics.
- Precache works across map transitions with peer ownership and no private slot
  write. SDKHooks routing survives entity deletion and index reuse.
- Repeated `.s2sp` reload removes only the outgoing generation. No callback
  enters a disposed V8 context. Preparation/validation failures preserve the
  running generation; failures after replacement activation begins clean up safely
  under the existing loader contract. S1 does not promise transactional rollback
  of arbitrary JavaScript startup effects or redesign activation.
- Full native/JS CI passes; the deployable sniper release includes the default
  plugins and passes the relevant live CS2 regression gates.
- Startup, map transitions, required restarts and normal plugin use pass on the
  integrated release. Leave the tested server running. The known shutdown-only
  SIGSEGV/139 is diagnostic and non-blocking; no separate quit gate is required.
  Preserve any incidentally captured child status without substituting the
  container wrapper status. Fixture-owned removal and peer-survival proof remain
  required independently of whole-process exit.
- Production interception inventory contains no remaining `s2detour` installer,
  private interception provider, or direct shared-vtable patch owned by s2script.
  Dependency diff and deployed host identity establish that upstream is stock.

## 8. Work ownership for the dynamic workflow

The implementation plan must define stable work-package IDs and exact dependency
commits. Resolver API/provider work precedes consumer migrations. The coordinator
owns integration, shared files and release readiness; workers receive bounded
file allowlists and return commits plus evidence, not unverified completion marks.

Natural ownership boundaries are resolver/provider + tests; declarative binding
integration; named shim adapters; SDKHooks; and acceptance tooling/documentation.
No CODEOWNERS or nested AGENTS ownership rules were found in this baseline beyond
the root guidance. Keep a single writer for `s2script_mm.cpp`, CMake/gate wiring,
and shared fixture files, or serialize their integration by reviewed sections.

Use stronger reasoning for ABI/lifetime and interface review; routine inventory,
documentation and test execution can use lighter workers. One designated operator
owns the live server and artifact installation. Other workers may build fixtures,
but do not deploy, restart, or alter the server during an evidence run.

## 9. Alternatives and references

Changing JSON alone was rejected because it preserves duplicate resolution and
signature-selected game behavior. A universal C++ FFI or new extension ecosystem
was rejected as unnecessary scope. The selected design copies SourceMod's useful
separation between runtime facilities, game configuration, calls/hooks, and game
extensions without copying every historical mechanism.

- [SourceMod SDKTools](https://wiki.alliedmods.net/SDKTools_(SourceMod_Scripting)):
  engine calls use gamedata with explicitly described call signatures.
- [DHooks](https://wiki.alliedmods.net/DHooks_(SourceMod_Scripting)):
  function descriptions and plugin-owned dynamic hooks are established patterns.
- [SourceMod GameConfigs](https://github.com/alliedmodders/sourcemod/blob/master/core/logic/GameConfigs.cpp):
  game/platform selection and custom configuration layering.
- [Pinned KHook interface](https://github.com/Kenzzer/KHook/blob/1e200e4cc8e0badcb7cf941525268d6977f6a4e6/include/khook.hpp):
  the stock provider contract against which feasibility must be demonstrated.
- [Existing migration invocation contracts](2026-09-14-khook-migration-design.md)
  and [stock-host decision](2026-09-15-khook-stock-host-decision.md).

Audit starting points are `shim/src/engine_calls.cpp`, `engine_hooks.cpp`,
`sdkhooks_vp.cpp`, `s2script_mm.cpp`, `call_validate.cpp`, `sigscan.cpp`, and
`core/src/gamedata_hooks.rs`. These are observations of the stated baseline,
not assertions about an implementation that has not landed.
