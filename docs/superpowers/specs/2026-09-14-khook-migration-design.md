# KHook migration — design spec

**Status:** Draft for review — planning companion is `docs/superpowers/plans/2026-09-14-khook-migration.md`.
**Audience:** shim / core / operator-docs maintainers.
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
| **A** | Pin Metamod + every SourceHook site → KHook `Virtual` | Yes. `s2detour` still private. |
| **B** | `s2detour` sites → KHook `Function`/`Member`; keep `s2detour` only as a test-only relocator or delete it | Yes. Unification. |
| **C** | Precache decision, docs (`ARCHITECTURE`, `INSTALL`, licenses pin), operator Metamod refresh runbook | Yes. Cleanup. |

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

1. **Process:** KHook. First `Override`/`Supersede` among Metamod plugins wins the return value; all PRE callbacks still run. We cannot assume we own the trampoline.
2. **Runtime:** s2script multiplexer. Unchanged `HookResult` contract among TypeScript plugins.

`ARCHITECTURE.md` §1 must be rewritten to say this. The product is still “plugins compose instead of compete” — the *detour* is now Metamod’s, the *JS contract* is ours.

**What does not move:**

- JS API, `HookResult`, ledger, entity books, gamedata ownership, `s2s` CLI.
- `IEntityListener` (not a detour).
- Schema / sigscan / RTTI validators. They still run *before* we ask KHook to patch.
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

Do **not** map JS `HookResult.Changed` to `KHook::Action::Override`. Changed means in-place mutation of a pointed-to object (`IGameEvent`, `CTakeDamageInfo`). The original still runs.

`MRES_HANDLED` is unused in our shim. Do not invent a use.

### 5.2 Interface hooks → `KHook::Virtual`

Pattern (matches Metamod `samples/s2_sample_mm/src/plugin.cpp`): construct with the MFP + callbacks; `Add` only after `PLUGIN_SAVEVARS()`. The ctor calls `GetVtableIndex` only — it does not patch.

`s2script_mm.h` cannot hold `KHook::Virtual<ISource2Server, …>` (`META_NO_HL2SDK`, incomplete `eiface.h`). Put one file-static struct in `s2script_mm.cpp` **after** the SDK includes and **after** `PLUGIN_EXPOSE` (so `&g_S2ScriptPlugin` is declared), using the official context ctor:

```cpp
struct InterfaceHooks {
    KHook::Virtual<ISource2Server, void, bool, bool, bool> gameFrame;
    InterfaceHooks()
      : gameFrame(&ISource2Server::GameFrame, &g_S2ScriptPlugin,
                  &S2ScriptPlugin::Hook_GameFramePre,
                  &S2ScriptPlugin::Hook_GameFramePost) {}
};
static InterfaceHooks g_hk;   // after PLUGIN_EXPOSE — same-TU init order
```

