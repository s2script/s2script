# KHook Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Load s2script on post–PR-223 Metamod (PLAPI 18, SourceHook deleted) and put every engine intercept we own through KHook’s one-detour-per-address catalog.

**Architecture:** Three stacked PRs. PR A pins Metamod and replaces every `SH_*` site with `KHook::Virtual`. PR B moves every `s2detour::Install` to `KHook::Function`/`Member`. PR C decides precache, rewrites operator/architecture docs, and refreshes the live-gate Metamod tree. JS `HookResult` and the Rust multiplexer do not change.

**Tech Stack:** Metamod:Source `master` ≥ `7e24ce9e7a03` (KHook submodule `1e200e4cc8e0`), C++17 shim, SafetyHook linked **inside Metamod** (we only include `khook.hpp`), existing `s2validate` / gamedata validators, Docker CS2 live gate.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-09-14-khook-migration-design.md`
- Research: `docs/superpowers/specs/2026-09-14-khook-vs-sourcehook-research.md`
- Hard cutover: new binary needs Metamod PLAPI 18; old binary will not load on that Metamod
- `PLUGIN_SAVEVARS()` is the first line of `Load`; no `Add`/`Configure` before it
- Do not link `libkhook` or SafetyHook into `s2script.so`
- JS API / `HookResult` / multiplexer unchanged
- `Handled|Stop` → `KHook::Action::Supersede`; everything else → `Ignore` unless the site is FireEvent (`CallOriginal` + `Supersede`) or SetClientListening (`Recall`)
- Validators run before `Configure`/`Add`; `INVALID_HOOK` is a named degrade
- Never `Add`/`Configure` a hook from inside that same hook (`async=true` deadlock)
- Linux x86_64 only; sniper build for anything that must load
- One PR per stack entry; each `make ci` green alone (Docker `test-gate.sh` miss is benign for docs-only commits)

**PR map (stack, bottom → top):**

| PR | Branch suffix | Delivers |
|----|---------------|----------|
| A | `khook-sourcehook-cutover` | Metamod pin + every SourceHook site |
| B | `khook-s2detour-cutover` | Inline detours through KHook |
| C | `khook-docs-precache` | Precache + operator/architecture docs |

---

## File Structure

```
third_party/metamod-source/          # PR A: submodule bump (sourcehook/ gone, khook/ arrives)
shim/CMakeLists.txt                  # PR A: include khook, drop sourcehook
shim/src/s2script_mm.h               # PR A: handler signatures → KHook::Return<T>
shim/src/s2script_mm.cpp             # PR A: Virtual members, Add/Remove, handler bodies
shim/src/sdkhooks_vp.cpp             # PR A: Virtual + Configure(slot)
shim/src/sdkhooks_vp.h               # PR A: drop SH_MANUALHOOK comments
shim/src/khook_map.h                 # PR A: NEW — Action helpers (Ignore / SupersedeFromHookResult)
shim/src/engine_hooks.cpp            # PR B: S2_HookInstall → Function::Configure
shim/src/s2script_mm.cpp             # PR B: DTA / HostSay / FOI / UserCmd → Member/Function
shim/src/detour.cpp                  # PR B: zero production call sites
docs/ARCHITECTURE.md                 # PR C
docs/INSTALL.md                      # PR C
docs/BUILDING.md                     # PR C
docs/PROGRESS.md                     # PR C: append finished-slice entry after live gate
scripts/cloud/install.sh             # PR C: force-refresh pre-KHook docker/metamod
scripts/gen-licenses.sh              # PR C: drop hardcoded 2.0.0.1403 label
```

Task map: **T1–T8 = PR A**. **T9–T11 = PR B**. **T12–T13 = PR C**. T1–T4 compile-only; T5–T8 need a post-223 Metamod to load; T9–T11 need the live damage/chat/output/usercmd paths; T12–T13 are docs + precache live check.

This file is the implementation plan. The PR that lands it is **docs only** — do not start T1 here.

---

## Canonical bind patterns (locked)

Read these before T3 / T5 / T9. They replace any “if assignment is deleted” hedging.

**Interface `Virtual` (PR A)** — Metamod s2 sample, file-static because the header cannot see complete `eiface.h` types. Construct **after** `PLUGIN_EXPOSE` (same TU, so `&g_S2ScriptPlugin` exists). Ctor stores MFP + callbacks; `GetVtableIndex` only; no patch.

```cpp
PLUGIN_EXPOSE(S2ScriptPlugin, g_S2ScriptPlugin);

