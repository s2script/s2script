# KHook migration — design spec

> **Lifecycle clarification:** Use [stock Metamod with resident s2script](2026-09-15-khook-stock-host-decision.md). Only `.s2sp` hot reload is required; the native shim updates on server restart. The [current remediation spec](2026-09-14-khook-pr-a-remediation-design.md) supersedes conflicting native hot-reload or private-host assumptions. Safe process shutdown remains required.

**Status:** Revised after source review (2026-09-14); implementation and live acceptance remain pending — planning companion is `docs/superpowers/plans/2026-09-14-khook-migration.md`.
**Audience:** shim / core / operator-docs maintainers.
**Execution:** The [implementation plan](../plans/2026-09-14-khook-migration.md#dynamic-subagent-execution-contract) defines dependency barriers, file ownership, worker handoffs and evidence for the user's dynamic subagent workflow. Worker packages may be scheduled independently within those barriers; the three PR slices remain atomic.
**PR A review amendment:** Read the [remediation spec](2026-09-14-khook-pr-a-remediation-design.md) and [plan](../plans/2026-09-14-khook-pr-a-remediation.md) for the review of `57a329b7`. They supersede the provider-lifetime, installed-host verification and acceptance assumptions below. Use stock PLAPI 18 Metamod and the amended script-reload, artifact-verification and shutdown requirements; PR B remains blocked until PR A acceptance completes.
**Primary sources:** [Metamod PR #223](https://github.com/alliedmodders/metamod-source/pull/223) (merged 2026-09-08); [Kenzzer/KHook](https://github.com/Kenzzer/KHook) at pin `1e200e4`; inventory in `docs/superpowers/specs/2026-09-14-khook-vs-sourcehook-research.md`.
**Builds on:** `docs/ARCHITECTURE.md` §1 (issue #215 / one detour per engine function); `docs/re-strategy.md`; declarative inbound hooks (`docs/superpowers/specs/2026-08-02-declarative-inbound-hooks-design.md`).

---

## 1. Why

On 2026-09-08 Metamod:Source **deleted SourceHook** and replaced it with **KHook**. Plugin API **17 no longer loads** (`PLAPI_MIN_VERSION` is now 18). Our vendored Metamod is still `26c03fa` / 2.0.0.1403 (PLAPI 17). The next `mmsdrop/2.0` operator refresh will refuse `s2script.so`.

That is the load-blocking reason.

The architectural reason is older and already written down: `docs/ARCHITECTURE.md` cites Metamod issue [#215](https://github.com/alliedmodders/metamod-source/issues/215) — competing CS2 frameworks each ship a private detour engine and fight over the same addresses. PR #223 **is** Metamod’s answer to #215: one SafetyHook (or vtable-slot) trampoline per address, callbacks multiplexed inside KHook. s2script’s thesis (“the core owns every engine touchpoint”) now has a layer above it. If we keep `s2detour` on addresses other Metamod plugins also hook, **we become the conflicting party**.

This spec decides how we move. It does not implement.

---

## 2. What we are moving off, and what we are moving onto

**Today (three intercept engines in the shim):**

| Engine | What it patches | Sites |
|--------|-----------------|-------|
| SourceHook (`SH_ADD_HOOK` / `SH_ADD_MANUALHOOK`) | Interface vtables + per-entity manual VP | 15 interface hooks + 14 SDKHooks kinds |
| `s2detour::Install` | Function prologues (5-byte E9 or 14-byte FF25) | `DispatchTraceAttack`, `HostSay`, `FireOutputInternal`, `ProcessUsercmds`, declarative `S2_HookInstall` |
| `WriteVtableSlot` | One class-vtable pointer | `OnPrecacheResource` (prologue refused by `s2detour`) |

**After (one intercept engine, plus two leftovers that are not hooks):**

| Engine | What it patches | Sites |
|--------|-----------------|-------|
| KHook `Virtual` | Shared vtable slot + this-pointer filter | Every current `SH_ADD_HOOK` / `SH_ADD_MANUALHOOK` |
| KHook `Function` / `Member` | SafetyHook inline via Metamod’s catalog | Every current `s2detour::Install` |
| KHook `Virtual` (or keep `WriteVtableSlot` until proven) | Class vtable slot | Precache |
| Unchanged | `IEntityListener` add/remove | Entity create/spawn/delete — not a detour |

KHook itself does not live in our `.so`. Metamod links `libkhook` + SafetyHook. We compile against `khook.hpp` and receive `KHook::IKHook*` from `ISmmAPI::GetDetourInterface` in `PLUGIN_SAVEVARS()`.

---

## 3. Approaches

### A — SourceHook cutover only (keep `s2detour`)

Bump Metamod past PR #223. Replace every `SH_*` site with `KHook::Virtual`. Leave `s2detour` and `WriteVtableSlot` alone.

- **Pros:** Smallest change that loads on PLAPI 18. GameFrame / clients / events / SDKHooks come back first.
- **Cons:** Re-creates #215 on every sig-scanned address (damage, chat, I/O, usercmd, declarative hooks). Two trampoline allocators (`s2detour` + SafetyHook) can double-patch the same prologue. Does not match the thesis in `ARCHITECTURE.md`.

### B — Holistic KHook unification (recommended destination)

Do A, then move every `s2detour::Install` to `KHook::Function`/`Member` so **every intercept we own goes through Metamod’s catalog**. Precache becomes `KHook::Virtual` if the slot rewrite is equivalent; otherwise it stays a named leftover with a reason.

- **Pros:** One detour per address process-wide. Other KHook consumers (future CSSharp / Swiftly / CS2Fixes on the same Metamod) compose with us instead of crashing us. `LookupSignature` sees original bytes. Matches #215 and our own architecture doc.
- **Cons:** Larger. SafetyHook’s relocator, not ours, decides “unrelocatable prologue”. Our named `s2detour` refusals (arg-width, stolen-byte decode) must be re-homed *before* `SetupHook`, not inside it.

### C — Shim adapter / dual-build

Invent `s2::Hook` that talks SourceHook on PLAPI 17 and KHook on PLAPI 18, or ship two `s2script.so` artifacts.

- **Pros:** Soft operator ramp.
- **Cons:** Metamod will not load API 17 on new hosts (`PLAPI_MIN_VERSION` 18). Dual-build doubles the live gate. The PR author rejected dual-framework mode for the same reason we should: it preserves the conflict. Rejected.

**Decision:** B is the destination. Ship it as a **three-PR stack** so each merge loads and is testable:

| PR | Delivers | Loads on post-223 Metamod? |
|----|----------|----------------------------|
| **A** | Pin Metamod + checked KHook virtual hooks + generated licenses + operator compatibility floor + durable live-host refresh | Yes. `s2detour` still private. |
| **B** | `s2detour` sites → KHook `Function`/`Member`; keep `s2detour` only as a test-only relocator or delete it | Yes. Unification. |
| **C** | Precache decision and broader architecture/build documentation | Yes. Cleanup; A already carries its installation prerequisites. |

PR A is not optional and is not split: once the submodule moves, `SH_*` does not compile.

---

## 4. Architecture after the move

```
                    CS2 engine function / vtable slot
                              │
                    KHook capsule (one per address)
                     PRE callbacks (all Metamod plugins)
                              │
                    original  (unless any PRE Supersede)
                              │
                     POST callbacks
                              │
              ┌───────────────┴───────────────┐
              │  s2script shim handler        │  other MM plugins
              │  (one Action out)             │
              │         │                     │
              │  core multiplexer             │
              │  HookResult max-collapse      │
              │  among .s2sp plugins          │
              └───────────────────────────────┘
```

**Two composition layers, not one:**

1. **Process:** KHook. At pin `1e200e4`, `SaveReturnValue` accepts a **strictly higher** action: `Ignore < Override < Supersede`. The first return wins a tie, but a later `Supersede` replaces an earlier `Override`. All PRE callbacks still run. POST callbacks can affect the eventual return too; our POST observes the current effective result at its place in that chain, not a result frozen against later peer callbacks. See [the pinned implementation](https://github.com/Kenzzer/KHook/blob/1e200e4cc8e0badcb7cf941525268d6977f6a4e6/src/detour.cpp#L365), which is more precise than the README's first-wins shorthand.
2. **Runtime:** s2script multiplexer. Unchanged `HookResult` contract among TypeScript plugins.

`ARCHITECTURE.md` §1 must be rewritten to say this. The product is still “plugins compose instead of compete” — the *detour* is now Metamod’s, the *JS contract* is ours.

**Damage is a semantic adapter, not separate hook infrastructure.** Damage needs
borrowed typed arguments, writable PRE/read-only POST and victim-liveness checks.
Installation, registration state and teardown use the same infrastructure as all
other hooks. This cutover preserves the existing dedicated dispatch/view adapter;
a subsequent descriptor/borrowed-view slice should remove that duplication.
Game-specific field mappings belong in the game package/gamedata. Ammo reads and
writes use schema accessors; an intercepted ammo-consumption event uses the shared
hook system with a typed view, not another private installer.

**What does not move:**

- JS API, `HookResult`, ledger, entity books, gamedata ownership, `s2s` CLI.
- `IEntityListener` (not a detour).
- Schema / sigscan / RTTI validation requirements. They still run *before* patching; PR B changes their byte source to a verified original module image so an existing KHook patch does not hide a signature or defeat prologue validation (§5.7).
- Core Rust tests. They never called SourceHook.

---

## 5. Mapping rules (locked)

### 5.1 Result protocol

| Today | KHook | When we use it |
|-------|-------|----------------|
| `RETURN_META(MRES_IGNORED)` | `return { KHook::Action::Ignore }` | Default notify / “let original run” |
| `RETURN_META_VALUE(MRES_IGNORED, v)` | `return { KHook::Action::Ignore, v }` | Typed hooks that do not override |
| `RETURN_META(MRES_SUPERCEDE)` / `>= HookResult.Handled` | `return { KHook::Action::Supersede }` | Drop the original (usermsg, commands, FireEvent after re-call) |
| `RETURN_META_VALUE(MRES_SUPERCEDE, v)` | `return { KHook::Action::Supersede, v }` | Same, with a return |
| `RETURN_META_VALUE_NEWPARAMS(MRES_IGNORED, v, mfp, (args))` | `KHook::Recall(mfp, {Ignore, v}, this, args…)` then `return { Ignore, v }` | Voice `SetClientListening` only, today |
| `SH_CALL(ptr, mfp)(args)` | `KHook::CallOriginal(mfp, ptr, args)` or `member.CallOriginal(ptr, args)` | FireEvent re-fire |

Do not use a universal `Changed` mapping. For pointed-to objects (`IGameEvent`, `CTakeDamageInfo`), edits are already in-place and `Ignore` is correct. For by-value declarative arguments, `Recall` must forward the edited values. For `CanAcquire`, `Changed` is a return-value vote that must be folded with the engine result; a POST `Override` may be required. Preserve the implicit `InvalidItem` denial for `Handled` without a vote (§5.4).

`MRES_HANDLED` is unused in our shim. Do not invent a use.

### 5.2 Interface hooks → `KHook::Virtual`

Pattern (matches Metamod `samples/s2_sample_mm/src/plugin.cpp`): construct with the MFP + callbacks; `Add` only after `PLUGIN_SAVEVARS()`. The ctor calls `GetVtableIndex` only — it does not patch.

`s2script_mm.h` cannot hold `KHook::Virtual<ISource2Server, …>` (`META_NO_HL2SDK`, incomplete `eiface.h`). Put one file-static struct in `s2script_mm.cpp` **after** the SDK includes and **after** `PLUGIN_EXPOSE` (so `&g_S2ScriptPlugin` is declared), using the official context ctor:

```cpp
struct InterfaceHooks {
    S2CheckedVirtual<ISource2Server, void, bool, bool, bool> gameFrame;
    InterfaceHooks()
      : gameFrame(&ISource2Server::GameFrame, &g_S2ScriptPlugin,
                  &S2ScriptPlugin::Hook_GameFramePre,
                  &S2ScriptPlugin::Hook_GameFramePost) {}
};
static InterfaceHooks g_hk;   // after PLUGIN_EXPOSE — same-TU init order
```

`GameFrame` is **one** checked virtual hook with both PRE and POST, then **one** registration. Today that is two `SH_ADD_HOOK`s. `S2CheckedVirtual` is the shim-owned checked binding specified in §5.6; it uses KHook's typed callbacks and exposes the registration receipt. A bare `Virtual::Add` returns void and is insufficient for a success flag.

Handler signatures gain the interface pointer as the first argument:

```cpp
KHook::Return<void> Hook_GameFramePre(ISource2Server*, bool simulating, bool first, bool last);
```

Lazy install (`s2_request_hook`) becomes `Add` / `Remove` on the same object. Do not construct `KHook::Function`/`Member` with an **address** before `PLUGIN_SAVEVARS` (`Configure` → `SetupHook`). Virtual-with-MFP is safe at static init.

Before each `Add`, `if (KHook::GetVtableIndex(&ISource2Server::GameFrame) < 0)` print `[s2script] KHOOK FAIL: GameFrame GetVtableIndex=-1` and skip. `Virtual::Configure(mfp)` swallows `-1`.

`AddGlobal` is reserved for “every instance of this vtable” (we do not need it for interface singletons). `AddContext` is not the primary bind.

PRE vs POST is **which callback pointer is non-null**, not a bool. PRE-only: `nullptr` POST (sample `ClientCommand`). POST-only: `nullptr` PRE (`CheckTransmit`, `ClientVoice`, `StartupServer`). Both: `GameFrame`.

### 5.3 SDKHooks manual VP → `KHook::Virtual` + `Configure(slot)`

Today: `SH_DECL_MANUALHOOK` with zeros, then `SH_MANUALHOOK_RECONFIGURE(name, slot, 0, 0)` after the RTTI walk. PRE and POST are **two** SourceHook installs (`VpKey{p, kind, post}`).

After: one `KHook::Virtual<CEntityInstance, …>` per kind, constructed at file scope with the existing free-function PRE/POST (`Virtual(fnCallback, fnCallback)` — index still `INVALID` until configure):

```cpp
S2CheckedVirtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(&Hook_StartTouch, &Hook_StartTouchPost);
// in S2SdkhooksVpLoad, after the RTTI+sig slot walk — BEFORE any vp_add:
g_hkStartTouch.Configure(slot);
```

`s2_sdkhook_vp_add` still takes `post`. `Add(entity)` on the **first** phase for that entity+kind. `Remove(entity)` only when **both** PRE and POST rows for that entity+kind are gone. Naive per-`post` `Remove` would drop the other phase’s this-filter.

Do **not** expect `Remove` to restore the original vtable slot. The trampoline stays for the process lifetime of that `Virtual` object. JS fan-out in `sdkhooks.rs` already filters by host-id.

Cost model change (accepted): this is VP-cost. Every Think on every pawn of that class enters the JIT when any pawn is hooked. Do not rebuild SourceHook’s per-instance vtable clone.

### 5.4 Inline detours and invocation state (PR B)

Use typed KHook callbacks and the checked binding from §5.6. Construct callback
objects without an address; register after `PLUGIN_SAVEVARS` and validation.
The KHook callback still returns `Return<T>`; the existing detour thunk itself
is not a valid KHook callback.

**Declarative hooks:** preserve all five `S2HookShape` signatures, with one
binding per hook id (maximum 64). `S2_HookInstall` keeps its existing ABI:
**0 for accepted registration, -1 with a named reason for failure**. This is
different from the SDKHooks VP add ABI (nonzero success). Accepted registration
is recorded as Pending until our first real callback, not as proven delivery.

Each declarative PRE creates an invocation record containing its `ArgView`,
bypass flag, and (for CanAcquire) plugin vote/result. A scoped guard pushes it on
a per-id thread-local stack and restores the enclosing active view on exit.
PRE invokes **synchronous `KHook::Recall`** with the edited arguments and the
local Ignore/Supersede decision. The remainder of the chain, including POST,
finishes before Recall returns, so the view stays alive. Keep opaque arguments
at their declared 64-bit width. POST uses that same record; do not retain a
pointer to a returned PRE frame. A bypassed invocation skips both our PRE and
POST dispatch while the engine and peer hooks retain their normal behavior.

For `this_i64_i32_i64` (CanAcquire):

- Preserve the separate `voted` bit. Continue supplies no vote; Changed supplies
  a vote and still calls the engine; Handled/Stop skips the engine and uses
  `InvalidItem` (1) when no result was written.
- In POST, query `WasOriginalFunctionSkipped()` before reading the original
  return. If it ran, combine the saved plugin vote with the engine result using
  the existing `S2Hook_MostRestrictiveAcquire`.
- Only when a saved vote changes the original result, submit local `Override`
  with `KHook::ManualReturn` **before** querying
  `GetCurrentReturn<int32_t>()` and dispatching our read-only POST. Merely
  returning Override at the end would publish a stale value to JS POST.
- Preserve KHook peer precedence: an existing higher/equal-priority peer result
  may win. POST exposes the current effective value and actual skipped state
  at our callback; subsequent peer POST callbacks can still change the return.
- Retain the existing HUD-click shape's copied text, controller receiver,
  suppression and notification ordering. Its existing `S2Hook_DispatchPost`
  occurs before the engine original; do not move it solely because KHook has a
  phase named POST. Notify once from PRE before Recall and leave the KHook POST
  callback for that shape without JS dispatch.

**Named hooks:**

| Site | Signature | Required behavior |
|------|-----------|-------------------|
| DTA | `int64_t(void*, void*, void*, void*)` | PRE and POST each save/restore both damage-info and victim pointers around their own dispatch. PRE returns Ignore; POST returns Ignore. Never call the engine with sentinel/dummy pointers. |
| HostSay | `void(void*, void*, bool, int, const char*)` | Supersede when chat suppresses, otherwise Ignore. |
| FireOutputInternal | `void(CEntityIOOutput*, CEntityInstance*, CEntityInstance*, const CVariant*, float, void*, char*)` | Supersede for result >= 2, otherwise Ignore. |
| ProcessUsercmds | `int(void*, void*, int, bool, float)` | Always Ignore after any in-place command neutralization; engine processing must run. |

DTA's old install-time sentinel is safe only under exclusive trampoline
ownership and is **removed** in PR B. Pending/failed registration can reach the
real function, and even a ready registration does not prevent peer PRE
callbacks from seeing the sentinel. Prove diversion with a controlled native
function fixture and prove live damage with a valid engine invocation.
The periodic core synthetic damage self-test is separate liveness evidence;
it does not prove the engine detour.

### 5.5 Precache (PR C)

Prefer a checked `Virtual` binding on the RTTI vtable plus validated index.
Use `AddGlobal` with a temporary one-word vtable holder for registration; retain
the vtable value, never the temporary holder pointer. Construct an equivalent
holder when removing the global filter. PRE saves/restores the manifest around
dispatch and returns Ignore.

Use first-fire evidence on a real map precache cycle. Pending is not failure.
If reverting to `WriteVtableSlot`, first remove/drain the KHook registration and
verify no other KHook owner holds that slot; never overwrite another consumer's
trampoline. Otherwise degrade precache by name and leave the slot alone.
The private fallback is a documented coexistence limitation.

### 5.6 Registration receipts, readiness and teardown (PR A foundation)

Add `shim/src/khook_binding.h`, containing `S2CheckedFunction<Ret, Args...>`
and `S2CheckedVirtual<Class, Ret, Args...>`. They are thin checked bindings over
the pinned typed KHook helpers, not an alternate detour engine or public API.
Inherit constructors and inspect the protected associated-id/maps under their
existing locks; isolate this pin-specific access in this header.

Registration returns a receipt with the actual `KHook::HookID_t`, a named
failure reason, and state `Failed | Pending | Active | Removing | Removed`.
`INVALID_HOOK` is Failed. A valid id is Pending; mark Active only when our
matching callback is observed. Hook-state updates shared with KHook's removal
worker must be synchronized. Neither a non-null interface, `IsActive()` (which
only inspects filters), nor a missing log proves physical installation.

At this pin the typed helpers request `async=true`: an existing capsule can
queue insertion. The documentation's warning and the implementation differ;
read the implementation and test both new-capsule and shared-capsule paths.
Do not block the game thread polling for first fire. If a capability requires
validated first-fire state (such as transmit filtering), keep it unavailable
until validation passes. Preserve accepted registration bookkeeping so it can
later become Active without losing subscribers. Lazy subscribe success means
accepted registration; immediate delivery before first-fire activation is not
promised. Test and document this native-backend activation window.

Keep JS unsubscribe/filter removal immediate and safe inside callbacks; removing
a this-filter does not physically unpatch. Do not synchronously destroy a hook
object from its callback. Physical removal must retain the typed binding,
callback context and any pending invocation state until KHook's removal
completion; never `RemoveHook(id, true)` followed by immediate context deletion.
The native shim stays resident through `.s2sp` reload; script unsubscribe must not
start whole-shim retirement. Physical teardown at server shutdown must retain
registrations and runtime ownership until a validated safe engine boundary, as
defined in the remediation spec. Never wait for an active stack from inside
itself. A KHook id and a declarative s2script id are different namespaces.

### 5.7 Original-byte resolution and validation (PR B prerequisite)

Update `ResolveSigValidated`, `S2_EngineCallResolve` and `S2SdkhooksVpLoad`, including
validated-call, relative-address and RTTI/vtable-equivalence checks. Build a
bounded read-only original module image from the ELF backing the live mapping,
verified against its identity/build id. Preserve the live load bias and
PT_LOAD address correspondence. Decode bytes by module-relative offset and
translate results back into live addresses; never execute or expose a pointer
into the copied image.

Pattern matching **and uniqueness counting**, instruction decoding, arg-width,
xref and prologue checks read that verified original image. Live executable
range, entity and vtable liveness checks still inspect the live process. For a
patched virtual slot, obtain its original target through KHook before comparing
it with the validated target. Do not validate the trampoline as if it were the
engine prologue.

`KHook::LookupSignature` demonstrates original-byte-aware lookup, but a
first-match API alone does not preserve our uniqueness and validator contracts.
Do not mix original-byte lookup with patched-byte uniqueness counting or silently
fall back to unverified on-disk bytes. If the backing image cannot be verified,
fail that descriptor with a named reason.

A native peer fixture must hook a target before and after s2script, exercise
both load orders, and assert one original call, delivery to both consumers,
argument/return precedence, clean unload, and unchanged duplicate-signature
rejection.

---

## 6. Hard constraints

1. **Hard cutover.** New `s2script.so` does not load on Metamod < PLAPI 18. Old `s2script.so` does not load on Metamod ≥ the PR #223 merge. PR A's operator docs and release metadata must name verified PLAPI 18 plus the tested pin/drop; a build date alone does not prove compatibility. Upgrade or roll back the runtime and Metamod as a pair.
2. **Pin Metamod to a commit, not `master` floating.** Recommended floor: `7e24ce9e7a03` (PR #223 + unload fix + khook bumps). Re-verify `third_party/khook` SHA is `1e200e4cc8e0`. A new pin requires rechecking the helper internals and repeating compatibility fixtures.
3. **C++17** already. Include `${MMS}/third_party/khook/include`. Remove `${MMS}/core/sourcehook`. Do not link `libkhook` or SafetyHook into `s2script.so`.
4. **`PLUGIN_SAVEVARS()` is the first line of `Load`.** No `Add`/`Configure` before it. No KHook use from the `S2ScriptPlugin` constructor except storing MFPs.
5. **Do not perform blocking physical hook destruction inside callbacks.** Keep callback-safe filter changes separate from retained, completion-aware physical teardown (§5.6). Test registration while a shared capsule is active; no readiness polling on the game thread.
6. **Validators stay in front of the patch and use verified original bytes.** Preserve bounded reads, uniqueness, executable-range and arg-width rejection. SafetyHook owns relocation decisions; do not retain obsolete s2detour-only refusal rules as requirements on its decoder. A KHook `INVALID_HOOK` is an additional named reason.
7. **Degrade per descriptor.** A broken GameFrame hook does not take down ClientCommand.
8. **Do not change JS.** No new `HookResult` members. No plugin-facing KHook types.
9. **Linux x86_64 only.** KHook’s Windows JIT is out of scope.
10. **One PR per slice of the stack.** A is atomic (does not compile halfway). B and C each `make ci` green alone on top of the previous.

---

## 7. Non-goals

- Shipping a SourceHook compatibility shim.
- Supporting Metamod 2.0.0.1403 after A lands.
- Rewriting the JS multiplexer or changing `HookResult` precedence.
- Moving entity listeners onto KHook.
- Vendoring KHook ourselves (it arrives as Metamod’s submodule).
- Transferring KHook to the AlliedModders org (upstream note in PR #223; not our job).
- Windows / ARM.
- Asking other frameworks to migrate (they will, or they will fight KHook, not us).

---

## 8. Testing / acceptance

**PR A (SourceHook cutover)**

- Full `make ci` green on PR A alone, including committed regenerated licenses. No reduced shim-only substitute for required CI.
- Sniper build loads: `meta list` shows `s2script`, PLAPI 18, no `SourceHook` in `meta version`.
- Boot banner still names every gamedata row.
- Execute T8's fixture matrix: exact original/callback counts, command handling, FireEvent suppression and recipient filtering, voice Recall and CheckTransmit validation/filtering. A first-fire log alone is insufficient.
- SDKHooks: entity-specific delivery, both phase-removal orders, in-callback unsubscribe, deletion/index reuse, peer-patched virtual slots and map change. Required unavailable human-client evidence remains pending.

**PR B (inline unification)**

- Valid live damage fires PRE/POST for the correct victim; controlled native nested-damage fixture restores both pointers. Synthetic core self-test alone is insufficient.
- Chat `HostSay` still reaches `Chat.onMessage`.
- `onOutput` still fires.
- `UserCmd.onRun` still lazy-installs.
- Every declarative shape passes mutation, bypass, opaque-width, result and lifetime tests in Task 9. CanAcquire Changed-deny vs engine-Allow and plugin-Allow vs engine-deny both deny; POST observes current effective result/skipped.
- `s2detour::Install` has **zero** production call sites (tests may keep the relocator).

**PR C**

- Precache still runs (sound slice / `OnMapStart` path).
- Architecture/precache documentation finalized. The minimum INSTALL/BUILDING floor, generated licenses and Metamod rev label already land in A.
- Re-run A's durable Metamod refresh/install path and document any precache coexistence limitation.

---

## 9. Risks (do not paper over)

| Risk | Why it is real | Mitigation |
|------|----------------|------------|
| Peer action precedence | Higher action replaces lower; first wins ties; later peer POST may change return | Test mixed priorities and document the point-in-chain POST observation. |
| VP-cost Think/Touch | One hooked pawn detours the class | Accepted. Measure on live gate; do not rebuild per-instance vtables. |
| SafetyHook relocates a prologue we would have refused | Different decoder | Keep arg-width + executable-range + uniqueness checks **before** `Configure`. Live-gate every current `s2detour` site. |
| Accepted but pending registration | Shared-capsule insertion can queue at this pin | Receipt + passive first-fire state; safe valid-object tests; no sentinel or busy wait. |
| `mmsdrop` latest is still 1403 on day A merges | Drop channel lags `master` | Pin a locally-built Metamod from the submodule for the live gate until drop catches up. `scripts/cloud/install.sh` must be able to use the submodule build. |
| Incomplete interface types under `META_NO_HL2SDK` | `GetVtableIndex` needs a real virtual MFP | Keep hook member types in the `.cpp` (complete `eiface.h`) or a new `shim/src/khooks.cpp` that includes the SDK headers. `s2script_mm.h` only forward-declares. |
| GameSessionConfiguration_t / CheckTransmit / PostEventAbstract ABI | We already special-cased SourceHook `sizeof` | Re-prove each signature on the live server. KHook `_copy_stack_size` copies conservatively; still verify `unsigned long` vs `uint64` on PostEventAbstract. |

---

## 10. File ownership (locked for the plan)

| File | Role after migration |
|------|----------------------|
| `shim/src/s2script_mm.h` | `ISmmPlugin` + handler decls returning `KHook::Return<T>` + `KHook::Virtual<…>` members (types must be complete — prefer moving members to `khooks.cpp` if the header cannot see `eiface.h`) |
| `shim/src/s2script_mm.cpp` | `Load`/`Unload` `Add`/`Remove`; handler bodies |
| `shim/src/sdkhooks_vp.cpp` | Per-kind `Virtual` + `Configure(slot)` + `Add`/`Remove` |
| `shim/src/engine_hooks.cpp` | Checked per-id Function binding, Recall invocation scope, CanAcquire fold (PR B) |
| `shim/src/khook_binding.h` | Checked typed bindings, receipts and teardown ownership (PR A) |
| `shim/src/original_module.h`, `original_module.cpp` | Verified original module bytes + live-address translation (PR B) |
| `shim/src/detour.cpp` | Production call sites gone after B; relocator tests may remain |
| `shim/CMakeLists.txt` | include khook; drop sourcehook |
| `third_party/metamod-source` | submodule bump |
| `docs/ARCHITECTURE.md`, `INSTALL.md`, `BUILDING.md` | thesis + operator floor |
| `scripts/cloud/install.sh`, `scripts/gen-licenses.sh` | Metamod refresh + pin label |
| `core/**` | Public contracts unchanged; comments/diagnostics only unless a tested adapter integration requires more |

---

## Spec self-review

- All eight review findings have explicit plan tasks: receipts/safe probes (T2/T10),
  mutable arguments and CanAcquire (T9), damage scope (T10), original bytes (T9),
  atomic licenses/install prerequisites (T1/T7), executable acceptance (T2/T8–T12),
  and process action precedence (T2/T13).
- Interface and SDKHooks use checked typed Virtual bindings. Declarative hooks
  use one checked typed Function per id, with synchronous Recall keeping the
  per-invocation view alive. Usercmd always returns Ignore.
- Unit/native fixture evidence, live engine evidence and missing human-client
  checks are recorded separately. Compilation or an rg match is never a
  behavioral acceptance result. Any unavailable required gate remains pending.
- This is a revised execution design, not a claim that the migration, the
  checked adapter or live engine behavior has already been implemented/proven.