`GameFrame` is **one** `Virtual` with both PRE and POST, then **one** `Add(m_server)`. Today that is two `SH_ADD_HOOK`s.

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
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(&Hook_StartTouch, &Hook_StartTouchPost);
// in S2SdkhooksVpLoad, after the RTTI+sig slot walk — BEFORE any vp_add:
g_hkStartTouch.Configure(slot);
```

`s2_sdkhook_vp_add` still takes `post`. `Add(entity)` on the **first** phase for that entity+kind. `Remove(entity)` only when **both** PRE and POST rows for that entity+kind are gone. Naive per-`post` `Remove` would drop the other phase’s this-filter.

Do **not** expect `Remove` to restore the original vtable slot. The trampoline stays for the process lifetime of that `Virtual` object. JS fan-out in `sdkhooks.rs` already filters by host-id.

Cost model change (accepted): this is VP-cost. Every Think on every pawn of that class enters the JIT when any pawn is hooked. Do not rebuild SourceHook’s per-instance vtable clone.

### 5.4 Inline detours → `KHook::Function` (PR B)

Named sites have concrete C signatures already. Use **typed** `KHook::Function<Ret, Args…>` — not `Function<void>`, not raw `SetupHook` as the first attempt. Default-construct (callbacks only), then `Configure(addr)` after `PLUGIN_SAVEVARS` + validators. A ctor that takes an address calls `SetupHook` immediately.

```cpp
static KHook::Function<int64_t, void*, void*, void*, void*> g_hkDTA(&Hook_DTA_Pre, &Hook_DTA_Post);
// Load, after SAVEVARS + ResolveSigValidated:
g_hkDTA.Configure(dtaAddr);
```

`Function` does not expose the hook id. After `Configure`, if `__exported__khook` was null or the live self-test does not print, treat it as a named degrade (`WARN: DispatchTraceAttack detour install failed`) and **do not** invoke the patched address with the sentinel `this` (that call would enter the real function).

Handlers **stop calling `g_orig*`**. Return `Ignore` and let KHook invoke original, or `Supersede` to skip.

| Site | Signature | Action |
|------|-----------|--------|
| DTA | `int64_t(void*, void*, void*, void*)` | PRE: sentinel `0xD2A7E57` → `Supersede(0)`; else arm info + OnTakeDamage + `Ignore`. POST: OnTakeDamagePost + clear. Self-test still calls the patched address **after** `Configure`. |
| HostSay | `void(void*, void*, bool, int, const char*)` | `Supersede` if chat suppress, else `Ignore` |
| FireOutputInternal | `void(CEntityIOOutput*, CEntityInstance*, CEntityInstance*, const CVariant*, float, void*, char*)` | `Supersede` if `result>=2`, else `Ignore` |
| ProcessUsercmds | `int(void*, void*, int, bool, float)` | **always `Ignore`** — `Handled` mutates the cmd; the engine must still process it |

`S2_HookInstall` keeps its public ABI. Internally one `KHook::Function` per `S2HookShape` (or per hook id), typed to that shape. Spike `this_void` (`onRespawn`) on the live server first. The thunk vocabulary **stays**; the compiled thunk becomes the PRE/POST **callback body**, not the SafetyHook destination. Bypass latch stays in the callback. KHook does not know about outbound `calls`.

Fallback if a typed `Function` cannot compose a shape (live spike fails): `KHook::SetupHook` with that helper’s `make_return` / `make_call_original`. Do not pass today’s thunk as `pre` — PRE must be a KHook callback that returns `Return<T>`.

### 5.5 Precache (PR C)

`OnPrecacheResource` starts with a RIP-relative store; `s2detour` refused it. Do **not** `Function::Configure` that address. Preferred: `KHook::Virtual` + `Configure(index)` + `AddGlobal` on a one-word holder whose first pointer is the RTTI vtable (`CGameRulesGameSystem`). That rewrites the **slot**. PRE always `Ignore` so the game’s own precache still runs. Fallback if live-gate fails: keep `WriteVtableSlot`, logged as a named leftover.

---

## 6. Hard constraints

1. **Hard cutover.** New `s2script.so` does not load on Metamod < PLAPI 18. Old `s2script.so` does not load on Metamod ≥ the PR #223 merge. Operator docs and the release zip must say “Metamod 2.0 **after 2026-09-08**” (or the first `mmsdrop` build that advertises it).
2. **Pin Metamod to a commit, not `master` floating.** Recommended floor: `7e24ce9e7a03` (PR #223 + unload fix + khook bumps). Re-verify `third_party/khook` SHA is `1e200e4cc8e0` (or newer if we bump again).
3. **C++17** already. Include `${MMS}/third_party/khook/include`. Remove `${MMS}/core/sourcehook`. Do not link `libkhook` or SafetyHook into `s2script.so`.
4. **`PLUGIN_SAVEVARS()` is the first line of `Load`.** No `Add`/`Configure` before it. No KHook use from the `S2ScriptPlugin` constructor except storing MFPs.
5. **Never `Add`/`Configure` a hook from inside that same hook** (`async=true` deadlock). Lazy install from JS subscribe is fine when the hook is not on the stack.
6. **Validators stay in front of the patch.** `s2detour`’s named refusals (unmapped, short branch, arg-width) move to the pre-`Configure` path. A KHook `INVALID_HOOK` is an additional named reason, not a silent skip.
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

- `make ci-native` (or at least shim + `cargo test -p s2script-core`) green after the pin.
- Sniper build loads: `meta list` shows `s2script`, PLAPI 18, no `SourceHook` in `meta version`.
- Boot banner still names every gamedata row.
- Live: `OnGameFrame` increments; a client lifecycle event fires (bot connect); `sm_slap` / chat path still works if those plugins are loaded; `CheckTransmit` first-fire validation still prints; voice first-fire still prints; FireEvent `Handled` still hides the kill-feed path (the `CallOriginal` + `Supersede` pair).
- SDKHooks: `SDKHook(entity, Touch, …)` still fans out only for that entity (isolate tests unchanged; live: one hooked prop).

**PR B (inline unification)**

- Damage self-test (`S2_DAMAGE_SELFTEST=1`) still fires.
- Chat `HostSay` still reaches `Chat.onMessage`.
- `onOutput` still fires.
- `UserCmd.onRun` still lazy-installs.
- A declarative `hooks` row still installs; a failed prologue / arg-width still degrades **by name** before `Configure`.
- `s2detour::Install` has **zero** production call sites (tests may keep the relocator).

**PR C**

- Precache still runs (sound slice / `OnMapStart` path).
- `docs/INSTALL.md`, `docs/ARCHITECTURE.md` §1, `docs/BUILDING.md`, `licenses/licenses.txt` pin, `scripts/gen-licenses.sh` Metamod rev label updated.
- `scripts/cloud/install.sh` force-refreshes Metamod when the on-disk binary is pre-KHook (or always re-pulls `mmsource-latest-linux` when `S2_REFRESH_METAMOD=1`).

---

## 9. Risks (do not paper over)

| Risk | Why it is real | Mitigation |
|------|----------------|------------|
| First-wins Action vs our max-collapse | Another MM plugin can Supersede before us and skip the original even if our JS said Continue | Accept. Log once if `WasOriginalFunctionSkipped()` in POST when we returned Ignore. Do not fight. |
| VP-cost Think/Touch | One hooked pawn detours the class | Accepted. Measure on live gate; do not rebuild per-instance vtables. |
| SafetyHook relocates a prologue we would have refused | Different decoder | Keep arg-width + executable-range + uniqueness checks **before** `Configure`. Live-gate every current `s2detour` site. |
| `async=true` deadlock | Documented in KHook commits | Never install from inside the hooked function. |
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
| `shim/src/engine_hooks.cpp` | `S2_HookInstall` → KHook `Function` (PR B) |
| `shim/src/detour.cpp` | Production call sites gone after B; relocator tests may remain |
| `shim/CMakeLists.txt` | include khook; drop sourcehook |
| `third_party/metamod-source` | submodule bump |
| `docs/ARCHITECTURE.md`, `INSTALL.md`, `BUILDING.md` | thesis + operator floor |
| `scripts/cloud/install.sh`, `scripts/gen-licenses.sh` | Metamod refresh + pin label |
| `core/**` | Comments only, unless an FFI string mentions SourceHook |

---

## Spec self-review

- **Placeholders:** none. Pins, mappings, and PR boundaries are explicit.
- **Consistency:** B is the destination; A is the load-blocking increment; C is docs/leftovers. Action mapping matches the research note’s s2script-specific section.
- **Scope:** one migration. Not a new hook API. Not a multiplexer rewrite.
- **Ambiguity:** Precache has a preferred path and a named leftover. `s2detour` production sites are zero after B. Interface hooks use the official sample ctor (not `AddContext`). Declarative hooks use typed `Function` per `S2HookShape`. Usercmd never `Supersede`s.