struct InterfaceHooks {
    KHook::Virtual<ISource2Server, void, bool, bool, bool> gameFrame;
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
    g_hk.gameFrame.Add(g_S2ScriptPlugin.m_server);   // lazy GameFrame: only when subscribed
}
```

`GameFrame` is **one** object with both callbacks, **one** `Add`. PRE-only / POST-only pass `nullptr` for the unused callback, matching `samples/s2_sample_mm/src/plugin.cpp`.

**SDKHooks `Virtual` (PR A)** — existing free-function handlers. Bind PRE/POST at file scope; set the slot later; share one object across phases.

```cpp
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(&Hook_StartTouch, &Hook_StartTouchPost);
// S2SdkhooksVpLoad, after slot derive, BEFORE any vp_add:
g_hkStartTouch.Configure(slot);
// vp_add: Add(p) on the first phase for this entity+kind
// vp_remove / drop: Remove(p) only when both PRE and POST rows for this entity+kind are gone
```

**Inline `Function` (PR B)** — typed to the real signature. Default-construct with callbacks; `Configure(addr)` only after `PLUGIN_SAVEVARS`. Never `Function<void>`. Never construct with an address at static init.

```cpp
static KHook::Function<int64_t, void*, void*, void*, void*> g_hkDTA(&Hook_DTA_Pre, &Hook_DTA_Post);
g_hkDTA.Configure(dtaAddr);   // Load, after validators
```

---

### Task 1: Pin Metamod past PR #223

**Files:**
- Modify: `.gitmodules` (unchanged URL; commit pointer moves)
- Modify: `third_party/metamod-source` (submodule)
- Modify: `shim/CMakeLists.txt`
- Modify: `scripts/gen-licenses.sh` (label only if you regenerate licenses in this task — prefer PR C)

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
# Expected: 1e200e4cc8e0badcb7cf941525268d6977f6a4e6 (or newer if that commit moved; record the actual SHA in the PR body)
test ! -d third_party/metamod-source/core/sourcehook
test -f third_party/metamod-source/third_party/khook/include/khook.hpp
grep -n "METAMOD_PLAPI_VERSION" third_party/metamod-source/core/ISmmPluginExt.h
# Expected: #define METAMOD_PLAPI_VERSION			18
```

If `7e24ce9` is not on the fetched `master` (upstream moved), take the newest `master` that still contains `GetDetourInterface` and `PLAPI_MIN_VERSION 18`, and write that SHA in the PR body. Do not float `master` without a SHA.

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

- [ ] **Step 4: Commit the pin + CMake include swap**

```bash
git add .gitmodules third_party/metamod-source shim/CMakeLists.txt
git commit -m "chore(shim): pin Metamod past KHook merge; include khook.hpp"
```

---

### Task 2: Action helpers + handler signature flip

**Files:**
- Create: `shim/src/khook_map.h`
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
  // collapsed JS HookResult: 0 Continue, 1 Changed, 2 Handled, 3 Stop
  inline KHook::Return<void> S2_FromHookResult(int r) {
      return r >= 2 ? S2_Supersede() : S2_Ignore();
  }
  template <typename T>
  inline KHook::Return<T> S2_FromHookResult(int r, T v) {
      return r >= 2 ? S2_Supersede(v) : S2_Ignore(v);
  }
  ```

- [ ] **Step 1: Write `shim/src/khook_map.h`**

Create the file with the exact helpers above, plus:

```cpp
#pragma once
#include <khook.hpp>
```

No `using namespace`. No macros named `RETURN_META`.

- [ ] **Step 2: Change handler declarations in `s2script_mm.h`**

Replace the SourceHook handler block (`s2script_mm.h` L60–115) with KHook signatures. Every `Virtual`/`Member` callback takes the interface pointer first. Keep the existing parameter types (including `unsigned long long` for `uint64` under `META_NO_HL2SDK`).

```cpp
#include "khook_map.h"

    KHook::Return<void> Hook_GameFramePre(ISource2Server*, bool simulating, bool first, bool last);
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

Forward-declare `IGameEventManager2`, `ICvar`, `IVEngineServer2`, `IGameEventSystem` at the top of the header next to the existing forwards. Do **not** put `KHook::Virtual<…>` members in this header yet (incomplete types / `META_NO_HL2SDK`). They land in Task 3 as file-static or `.cpp` members.

- [ ] **Step 3: Commit**

```bash
git add shim/src/khook_map.h shim/src/s2script_mm.h
git commit -m "feat(shim): KHook handler signatures and Action helpers"
```

---

### Task 3: `KHook::Virtual` members, ctor, Add/Remove

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (drop every `SH_DECL_HOOK*`; add members + ctor)
- Modify: `shim/src/s2script_mm.h` only if you choose to store the `Virtual` objects as members — then the `.cpp` must include `eiface.h` **before** the header, or the members live as file-static in the `.cpp`.

**Interfaces:**
- Consumes: Task 2 handler signatures.
- Produces: one file-static `InterfaceHooks` with a `KHook::Virtual<Iface, Ret, Args…>` per interface hook (official context ctor), `Add`ed after `PLUGIN_SAVEVARS`.

- [ ] **Step 1: Delete every `SH_DECL_HOOK*` at the top of `s2script_mm.cpp` (L93–L170)**

Those macros will not exist. Do not leave them commented.

- [ ] **Step 2: Declare one `InterfaceHooks` struct in `s2script_mm.cpp` immediately after `PLUGIN_EXPOSE` (today L175)**

`S2ScriptPlugin g_S2ScriptPlugin;` is L174. The struct must be **after** that object so the context ctor can take `&g_S2ScriptPlugin`. Same-TU initialization order is declaration order.

Use the official context ctor (MFP + `&g_S2ScriptPlugin` + member callbacks). Same integer aliases as the deleted `SH_DECL_*` lines (`uint64` vs `unsigned long long`) so the MFP matches `eiface.h`.

```cpp
struct InterfaceHooks {
    KHook::Virtual<ISource2Server, void, bool, bool, bool> gameFrame;
    KHook::Virtual<IGameEventManager2, bool, IGameEvent*, bool> fireEvent;
    KHook::Virtual<IGameEventSystem, void, CSplitScreenSlot, bool, int, const uint64*,
                   INetworkMessageInternal*, const CNetMessage*, unsigned long, NetChannelBufType_t> postEvent;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot, const CCommand&> clientCommand;
    KHook::Virtual<ICvar, void, ConCommandRef, const CCommandContext&, const CCommand&> dispatchConCommand;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot, const char*, uint64, const char*, const char*, bool> onClientConnected;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot, const char*, int, uint64> clientPutInServer;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot, bool, const char*, uint64> clientActive;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot> clientFullyConnect;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot, ENetworkDisconnectionReason, const char*, uint64, const char*> clientDisconnect;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot> clientSettingsChanged;
    KHook::Virtual<ISource2GameClients, void, CPlayerSlot> clientVoice;
    KHook::Virtual<ISource2GameEntities, void, CCheckTransmitInfo**, int, CBitVec<16384>&, CBitVec<16384>&,
                   const Entity2Networkable_t**, const unsigned short*, int> checkTransmit;
    KHook::Virtual<IVEngineServer2, bool, CPlayerSlot, CPlayerSlot, bool> setClientListening;
    KHook::Virtual<INetworkServerService, void, const GameSessionConfiguration_t&, ISource2WorldSession*, const char*> startupServer;

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
    g_hk.gameFrame.Add(g_S2ScriptPlugin.m_server);
    g_S2ScriptPlugin.m_frameHookInstalled = true;
} else if (!enable && g_S2ScriptPlugin.m_frameHookInstalled) {
    g_hk.gameFrame.Remove(g_S2ScriptPlugin.m_server);
    g_S2ScriptPlugin.m_frameHookInstalled = false;
}
```

Same shape for `"GameEvent"` → `g_hk.fireEvent`.

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
KHook::Return<void> S2ScriptPlugin::Hook_GameFramePre(ISource2Server*, bool simulating, bool first, bool last) {
    // …existing dispatch body unchanged…
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

**Interfaces:**
- Consumes: existing `s2_sdkhook_vp_add` / `s2_sdkhook_vp_remove` FFI (signatures unchanged).
- Produces: same FFI, implemented with `KHook::Virtual<CEntityInstance, …>`.

- [ ] **Step 1: Replace `SH_DECL_MANUALHOOK*` (L50–63) with Virtual objects**

```cpp
#include <khook.hpp>
#include "khook_map.h"

namespace {
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch;
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkTouch;
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkEndTouch;
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkBlocked;
KHook::Virtual<CEntityInstance, void> g_hkSpawn;
KHook::Virtual<CEntityInstance, void> g_hkThink;
KHook::Virtual<CEntityInstance, void> g_hkPreThink;
KHook::Virtual<CEntityInstance, void> g_hkPostThink;
KHook::Virtual<CEntityInstance, void, CEntityInstance*, CEntityInstance*, int, float> g_hkUse;
KHook::Virtual<CEntityInstance, int> g_hkGetMaxHealth;
KHook::Virtual<CEntityInstance, bool, int, int> g_hkShouldCollide;
KHook::Virtual<CEntityInstance, void> g_hkVPhysicsUpdate;
KHook::Virtual<CEntityInstance, void> g_hkGroundEntChanged;
KHook::Virtual<CEntityInstance, bool> g_hkCanBeAutobalanced;
}
```

- [ ] **Step 2: Bind PRE/POST on the file-static objects (no `AddContext`)**

Keep the existing `Hook_StartTouch` / `Hook_StartTouchPost` free functions. Change their signatures to take `CEntityInstance* thisPtr` first and return `KHook::Return<void>` / `KHook::Return<int>` / `KHook::Return<bool>`. Replace `RETURN_META(MRES_SUPERCEDE)` with `return S2_Supersede();` and `RETURN_META(MRES_IGNORED)` with `return S2_Ignore();`. For `GetMaxHealth` / `ShouldCollide` / `CanBeAutobalanced`, replace `RETURN_META_VALUE` with `S2_Supersede(v)` / `S2_Ignore(v)`.

Construct each object with those free functions (index still invalid):

```cpp
KHook::Virtual<CEntityInstance, void, CEntityInstance*> g_hkStartTouch(&Hook_StartTouch, &Hook_StartTouchPost);
```

Where a handler today calls `SH_MCALL` (`GetMaxHealth` L302, `ShouldCollide` L315, `CanBeAutobalanced` L328):

```cpp
int orig = g_hkGetMaxHealth.CallOriginal(thisPtr);
```

- [ ] **Step 3: `SH_MANUALHOOK_RECONFIGURE` → `Configure(slot)` once, before any `Add`**

In `Reconfigure` (today L381–398), replace each `SH_MANUALHOOK_RECONFIGURE` with `g_hkStartTouch.Configure(slot);` (and the matching object). `S2SdkhooksVpLoad` already runs at Load, before JS can `vp_add`. Do not reconstruct the `Virtual`. Do not `Configure` again after entities are in `_hooked_this` — that clears the this-filter (`khook.hpp` L1881–1888).

- [ ] **Step 4: `SH_ADD_MANUALHOOK` → `Add`; `SH_REMOVE_HOOK_ID` → `Remove` only when both phases are gone**

Core never stores the SourceHook id (`sdkhooks.rs` `vp_add` is `f(...) != 0`). FFI stays `1`/`0`. Drop the shim `hook_id` field or leave it unused.

`g_installed` is still keyed `{p, kind, post}` because core calls `vp_add`/`vp_remove` once per phase. One `Virtual` serves both phases:

```cpp
static void VpAddThis(Kind kind, void* p) {
    switch (kind) {
    case Kind::StartTouch: g_hkStartTouch.Add(static_cast<CEntityInstance*>(p)); break;
    // …every kind…
    }
}
static void VpRemoveThis(Kind kind, void* p) {
    switch (kind) {
    case Kind::StartTouch: g_hkStartTouch.Remove(static_cast<CEntityInstance*>(p)); break;
    // …every kind…
    }
}

static bool OtherPhaseLive(void* p, Kind kind, int post) {
    VpKey other{ p, kind, post ? 0 : 1 };
    return g_installed.find(other) != g_installed.end();
}
```

In `s2_sdkhook_vp_add`, after the existing refcount-hit return: `AddManual` becomes `VpAddThis` (always, even if the other phase already `Add`ed — `_hooked_this.insert` is idempotent). In `s2_sdkhook_vp_remove` / `s2_sdkhook_vp_drop` / `S2SdkhooksVpUnload`: call `VpRemoveThis` only when the last remaining row for that `{p, kind}` (either phase) is erased. If PRE unsubscribes while POST is still live, do **not** `Remove`.

Update the comment at `core/src/sdkhooks.rs` L4–5 from `SH_ADD_MANUALHOOK` to `KHook::Virtual::Add`.

- [ ] **Step 5: Shim unit tests that mention SourceHook**

```bash
rg -n "SH_|SourceHook" shim/tests
```

Update comments only. No new C++ test for KHook itself (it lives in Metamod).

- [ ] **Step 6: Commit**

```bash
git add shim/src/sdkhooks_vp.cpp shim/src/sdkhooks_vp.h core/src/sdkhooks.rs
git commit -m "feat(shim): SDKHooks VP via KHook::Virtual"
```

---

### Task 6: Compile the shim (PR A)

**Files:** whatever Task 3–5 left failing.

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

- [ ] **Step 2: Core tests (unchanged contract)**

```bash
cargo test -p s2script-core
```

Expected: PASS (single-threaded via `.cargo/config.toml`).

- [ ] **Step 3: Sniper build**

```bash
sudo docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry \
  rust:bullseye bash /repo/scripts/build-sniper.sh
```

Expected: `dist/addons/s2script/bin/linuxsteamrt64/s2script.so` exists and `ldd` does not name `libkhook` or `libsafetyhook`.

- [ ] **Step 4: Commit any compile fixes**

```bash
git add -u
git commit -m "fix(shim): compile clean against KHook / PLAPI 18"
```

---

### Task 7: Live-gate Metamod binary (PR A)

**Files:**
- Modify: `scripts/cloud/install.sh` (temporary force-refresh is enough for the gate; PR C makes it durable)
- Operator tree: `docker/metamod/` (gitignored)

**Interfaces:**
- Produces: `docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so` built from the pinned submodule **or** a post-223 `mmsdrop` tarball.

- [ ] **Step 1: Detect whether drop is new enough**

```bash
strings docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so | rg -n "GetDetourInterface|SourceHook version|KHook" | head
```

If `GetDetourInterface` is absent, the tree is pre-223.

- [ ] **Step 2: Replace `docker/metamod`**

Preferred if `mmsdrop` is still 1403: build Metamod from the submodule inside the sniper/AMBuild environment and copy `addons/metamod/*` over `docker/metamod/`. If `mmsource-latest-linux` already contains `GetDetourInterface`:

```bash
latest="$(curl -fsSL https://mms.alliedmods.net/mmsdrop/2.0/mmsource-latest-linux)"
curl -fsSL "https://mms.alliedmods.net/mmsdrop/2.0/${latest}" -o /tmp/mms.tar.gz
rm -rf /tmp/mms && mkdir -p /tmp/mms && tar xzf /tmp/mms.tar.gz -C /tmp/mms
rm -rf docker/metamod
mkdir -p docker/metamod
cp -r /tmp/mms/addons/metamod/* docker/metamod/
test -f docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so
```

Keep `docker/s2script.vdf` in place (`scripts/package-addon.sh` copies it).

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

### Task 8: PR A live acceptance (do not start PR B until this is green)

**Files:** none (evidence goes in the PR body).

- [ ] **Step 1: Frame + events**

```bash
python3 scripts/rcon.py "sv_hibernate_when_empty 0"
# existing cookbook / hello-plugin that increments OnGameFrame — or watch
# [s2script] boot lines and S2_DAMAGE_SELFTEST if compose still sets it
```

Expected: server ticks; no crash on `GameFrame`. If `S2_DAMAGE_SELFTEST=1` is on, the synthetic damage hook still prints (still `s2detour` in PR A — that is OK).

- [ ] **Step 2: Clients + commands**

```bash
python3 scripts/rcon.py "bot_quota_mode normal" "bot_join_after_player 0" "mp_limitteams 0" "bot_quota 1"
python3 scripts/rcon.py "meta list"
```

Expected: bot connect prints client-lifecycle first-fire if those logs still exist; `ClientCommand` / `DispatchConCommand` do not crash on `say` / `status`.

- [ ] **Step 3: Voice first-fire**

Look for `[s2script] voice: SetClientListening first fire` in `docker logs`. Expected: present after a listen-matrix refresh; slots in range. A mute via `basecomm` (if loaded) must still flip `bListen` — if you cannot drive a real client, at least confirm the first-fire line and that Recall did not deadlock (server still ticks).

- [ ] **Step 4: FireEvent hide path**

If a plugin returning `HookResult.Handled` on `player_death` is available (TTT or a one-off), confirm the `CallOriginal` + `Supersede` path still arms `s_legacyFilterActive`. If not available, unit-level: the handler compiles and `rg CallOriginal shim/src/s2script_mm.cpp` hits `Hook_FireEventPre`.

- [ ] **Step 5: Open / update PR A**

Title: `shim: cut SourceHook over to KHook (PLAPI 18)`.
Body: Why (Metamod deleted SourceHook; we cannot load). Link the spec + research. Paste `meta version` / `meta list` output. List every interface hook converted.

---

### Task 9: PR B — `S2_HookInstall` through KHook

**Files:**
- Modify: `shim/src/engine_hooks.cpp`
- Modify: `shim/src/engine_hooks.h` (comment only)

**Interfaces:**
- Consumes: existing `S2_HookInstall(hookId, shape, addr, reason, cap)` ABI.
- Produces: same ABI; internally `KHook::Function` per slot.

- [ ] **Step 1: One typed `Function` per shape, not `Function<void>`**

`S2HookShape` already is a closed typed vocabulary. Map each id to a `KHook::Function` with that signature. Default-construct with PRE/POST callbacks that contain today’s thunk **body** (dispatch + bypass + collapse). Do **not** pass the old thunk as KHook `pre` (it does not return `KHook::Return<T>`). Do **not** use `Function<void>`.

```cpp
static KHook::Function<void, void*> g_fnThisVoid(&Pre_ThisVoid, &Post_ThisVoid);
static KHook::Function<void, void*, float, int32_t, int32_t, int32_t> g_fnThisF32I32I32I32(&Pre_F32I32I32I32, &Post_F32I32I32I32);
static KHook::Function<void, void*, float, int32_t, int64_t, int64_t> g_fnThisF32I32I64I64(&Pre_F32I32I64I64, &Post_F32I32I64I64);
static KHook::Function<int32_t, void*, int64_t, int32_t, int64_t> g_fnThisI64I32I64(&Pre_I64I32I64, &Post_I64I32I64);
static KHook::Function<void, void*, int64_t, int64_t, int64_t> g_fnThisI64I64I64(&Pre_I64I64I64, &Post_I64I64I64);
```

Each PRE: `if (S2Hook_BypassTake(hookId)) return S2_Ignore();` then existing `S2Hook_Dispatch`; `return S2_FromHookResult(result);` (and for `this_i64_i32_i64`, `S2_FromHookResult(result, ret)`). Each POST: `S2Hook_DispatchPost`. Stop writing `g_hooks[id].orig` and stop calling it — KHook runs original on `Ignore`.

`S2_HookInstall` still validates range + arg-width **before** `Configure`. Then `Configure((void*)addr)` on the Function for that shape. New fail string: `"khook SetupHook returned INVALID_HOOK"`. Idempotence and “two ids on one address” checks stay.

Need the hook id inside the shared-per-shape callback: keep a TLS / `GetContext` slot, or one `Function` **per hook id** (array of 64) typed via a small wrapper that closes over `hookId`. Per-id objects are simpler and match today’s `g_hooks[S2_HOOK_MAX]`. If the five shape types cannot live in one array, use five arrays or a `variant`. Locked: **per hook id**, constructed when that id is first installed (after SAVEVARS).

- [ ] **Step 2: Spike `this_void` on the live server**

`gamedata/cs2` ships `onRespawn` as `this_void` (`CCSPlayerController_Respawn`). Convert only that shape first. Confirm: subscribe installs; JS `Handled` skips the original; outbound `respawn` still honors `S2_HookArmBypass`. `onTerminateRound` is `this_f32_i32_i64_i64` — second spike, not the first.

If the typed `Function` spike fails (wrong register restore, return not applied), fall back to `KHook::SetupHook` using that `Function` specialization’s `_KHook_MakeReturn` / `_KHook_MakeOriginalCall` (copy the two statics). Do not invent a third trampoline.

- [ ] **Step 3: Convert the remaining shapes**

Same wrapper for `this_f32_i32_i32_i32` and the other rows in `hook_dispatch.h`. `S2_HookInstall` failure paths stay identical strings plus one new reason: `"khook SetupHook returned INVALID_HOOK"`.

- [ ] **Step 4: `S2_HookResetAll` / Unload**

For each used slot, `KHook::RemoveHook(id, true)` and clear `g_hooks[i]`. Still call this from the same place as `s2detour::RemoveAll()` until Task 10 removes that call.

- [ ] **Step 5: Commit**

```bash
git add shim/src/engine_hooks.cpp shim/src/engine_hooks.h shim/src/hook_dispatch.cpp
git commit -m "feat(shim): declarative inbound hooks install via KHook::SetupHook"
```

---

### Task 10: PR B — named `s2detour` sites

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (`Detour_DispatchTraceAttack`, `Detour_HostSay`, `Hook_FireOutputInternal`, `Detour_ProcessUsercmds`)

**Interfaces:**
- Consumes: Task 9’s `SetupHook` pattern (absolute address + member/free PRE).
- Produces: four `KHook::Member` or `KHook::Function` objects, configured after sig resolve.

- [ ] **Step 1: Typed `Function` per named site (keep file-static `Detour_*`)**

Cannot name the game class — use `Function`, not `Member`. Default-construct with callbacks; `Configure(addr)` after `ResolveSigValidated` + `.text` check + `PLUGIN_SAVEVARS`. Delete every `g_orig*` call.

```cpp
static KHook::Function<int64_t, void*, void*, void*, void*> g_hkDTA(&Hook_DTA_Pre, &Hook_DTA_Post);
static KHook::Function<void, void*, void*, bool, int, const char*> g_hkHostSay(&Hook_HostSay_Pre, nullptr);
static KHook::Function<void, CEntityIOOutput*, CEntityInstance*, CEntityInstance*, const CVariant*, float, void*, char*>
    g_hkFOI(&Hook_FOI_Pre, nullptr);
static KHook::Function<int, void*, void*, int, bool, float> g_hkUsercmd(&Hook_Usercmd_Pre, nullptr);
```

| Site | PRE / POST | Action |
|------|------------|--------|
| DTA | Split today’s body: PRE arms `s_currentDamageInfo` + `s2script_core_dispatch_damage`; POST runs `dispatch_damage_post` and clears. Sentinel `this == 0xD2A7E57` in PRE → `return S2_Supersede(int64_t{0});` (do not enter original). Real damage → `return S2_Ignore(int64_t{0});` (KHook calls original; ignore the dummy ret, original’s ret wins on Ignore). After `Configure`, keep the install-time self-test call on `dtaAddr`. | |
| HostSay | One PRE. `suppress` → `S2_Supersede();` else `S2_Ignore();` | |
| FOI | One PRE. `result >= 2` → `S2_Supersede();` else `S2_Ignore();` | |
| Usercmd | One PRE. Neutralize cmds in place when `res >= 2`. **Always `return S2_Ignore(0);`** — the engine must still process the (possibly zeroed) usercmd. | |

Lazy Usercmd: `Shim_UsercmdHookInstall` becomes `g_hkUsercmd.Configure(s_pProcessUsercmdsAddr)` instead of `s2detour::Install`. Still idempotent.

- [ ] **Step 2: Grep production Installs**

```bash
rg -n "s2detour::Install" shim/src
```

Expected: zero hits in `s2script_mm.cpp` and `engine_hooks.cpp`. Hits in `shim/tests/detour_reloc_test.cpp` are OK.

- [ ] **Step 3: Live paths**

- `S2_DAMAGE_SELFTEST=1` still prints.
- Chat message from a client/bot reaches `Chat.onMessage` (or the antiflood / basechat path).
- An entity output subscription still fires (`onOutput`).
- A plugin subscribed to `UserCmd.onRun` still installs lazily and can `Handled`-block.

- [ ] **Step 4: Commit**

```bash
git add shim/src/s2script_mm.cpp
git commit -m "feat(shim): install DTA/HostSay/FOI/UserCmd through KHook"
```

---

### Task 11: PR B — retire production `s2detour`

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (`s2detour::RemoveAll()` in `Unload`)
- Modify: `shim/CMakeLists.txt` only if `detour.cpp` is no longer linked into `s2script` (keep it for `detour_reloc_test`)

**Interfaces:**
- Produces: Unload only calls `KHook::RemoveHook` / `Virtual` destructors / `S2_HookResetAll`.

- [ ] **Step 1: Unload**

Remove `s2detour::RemoveAll()` from `Unload` once Task 9–10 own every patch. Metamod also async-removes hooks on plugin unload (`metamod_plugins.cpp` Unloader) — still `Remove` ourselves so a failed Unload that stays resident does not leave PRE callbacks pointing at us.

- [ ] **Step 2: Host + sniper + core tests**

```bash
cmake --build build/shim -j
cargo test -p s2script-core
# sniper + docker restart + the four live paths from Task 10
```

- [ ] **Step 3: Commit + PR B**

```bash
git add -u
git commit -m "chore(shim): drop production s2detour::Install"
```

Title: `shim: put inline detours through KHook`.
Body: Why (issue #215 / we were the private detour). List the four named sites + declarative `S2_HookInstall`. State that `detour_reloc_test` still ships the relocator.

---

### Task 12: PR C — precache

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (`WriteVtableSlot` / `InstallPrecacheHook`)

**Interfaces:**
- Consumes: existing RTTI vtable + gamedata index.
- Produces: `KHook::Virtual` on that class, or a named leftover.

- [ ] **Step 1: `Virtual` on the RTTI vtable + gamedata index (slot rewrite, not prologue)**

Do **not** `Function::Configure(slotFn)` — that is SafetyHook on the RIP-relative prologue `s2detour` already refused (`s2script_mm.cpp` L3498–3500). KHook `Virtual` rewrites the **slot**, which is what `WriteVtableSlot` does, but catalogued.

```cpp
struct PrecacheObj {};   // never name CGameRulesGameSystem
static KHook::Return<void> Hook_Precache_Pre(PrecacheObj* self, void* pManifest) {
    s_currentPrecacheManifest = pManifest;
    s2script_core_dispatch_precache();
    s_currentPrecacheManifest = nullptr;
    return S2_Ignore();   // original still runs
}
static KHook::Virtual<PrecacheObj, void, void*> g_hkPrecache(&Hook_Precache_Pre, nullptr);

// InstallPrecacheHook, after RTTI + .text check:
g_hkPrecache.Configure(s_precacheVtblIdx);
struct { void** vptr; } holder{ vt };
g_hkPrecache.AddGlobal(reinterpret_cast<PrecacheObj*>(&holder));
```

`Add` would filter one `this`; `AddGlobal` keeps every instance of that vtable (one game-rules system, but the filter is the vtable). Unload: `RemoveGlobal` / let the `Virtual` dtor run. Delete `WriteVtableSlot` if this live-gates.

If `AddGlobal` does not fire (index wrong / JIT skip), restore `WriteVtableSlot`, keep the comment, print `[s2script] precache: KHook Virtual failed, using WriteVtableSlot leftover`.

- [ ] **Step 2: Live**

A map change must still precache the sound slice’s paths (existing log lines). If this segfaults or skips, revert to `WriteVtableSlot`, leave the comment, and print `[s2script] precache: KHook Virtual failed, using WriteVtableSlot leftover`.

- [ ] **Step 3: Commit**

```bash
git add shim/src/s2script_mm.cpp
git commit -m "feat(shim): precache slot hook via KHook (or named leftover)"
```

---

### Task 13: PR C — docs, licenses, install refresh

**Files:**
- Modify: `docs/ARCHITECTURE.md` §1 (issue #215 paragraph)
- Modify: `docs/INSTALL.md` (Metamod floor)
- Modify: `docs/BUILDING.md` (same)
- Modify: `docs/PROGRESS.md` (append the finished-slice entry **after** the live gate, not before)
- Modify: `scripts/cloud/install.sh` (force refresh when `GetDetourInterface` is missing)
- Modify: `scripts/gen-licenses.sh` (drop hardcoded `2.0.0.1403`)
- Modify: `licenses/licenses.txt` via `./scripts/gen-licenses.sh`
- Modify: `CLAUDE.md` / `AGENTS.md` Current state / live-gate Metamod note if they still say 1403

**Interfaces:**
- Produces: operators know they need Metamod ≥ PR #223; architecture thesis names two composition layers.

- [ ] **Step 1: `ARCHITECTURE.md` §1**

Replace the sentence that says frameworks “each ship their own detour engine” and that s2script is the sole arbiter of the detour. New text must say:

- Metamod/KHook owns the one process-wide trampoline (PR #223 / issue #215).
- s2script owns the JS `HookResult` contract among `.s2sp` plugins.
- The shim is a KHook consumer, not a second detour library.

- [ ] **Step 2: `INSTALL.md` / `BUILDING.md`**

```markdown
Metamod:Source 2.0 **built on or after 2026-09-08** (plugin API 18, KHook).
2.0.0.1403 and earlier refuse this runtime (`Plugin uses old SourceHook Metamod build`).
```

- [ ] **Step 3: `install.sh`**

If `strings` on the on-disk `metamod.2.cs2.so` lacks `GetDetourInterface`, delete `docker/metamod` and re-pull (or build from the submodule). Do not skip just because the file exists.

- [ ] **Step 4: Licenses**

```bash
./scripts/gen-licenses.sh
./scripts/check-licenses-generated.sh
```

Expected: Metamod SHA is the new pin; no `2.0.0.1403` label unless that is still true (it must not be).

- [ ] **Step 5: Commit + PR C**

```bash
git add docs/ARCHITECTURE.md docs/INSTALL.md docs/BUILDING.md docs/PROGRESS.md \
        scripts/cloud/install.sh scripts/gen-licenses.sh licenses/licenses.txt
git commit -m "docs: KHook cutover — Metamod floor, two-layer composition"
```

Title: `docs: KHook operator floor and architecture`.
Body: Why. Link A + B. Precache leftover yes/no.

---

## Self-review

**1. Spec coverage**

| Spec section | Task |
|--------------|------|
| §3 approach B / stack | PR map; T1–T8 = A, T9–T11 = B, T12–T13 = C |
| §5.1 Action mapping | T2 helpers; T4 FireEvent / Recall |
| §5.2 interface `Virtual` | T3, T4 |
| §5.3 SDKHooks | T5 |
| §5.4 declarative + named inline | T9, T10 |
| §5.5 precache | T12 |
| §6 hard constraints | Global Constraints + T1 pin + T3 bind-after-SAVEVARS |
| §8 acceptance | T6 compile, T7–T8 live A, T10 live B, T12–T13 C |
| §9 risks | T3 index=-1 banner; T7 submodule-built Metamod; T9 spike |

**2. Placeholder scan:** Task 9 locks typed `Function` per shape + `this_void`/`onRespawn` spike; raw `SetupHook` is fallback only. Precache has a named leftover. No TBD. SDKHooks PRE/POST share one `Virtual`.

**3. Type consistency:** Helpers are `S2_Ignore` / `S2_Supersede` / `S2_FromHookResult`. Virtual objects are `g_hkGameFrame` etc. FFI `s2_sdkhook_vp_add` still returns non-zero on success.

---

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-14-khook-migration.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.

PR A is the only load-blocking work. Do not start PR B until Task 8 is green on a live CS2 server.
