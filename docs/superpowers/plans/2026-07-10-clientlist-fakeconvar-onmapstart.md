# Client-list refactor + FakeConVar + OnMapStart — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** (1) Fix the 2000870 client-list regression permanently by replacing the 6 hand-committed engine-identity offsets with typed SDK virtuals (self-healing); (2) let plugins register their own ConVars (`Server.registerCvar`, CSSharp `FakeConVar` parity); (3) ship a real `Server.onMapStart` framework event (CSSharp `OnMapStart`), replacing the per-plugin map-name poll.

**Architecture:** Feature 1 is shim+gamedata ONLY (the 5 `client_*` op contracts are unchanged — `GetIGameServer()`/`GetPlayerUserId()`/`GetClientConVarValue(slot,"name")` typed virtuals + a lifecycle-hook-tracked signon array replace every offset deref). Feature 2 is ONE new op (`convar_register`, ABI-appended after `user_message_send`) filling `ConVarCreation_t` exactly as `CConVar<T>::Register` does and vtable-calling `ICvar::RegisterConVar` (the proven `RegisterConCommand` pattern). Feature 3 is NO op: a `StartupServer` POST SourceHook (the CSSharp mechanism, verbatim) → a new FFI export `s2script_core_dispatch_map_start` → a core `MAP_MUX` (event_mux reuse) → `Server.onMapStart` (mirrors the client-lifecycle event spine).

**Tech Stack:** Rust core (`core/src/v8host.rs` + `core/src/ffi.rs`, rusty_v8), C++ shim (`shim/src/s2script_mm.cpp` + `shim/src/s2script_mm.h` + `shim/include/s2script_core.h`, hl2sdk cs2 + SourceHook), `gamedata/core.gamedata.jsonc`, TypeScript plugin + `packages/server/index.d.ts`.

## Global Constraints

- **Core owns every engine touchpoint; dependencies game → core only.** Everything in this slice is engine-generic (client identity / convar registration / map start are Source2 concepts; `INetworkServerService`/`IVEngineServer2`/`ICvar` are Source2 engine types → shim-only). Expect ZERO CS2 names in the core/shim diff. Both boundary gates (`scripts/check-core-boundary.sh`, `scripts/test-boundary-nameleak.sh`) must stay green.
- **ABI-append discipline (mandatory).** The slice adds exactly ONE op, `convar_register`, appended **after `user_message_send`** (the current last op — verified against `shim/include/s2script_core.h:189/:278`) — never inserted mid-struct — byte-identical across FIVE touchpoints: (1) the C header typedef + struct field; (2) the Rust `type ConvarRegisterFn` + `pub convar_register: Option<ConvarRegisterFn>` in `core/src/v8host.rs`; (3) **both** in-isolate test op-struct literals (the two that currently end with `user_message_send: None,` — near `core/src/v8host.rs:8523` and `:9035`); (4) the shim `ops.convar_register = &s2_convar_register;`. Feature 1 changes NO op signature; feature 3 adds a FUNCTION EXPORT (like `s2script_core_dispatch_client_event`), not an op.
- **Degrade-never-crash.** Every new native `catch_unwind`s with a safe default set first; every shim path null-guards (`s_pEngine`/`s_pCvar`/`s_pNetworkServerService`/`S2_GameServer()`); the map-start dispatch mirrors `dispatch_client_event`'s `try_borrow_mut` re-entrancy guard + `is_live` + per-handler TryCatch.
- **Tests run serial:** `.cargo/config.toml` sets `RUST_TEST_THREADS=1`. Run with `cd core && cargo test`.
- **The shim C++ is NOT compiled locally** — only at the docker sniper build. Write it carefully; the adversarial reviews are the compile gate proxy.
- **Preserve the observable client-op contracts:** `client_valid` 0/1 connected (incl. bots, incl. pawnless); `client_userid` engine userid or -1; `client_signon` comparable with `>= 2` (connected) and `>= 4` (in-game — the `kickWithReason` deliver-now gate); `client_name` engine-owned string valid during the call (core copies); `client_find_by_userid` slot or -1.
- Git commits use `git commit -F - <<'EOF' … EOF` (never backticks) and end with the Claude-Session trailer shown in each step.

---

## File Structure

- `shim/src/s2script_mm.cpp` — Task 1: rewrite `S2_GameServer` + the 5 `s2_client_*` ops, delete `S2_ClientAt`/`s_off*`/the offset `pick()` block, add signon tracking to the six `Hook_Client*` bodies. Task 2: `s2_convar_register` + registry + `ops.` assignment. Task 3: `SH_DECL_HOOK3_void(INetworkServerService, StartupServer, …)` + `Hook_StartupServer` + add/remove at Load/Unload.
- `shim/src/s2script_mm.h` — Task 3: `Hook_StartupServer` decl + `m_startupServerHookInstalled` + forward decls.
- `shim/include/s2script_core.h` — Task 1: `client_signon` contract comment. Task 2: `s2_convar_register_fn` typedef + field. Task 3: `s2script_core_dispatch_map_start` export decl (next to `s2script_core_dispatch_client_event` at `:306`).
- `gamedata/core.gamedata.jsonc` — Task 1: delete the 6 engine-identity offset entries.
- `core/src/v8host.rs` — Task 2: op mirror + both test structs + `s2_convar_register` native + `Server.registerCvar` prelude + test. Task 3: `MAP_MUX` + `dispatch_map_start` + `s2_map_start_subscribe` native + `Server.onMapStart` prelude + shutdown/unload wiring + test.
- `core/src/ffi.rs` — Task 3: `s2script_core_dispatch_map_start` export.
- `packages/server/index.d.ts` — Tasks 2+3: `registerCvar` + `onMapStart` types.
- `packages/clients/index.d.ts` — Task 1: `signonState` doc-comment refresh (tracked semantics).
- `plugins/clientlist-convar-mapstart-demo/{package.json,tsconfig.json,src/plugin.ts}` — Task 4.

---

## Task 1: Client-list refactor — typed SDK virtuals replace the hand offsets

**Files:**
- Modify: `shim/src/s2script_mm.cpp` (the engine-identity section `:505-596`, `S2_GameServer` `:944`, the six `Hook_Client*` bodies, the offset `pick()` block `:2252-2271`)
- Modify: `shim/include/s2script_core.h` (`client_signon` comment only — line 48)
- Modify: `gamedata/core.gamedata.jsonc` (delete 6 offsets)
- Modify: `packages/clients/index.d.ts` (`signonState` doc comment)

**Interfaces:**
- Consumes: `INetworkServerService::GetIGameServer()` (`third_party/hl2sdk/public/iserver.h:218`), `IVEngineServer2::GetPlayerUserId`/`GetClientConVarValue` (`public/eiface.h:217/:247`), the six existing `ISource2GameClients` lifecycle hooks.
- Produces: the SAME five ops (`client_valid`/`client_userid`/`client_signon`/`client_name`/`client_find_by_userid`) with unchanged signatures + contracts; NO core/ABI change.

- [ ] **Step 1: Rewrite `S2_GameServer` and delete the later duplicate.** In `shim/src/s2script_mm.cpp`, the engine-identity section (currently `:505-596`) is rewritten. Replace the statics + `S2_ClientAt` + the five ops with:

```cpp
// ---------------------------------------------------------------------------
// Engine-identity: TYPED SDK VIRTUALS (clientlist-fakeconvar-onmapstart slice — replaces the
// 5D.2 hand-offset walk that went stale on 2000870). Self-healing: every read below is a
// compiler-resolved virtual against the pinned hl2sdk headers, which the update treadmill
// already bumps per patch — an engine layout change is fixed by the SDK bump alone (no
// gamedata, no shim edit). CSSharp-cross-validated (their player_manager reads userids via
// GetPlayerUserId and never touches CServerSideClient).
// s_pNetworkServerService acquired in Load(). Degrade-never-crash: null -> safe miss.
// ---------------------------------------------------------------------------
static void* s_pNetworkServerService = nullptr;

// The one client fact with NO engine virtual: per-slot signon state. Tracked from the six
// ALREADY-INSTALLED ISource2GameClients lifecycle hooks (the CSSharp model — lifecycle-driven
// state, not struct reads). Values preserve the two observable JS gates: >= 2 = connected
// (Player.allConnected-era kSignonConnected), >= 4 = in-game (Client.kickWithReason deliver-now).
static const int kSignonNone = 0, kSignonConnected = 2, kSignonSpawn = 5, kSignonFull = 6;
static const int kMaxClientSlots = 64;
static int s_trackedSignon[kMaxClientSlots] = {0};

// INetworkGameServer via the TYPED virtual (replaces the NetworkServerService.gameServer offset).
// CNetworkGameServerBase (the GetIGameServer return type) IS-A INetworkGameServer.
static INetworkGameServer* S2_GameServer() {
    if (!s_pNetworkServerService) return nullptr;
    return static_cast<INetworkServerService*>(s_pNetworkServerService)->GetIGameServer();
}

static int s2_client_userid(int slot) {
    if (!s_pEngine || slot < 0 || slot >= kMaxClientSlots) return -1;
    return s_pEngine->GetPlayerUserId(CPlayerSlot(slot)).Get();   // -1 when no player at slot
}
static int s2_client_valid(int slot) {
    return s2_client_userid(slot) != -1 ? 1 : 0;   // engine-authoritative; bots included
}
static int s2_client_signon(int slot) {
    if (slot < 0 || slot >= kMaxClientSlots) return -1;
    return s_trackedSignon[slot];                  // 0 = never-connected / disconnected
}
static const char* s2_client_name(int slot) {
    if (!s_pEngine || !s2_client_valid(slot)) return nullptr;
    return s_pEngine->GetClientConVarValue(CPlayerSlot(slot), "name");   // userinfo name; core copies
}
static int s2_client_find_by_userid(int id) {
    if (id < 0) return -1;
    INetworkGameServer* gs = S2_GameServer();
    int n = gs ? gs->GetMaxClients() : kMaxClientSlots;
    if (n <= 0 || n > kMaxClientSlots) n = kMaxClientSlots;
    for (int slot = 0; slot < n; slot++) {
        if (s2_client_userid(slot) == id) return slot;
    }
    return -1;
}
```

Then DELETE the old `S2_GameServer()` definition at `:944-947` (the offset-deref version) — the server-info ops (`s2_server_max_clients`/`map_name`/`game_time`) call `S2_GameServer()` and now transparently use the typed path. NOTE the section-order constraint: `s_pEngine` (declared ~`:537`) must be declared BEFORE the ops that now use it — keep the existing declaration order (the identity block already sits after the `s_pCvar`/`s_pEngine` declarations… it does NOT: `s_pEngine` is declared at `:537`, AFTER `:505`. Move the whole rewritten identity block to just AFTER the `s_pEngine` declaration, or forward-declare `static IVEngineServer2* s_pEngine;` above it. Prefer moving the block — cleaner).

- [ ] **Step 2: Delete every offset consumer.** Remove: the `s_offGameServer/s_offClientCount/s_offClientElems/s_offSscName/s_offSscSignon/s_offSscUserId` statics (`:510-511`), `S2_ClientAt` (`:548-556`), and in the Load() gamedata block (`:2252-2271`) the six `pick("NetworkServerService.gameServer")`…`pick("ServerSideClient.userId")` lines, the `identity offsets:` `META_CONPRINTF`, and the six-entry `GamedataResult` loop. Keep the surrounding block (it still loads `CBaseEntity_Teleport` etc.). Verify with `grep -n "S2_ClientAt\|s_offGameServer\|s_offClientCount\|s_offClientElems\|s_offSsc" shim/src/s2script_mm.cpp` → 0 hits (the `s2_event_fire_to_client` doc comment at `:592` mentions `S2_ClientAt` — reword it to reference `s2_client_valid`).

- [ ] **Step 3: Signon tracking in the six existing hook bodies.** State is set BEFORE the core dispatch (handlers observe the new state); disconnect clears AFTER dispatch:

```cpp
void S2ScriptPlugin::Hook_OnClientConnected(CPlayerSlot slot, const char*, uint64, const char*, const char*, bool) {
    int s = slot.Get();
    if (s >= 0 && s < kMaxClientSlots) s_trackedSignon[s] = kSignonConnected;
    s2script_core_dispatch_client_event("connect", s);
    RETURN_META(MRES_IGNORED);
}
```
Same pattern: `Hook_ClientPutInServer` → `kSignonSpawn`; `Hook_ClientActive` → `kSignonFull`; `Hook_ClientFullyConnect` → `kSignonFull`; `Hook_ClientDisconnect` → dispatch `"disconnect"` FIRST, then `s_trackedSignon[s] = kSignonNone;`. `Hook_ClientSettingsChanged` — untouched.

- [ ] **Step 4: gamedata cleanup.** In `gamedata/core.gamedata.jsonc`, delete the six `.offsets` entries `NetworkServerService.gameServer`, `NetworkGameServer.clientCount`, `NetworkGameServer.clientElems`, `ServerSideClient.name`, `ServerSideClient.signon`, `ServerSideClient.userId` (and their 5D.2 comment block; the surrounding `GameEntitySystem`/`CNavPhysicsInterface_TraceShape`/`CBaseEntity_Teleport`/item entries stay). The GAMEDATA VALIDATION count drops by 6 — note it for the live gate.

- [ ] **Step 5: Contract comments.** In `shim/include/s2script_core.h:48`, update the comment: `typedef int (*s2_client_signon_fn)(int slot); /* tracked signon: 0 none/disconnected, 2 connected, 5 spawned, 6 full in-game; -1 slot OOB */`. In `packages/clients/index.d.ts`, refresh the `signonState` doc comment to the same values (the `>= 4` in-game reading is unchanged for consumers).

- [ ] **Step 6: Verify.** `cd core && cargo test` → all green (no core change — this is a regression sanity run). `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → green.

- [ ] **Step 7: Commit**
```bash
git add shim/src/s2script_mm.cpp shim/include/s2script_core.h gamedata/core.gamedata.jsonc packages/clients/index.d.ts
git commit -F - <<'EOF'
fix(clients): client-list via typed SDK virtuals — retire the 5D.2 hand offsets

GetIGameServer()/GetPlayerUserId()/GetClientConVarValue(slot,"name") replace the
6 gamedata offsets that went stale on 2000870 (offsets are never re-scanned, so
the validation gate could not catch them); signon is tracked from the six
existing lifecycle hooks (2/5/6, preserving the >=2 and >=4 gates). Self-healing:
future layout moves are fixed by the routine hl2sdk bump alone. Shim+gamedata
only — the 5 client-op contracts and the core are unchanged.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 2: FakeConVar — `convar_register` op + `Server.registerCvar`

**Files:**
- Modify: `shim/include/s2script_core.h` (typedef + struct field after `user_message_send`)
- Modify: `core/src/v8host.rs` (op mirror + both test structs + native + prelude + test)
- Modify: `shim/src/s2script_mm.cpp` (`s2_convar_register` + registry + `ops.` assignment)
- Modify: `packages/server/index.d.ts`

**Interfaces:**
- Produces: op `int convar_register(const char* name, const char* help, uint64_t flags, int type, const char* defaultValue, const char* minValue, const char* maxValue)` (type: 0=bool 1=int32 2=float32 3=string; min/max nullable, numeric only; returns 1 ok/already, 0 fail); native `__s2_convar_register(...) -> i32`; `Server.registerCvar(name, opts) -> boolean`.

- [ ] **Step 1: C header** — in `shim/include/s2script_core.h`, after the `s2_user_message_send_fn` typedef:
```c
/* convar_register: register a plugin-owned ConVar via ICvar::RegisterConVar (FakeConVar slice).
 * type: 0=bool 1=int32 2=float32 3=string. defaultValue always a string (shim parses).
 * minValue/maxValue: nullable, numeric types only. FCVAR_RELEASE is OR'd shim-side.
 * Returns 1 registered (or already registered — idempotent), 0 fail. */
typedef int (*s2_convar_register_fn)(const char* name, const char* help, uint64_t flags, int type,
                                     const char* defaultValue, const char* minValue, const char* maxValue);
```
and in `struct S2EngineOps` after `s2_user_message_send_fn user_message_send;`:
```c
    /* FakeConVar (clientlist-fakeconvar-onmapstart slice) — APPENDED after user_message_send; order is the ABI. */
    s2_convar_register_fn convar_register;
```

- [ ] **Step 2: Rust op mirror** — in `core/src/v8host.rs`, next to `UserMessageSendFn`:
```rust
type ConvarRegisterFn = unsafe extern "C" fn(
    *const std::os::raw::c_char, *const std::os::raw::c_char, u64, i32,
    *const std::os::raw::c_char, *const std::os::raw::c_char, *const std::os::raw::c_char) -> i32;
```
and in `pub struct S2EngineOps` after `pub user_message_send: Option<UserMessageSendFn>,`:
```rust
    pub convar_register: Option<ConvarRegisterFn>,
```
Add `convar_register: None,` to **both** in-isolate test op-struct literals (the two that end `user_message_send: None,`, near `:8523` and `:9035`).

- [ ] **Step 3: Write the failing test** (same harness as the existing degrade tests — `init`/`set_engine_ops(None)`/`create_plugin_context`/`eval_in_context_string`/`shutdown`):
```rust
#[test]
fn register_cvar_degrades_false_without_op() {
    let _ = init(dummy_logger());
    set_engine_ops(None);
    create_plugin_context("pcv");
    let out = eval_in_context_string("pcv", r#"
        var a = __s2pkg_server.Server.registerCvar("s2_test_cvar", { type: "int", default: 42, min: 0, max: 100 });
        var b = __s2pkg_server.Server.registerCvar("s2_bad", { type: "nope", default: 1 });
        String(a === false && b === false)
    "#);
    assert_eq!(out, "true");
    shutdown();
}
```

- [ ] **Step 4: Run it — expect FAIL** — `cd core && cargo test register_cvar_degrades` → FAIL (`registerCvar` undefined).

- [ ] **Step 5: Core native** — in `core/src/v8host.rs` (model on the existing multi-string natives; nullable args pass a null pointer):
```rust
/// Native `__s2_convar_register(name, helpOrNull, flags, type, defaultStr, minOrNull, maxOrNull) -> i32`.
/// Over the `convar_register` op. Degrades to 0 with no op; never throws.
fn s2_convar_register(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_int32(0);
        let ops = ENGINE_OPS.with(|o| o.get());
        let Some(func) = ops.and_then(|o| o.convar_register) else { return };
        let name = args.get(0).to_rust_string_lossy(scope);
        let Ok(c_name) = std::ffi::CString::new(name) else { return };
        // helpOrNull / minOrNull / maxOrNull: JS null/undefined -> C null pointer.
        let opt_cstr = |scope: &mut v8::PinScope, v: v8::Local<v8::Value>| -> Option<std::ffi::CString> {
            if v.is_null_or_undefined() { return None; }
            std::ffi::CString::new(v.to_rust_string_lossy(scope)).ok()
        };
        let c_help = opt_cstr(scope, args.get(1));
        let flags = args.get(2).number_value(scope).unwrap_or(0.0) as u64;
        let ty = args.get(3).int32_value(scope).unwrap_or(-1);
        let def = args.get(4).to_rust_string_lossy(scope);
        let Ok(c_def) = std::ffi::CString::new(def) else { return };
        let c_min = opt_cstr(scope, args.get(5));
        let c_max = opt_cstr(scope, args.get(6));
        let r = unsafe {
            func(c_name.as_ptr(),
                 c_help.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                 flags, ty, c_def.as_ptr(),
                 c_min.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                 c_max.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()))
        };
        rv.set_int32(r);
    }));
}
```
Register it: `set_native(scope, global_obj, "__s2_convar_register", s2_convar_register);` (next to `__s2_cvar_get`). NOTE: if the existing natives in this file take `v8::Local<v8::Value>` differently for the helper closure, inline the null-checks per-arg instead of a closure — match the file's local style; the observable behavior above is the contract.

- [ ] **Step 6: Prelude** — in `core/src/v8host.rs`, add to the `var __s2_server = { … }` object literal (after `setCvar`):
```js
    // Register a plugin-owned ConVar (FakeConVar). Type-checked JS-side; the shim ORs FCVAR_RELEASE.
    // Value reads reuse getCvar; writes reuse setCvar/console. Idempotent (reload-safe); the cvar and
    // its value persist for the process lifetime (SourceMod parity).
    registerCvar: function (name, opts) {
      opts = opts || {};
      var tmap = { bool: 0, int: 1, float: 2, string: 3 };
      var type = tmap[String(opts.type == null ? "string" : opts.type)];
      if (type === undefined) return false;
      var def = opts.default;
      var defStr = (type === 0) ? (def ? "1" : "0")
                                : String(def == null ? (type === 3 ? "" : 0) : def);
      return __s2_convar_register(String(name),
        opts.help == null ? null : String(opts.help),
        opts.flags == null ? 0 : +opts.flags, type, defStr,
        opts.min == null ? null : String(opts.min),
        opts.max == null ? null : String(opts.max)) === 1;
    },
```

- [ ] **Step 7: Run the test — expect PASS** — `cd core && cargo test register_cvar_degrades` → PASS. Then `cd core && cargo test` → all green.

- [ ] **Step 8: Shim impl** — in `shim/src/s2script_mm.cpp`, directly below `s2_concommand_register` (same section — it reuses `s_pCvar`):
```cpp
// ---------------------------------------------------------------------------
// FakeConVar registration (clientlist-fakeconvar-onmapstart slice). Fills ConVarCreation_t exactly
// as the SDK's CConVar<T>::Register does (name/help/flags + ConVarValueInfo_t(type) + typed
// SetDefaultValue/SetMinValue/SetMaxValue; m_Version stays 0 — verified in tier1/convar.cpp, which
// passes the struct through to ICvar::RegisterConVar untouched), then vtable-calls RegisterConVar
// directly on s_pCvar — bypassing the NON-INLINE tier1 SetupConVar/SanitiseConVarFlags (the 5D.1
// dlopen cascade), the same call-the-interface-directly pattern as RegisterConCommand (6.1).
// s_convarRefs is the name-lifetime anchor + idempotency guard (mirrors s_concommandRefs): the map
// key owns m_pszName's storage; help/default strings persist in the entry. No unregistration — a
// registered cvar persists for the process lifetime (SM parity; no callback into plugin code, so
// no UAF surface on plugin reload).
// ---------------------------------------------------------------------------
struct S2ConVarReg { ConVarRef ref; std::string help; std::string defVal; };
static std::map<std::string, S2ConVarReg> s_convarRefs;
// CVValue_t's string arm is a CUtlString == exactly one char* member; every CUtlString mutator is
// DLL_CLASS_IMPORT (tier0). We therefore set a string default by writing the pointer bytes directly
// (SetDefaultValue<const char*>) into the value slot — guarded here. The engine copies the value
// into its own ConVarData at registration; our buffer additionally persists in s_convarRefs.
static_assert(sizeof(CUtlString) == sizeof(const char*),
              "CUtlString layout changed — string convar default punning is invalid");

static int s2_convar_register(const char* name, const char* help, uint64_t flags, int type,
                              const char* defaultValue, const char* minValue, const char* maxValue) {
    if (!name || !name[0] || !defaultValue) return 0;
    if (!s_pCvar) {
        META_CONPRINTF("[s2script] WARN: ConVar '%s' not registered — ICvar not acquired\n", name);
        return 0;
    }
    auto found = s_convarRefs.find(name);
    if (found != s_convarRefs.end()) return found->second.ref.IsValidRef() ? 1 : 0;  // idempotent (reload-safe)

    auto it = s_convarRefs.emplace(name, S2ConVarReg{}).first;
    S2ConVarReg& reg = it->second;
    reg.help   = help ? help : "s2script convar";
    reg.defVal = defaultValue;

    ConVarCreation_t setup;
    setup.m_pszName       = it->first.c_str();      // persistent (map key) — name-lifetime anchor
    setup.m_pszHelpString = reg.help.c_str();       // persistent (entry)
    // FCVAR_RELEASE: without it a retail CS2 hides the cvar from customers. Caller flags are additive.
    setup.m_nFlags        = flags | FCVAR_RELEASE;

    switch (type) {
        case 0: {   // bool
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_Bool);
            bool v = (reg.defVal == "1" || reg.defVal == "true");
            setup.m_valueInfo.SetDefaultValue<bool>(v);
            break;
        }
        case 1: {   // int32
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_Int32);
            setup.m_valueInfo.SetDefaultValue<int32>(atoi(reg.defVal.c_str()));
            if (minValue) setup.m_valueInfo.SetMinValue<int32>(atoi(minValue));
            if (maxValue) setup.m_valueInfo.SetMaxValue<int32>(atoi(maxValue));
            break;
        }
        case 2: {   // float32
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_Float32);
            setup.m_valueInfo.SetDefaultValue<float>(static_cast<float>(atof(reg.defVal.c_str())));
            if (minValue) setup.m_valueInfo.SetMinValue<float>(static_cast<float>(atof(minValue)));
            if (maxValue) setup.m_valueInfo.SetMaxValue<float>(static_cast<float>(atof(maxValue)));
            break;
        }
        case 3: {   // string — pointer punning into the CUtlString slot (see static_assert above)
            setup.m_valueInfo = ConVarValueInfo_t(EConVarType_String);
            setup.m_valueInfo.SetDefaultValue<const char*>(reg.defVal.c_str());  // persistent buffer
            break;
        }
        default:
            s_convarRefs.erase(it);
            return 0;
    }

    ConVarRef ref;
    ConVarData* data = nullptr;
    s_pCvar->RegisterConVar(setup, 0, &ref, &data);
    reg.ref = ref;
    if (ref.IsValidRef()) {
        META_CONPRINTF("[s2script] ConVar '%s' registered (accessIdx=%u)\n",
                       it->first.c_str(), (unsigned)ref.GetAccessIndex());
        return 1;
    }
    META_CONPRINTF("[s2script] WARN: ConVar '%s' — RegisterConVar returned invalid ref\n", it->first.c_str());
    return 0;   // entry stays in the map (invalid ref) to prevent retry loops
}
```
Wire it where the other assignments live, in ABI order (after `ops.user_message_send`): `ops.convar_register = &s2_convar_register;`. (`FCVAR_RELEASE`, `ConVarCreation_t`, `ConVarValueInfo_t`, `EConVarType_*`, `ConVarRef`, `ConVarData`, `CUtlString` all come from the already-included `tier1/convar.h`/`icvar.h` — the ConCommand path uses the same headers.)

- [ ] **Step 9: Types** — in `packages/server/index.d.ts`, add after `setCvar`:
```ts
  /**
   * Register a plugin-owned ConVar (CSSharp FakeConVar / SM CreateConVar parity). Idempotent —
   * re-registering an existing name is a no-op success, and the cvar + its value persist across
   * plugin reloads (SourceMod parity). The shim adds FCVAR_RELEASE (customer-visible); `flags`
   * are additive raw FCVAR bits. Read the value with `getCvar`; set it with `setCvar`/the console.
   * `min`/`max` apply to numeric types only.
   */
  registerCvar(name: string, opts: {
    type: "bool" | "int" | "float" | "string";
    default: boolean | number | string;
    help?: string;
    flags?: number;
    min?: number;
    max?: number;
  }): boolean;
```

- [ ] **Step 10: Commit**
```bash
git add shim/include/s2script_core.h core/src/v8host.rs shim/src/s2script_mm.cpp packages/server/index.d.ts
git commit -F - <<'EOF'
feat(server): Server.registerCvar — plugin-owned ConVars (FakeConVar parity)

One convar_register op (ABI-appended after user_message_send): fill
ConVarCreation_t as CConVar<T>::Register does and vtable-call
ICvar::RegisterConVar directly (no tier1 SetupConVar — the 5D.1 cascade).
bool/int32/float32/string + numeric min/max; string defaults via const char*
punning into the CUtlString value slot (static_assert-guarded); FCVAR_RELEASE
OR'd. Name-keyed persistent registry (idempotent, reload-safe). Degrades to
false with no op.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 3: OnMapStart — StartupServer hook → `dispatch_map_start` → `Server.onMapStart`

**Files:**
- Modify: `core/src/v8host.rs` (`MAP_MUX` + `dispatch_map_start` + native + prelude + shutdown/unload wiring + test)
- Modify: `core/src/ffi.rs` (`s2script_core_dispatch_map_start`)
- Modify: `shim/include/s2script_core.h` (the export decl)
- Modify: `shim/src/s2script_mm.h` (hook decl + flag + forward decls)
- Modify: `shim/src/s2script_mm.cpp` (SH_DECL + hook body + add/remove)
- Modify: `packages/server/index.d.ts`

**Interfaces:**
- Consumes: `S2_GameServer()` (Task 1's typed version) for the map name; the `event_mux::EventMux` + `dispatch_client_event` pattern.
- Produces: FFI export `void s2script_core_dispatch_map_start(const char* map)`; native `__s2_map_start_subscribe(handler)`; `Server.onMapStart(handler: (mapName: string) => void)`.

- [ ] **Step 1: `MAP_MUX`** — in `core/src/v8host.rs`, beside the `CLIENT_MUX` thread_local (`:467`), add in the same `thread_local!` block:
```rust
    /// Map-start subscribers (clientlist-fakeconvar-onmapstart slice). Fixed key "" (map-start has
    /// no name dimension, like CHAT_MSG_SUBS); notify-only.
    static MAP_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
        = std::cell::RefCell::new(crate::event_mux::EventMux::new());
```

- [ ] **Step 2: Write the failing test:**
```rust
/// dispatch_map_start delivers the map name to a Server.onMapStart subscriber (the MAP_MUX reuse +
/// the string-arg dispatch); mirrors client_event_dispatch_reaches_subscriber.
#[test]
fn map_start_dispatch_delivers_map_name() {
    let _ = init(dummy_logger());
    set_engine_ops(None);
    create_plugin_context("pms");
    eval_in_context_string("pms", r#"
        globalThis.__map = "";
        __s2pkg_server.Server.onMapStart(function (m) { globalThis.__map = m; });
        "ok"
    "#);
    dispatch_map_start("de_test");
    assert_eq!(eval_in_context_string("pms", "globalThis.__map"), "de_test");
    shutdown();
}
```

- [ ] **Step 3: Run it — expect FAIL** — `cd core && cargo test map_start_dispatch` → FAIL.

- [ ] **Step 4: `dispatch_map_start`** — in `core/src/v8host.rs`, directly below `dispatch_client_event` (`:2915`), a verbatim mirror with a string arg:
```rust
/// Deliver a map-start notification to the `Server.onMapStart` subscribers. Called from ffi.rs's
/// `s2script_core_dispatch_map_start` (the shim's INetworkServerService::StartupServer POST hook).
/// Mirrors `dispatch_client_event` verbatim: snapshot (release the mux borrow), `try_borrow_mut`
/// re-entrancy guard, per-subscriber `is_live` + context clone + HandleScope/ContextScope/TryCatch +
/// WARN-on-throw. Notify-only — each handler is called with the single String `map` and its return
/// is ignored.
pub(crate) fn dispatch_map_start(map: &str) {
    let snap = MAP_MUX.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return; }

    HOST.with(|h| {
        let Ok(mut borrow) = h.try_borrow_mut() else { return };
        let Some(host) = borrow.as_mut() else { return };

        for (owner, generation, handler_g) in &snap {
            if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
            let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };

            let mut hs_storage = v8::HandleScope::new(&mut host.isolate);
            let mut hs = unsafe { std::pin::Pin::new_unchecked(&mut hs_storage) }.init();
            let hs = &mut hs;
            let ctx_local = v8::Local::new(hs, &g_ctx);
            let scope = &mut v8::ContextScope::new(hs, ctx_local);

            let mut tc_storage = v8::TryCatch::new(scope);
            let mut tc = unsafe { std::pin::Pin::new_unchecked(&mut tc_storage) }.init();
            let tc = &mut tc;

            let recv: v8::Local<v8::Value> = v8::undefined(tc).into();
            let map_val: v8::Local<v8::Value> = match v8::String::new(tc, map) {
                Some(s) => s.into(),
                None => continue,
            };
            let func = v8::Local::new(tc, handler_g);
            if func.call(tc, recv, &[map_val]).is_none() {
                let msg = tc.exception()
                    .map(|e| e.to_rust_string_lossy(&*tc))
                    .unwrap_or_else(|| "handler threw".into());
                log_warn(&format!("WARN: dispatch_map_start: handler '{}': {}", owner, msg));
            }
        }
    });
}
```

- [ ] **Step 5: Subscribe native** — a verbatim mirror of `s2_chat_on_message` (`:3632`) over `MAP_MUX`:
```rust
/// `__s2_map_start_subscribe(handler)` — subscribe a JS fn to the map-start event. Owner-tracked
/// (mirrors `__s2_chat_on_message`); fixed mux key "". The handler receives the map name string.
fn s2_map_start_subscribe(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 1 { return; }
        let Ok(func_local) = v8::Local::<v8::Function>::try_from(args.get(0)) else { return };
        let handler_g = v8::Global::new(scope.as_ref(), func_local);
        let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());
        let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);
        MAP_MUX.with(|m| { m.borrow_mut().subscribe("", owner, generation, handler_g); });
    }));
}
```
Register it: `set_native(scope, global_obj, "__s2_map_start_subscribe", s2_map_start_subscribe);` (next to `__s2_client_subscribe` at `:5151`).

- [ ] **Step 6: Teardown wiring** — beside the `CLIENT_MUX` reset in `shutdown()` (`:6687`): `MAP_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());` — and beside the `CLIENT_MUX` `remove_by_owner` in plugin unload (`:6970`): `MAP_MUX.with(|m| m.borrow_mut().remove_by_owner(id));`.

- [ ] **Step 7: Prelude** — add to the `var __s2_server = { … }` object literal (after `registerCvar`):
```js
    // Subscribe to map start (the framework event replacing the Server.mapName OnGameFrame poll).
    // Fires on every StartupServer (boot-loaded plugins get the first map); a plugin hot-loaded
    // mid-map should read Server.mapName at load for the CURRENT map. Handlers may be async
    // (fire-and-forget). Auto-ledgered per plugin; torn down on unload.
    onMapStart: function (h) { __s2_map_start_subscribe(h); },
```

- [ ] **Step 8: FFI export** — in `core/src/ffi.rs`, next to `s2script_core_dispatch_client_event` (`:95`):
```rust
/// Shim → core: the INetworkServerService::StartupServer POST hook reports a map start with the
/// live map name. Notify-only: dispatches to the `Server.onMapStart` JS subscribers.
/// `catch_unwind`-wrapped; a null pointer degrades to "" (never panic across the FFI boundary).
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_map_start(map: *const c_char) {
    let _ = catch_unwind(|| {
        let map_str = if map.is_null() { "" } else {
            (unsafe { CStr::from_ptr(map) }).to_str().unwrap_or("")
        };
        v8host::dispatch_map_start(map_str);
    });
}
```
And declare it in `shim/include/s2script_core.h` next to `s2script_core_dispatch_client_event` (`:306`):
```c
void s2script_core_dispatch_map_start(const char* map);
```

- [ ] **Step 9: Run the test — expect PASS** — `cd core && cargo test map_start_dispatch` → PASS; `cd core && cargo test` → all green.

- [ ] **Step 10: Shim hook.** In `shim/src/s2script_mm.h`: forward decls near the existing ones (`class INetworkServerService; class GameSessionConfiguration_t; class ISource2WorldSession;`), the handler decl next to `Hook_ClientSettingsChanged`:
```cpp
    // Map-start hook (clientlist-fakeconvar-onmapstart slice) — POST hook on
    // INetworkServerService::StartupServer (the CSSharp OnMapStart mechanism). Reads the live map
    // name off the (typed) game server and forwards to s2script_core_dispatch_map_start.
    void Hook_StartupServer(const GameSessionConfiguration_t& config, ISource2WorldSession* session,
                            const char* unk);
```
and the member flag next to `m_clientLifecycleHooksInstalled`: `bool m_startupServerHookInstalled = false;`.

In `shim/src/s2script_mm.cpp`: the SH_DECL next to the six lifecycle decls (`:92-97`) — verbatim CSSharp (`mm_plugin.cpp:82`):
```cpp
SH_DECL_HOOK3_void(INetworkServerService, StartupServer, SH_NOATTRIB, 0, const GameSessionConfiguration_t&, ISource2WorldSession*, const char*);
```
(Signature confirmed against OUR `iserver.h:221`.) The hook body (place near the other `Hook_Client*` bodies):
```cpp
// POST StartupServer = the map is starting up on a live, named game server (CSSharp reads the map
// name in its POST hook the same way). Also doubles as the client-list slice's boot sanity line —
// a garbage GetIGameServer()/GetMapName()/GetMaxClients() vtable read would be visible here.
void S2ScriptPlugin::Hook_StartupServer(const GameSessionConfiguration_t&, ISource2WorldSession*, const char*) {
    INetworkGameServer* gs = S2_GameServer();
    const char* map = gs ? gs->GetMapName() : nullptr;
    META_CONPRINTF("[s2script] map start: %s (maxClients=%d)\n",
                   map ? map : "<null>", gs ? gs->GetMaxClients() : -1);
    s2script_core_dispatch_map_start(map ? map : "");
    RETURN_META(MRES_IGNORED);
}
```
Install at Load, directly after the `NetworkServerService` acquisition block (`:1947-1959`), inside its success path:
```cpp
            if (s_pNetworkServerService && ret == 0) {
                META_CONPRINTF("[s2script] interface OK: NetworkServerService (%s)\n", verStr);
                SH_ADD_HOOK(INetworkServerService, StartupServer,
                            static_cast<INetworkServerService*>(s_pNetworkServerService),
                            SH_MEMBER(this, &S2ScriptPlugin::Hook_StartupServer), true);   // POST
                m_startupServerHookInstalled = true;
            } else { … }
```
Remove at Unload, beside the lifecycle-hook removals (`:2495+`):
```cpp
    if (m_startupServerHookInstalled && s_pNetworkServerService) {
        SH_REMOVE_HOOK(INetworkServerService, StartupServer,
                       static_cast<INetworkServerService*>(s_pNetworkServerService),
                       SH_MEMBER(this, &S2ScriptPlugin::Hook_StartupServer), true);
        m_startupServerHookInstalled = false;
    }
```

- [ ] **Step 11: Types** — in `packages/server/index.d.ts`, after `registerCvar`:
```ts
  /**
   * Subscribe to map start (fires on every server map startup — the framework event that replaces
   * polling `Server.mapName` on OnGameFrame). Boot-loaded plugins receive the first map's fire;
   * a plugin hot-loaded mid-map does NOT get a synthetic fire for the current map — read
   * `Server.mapName` at load. Torn down automatically on plugin unload.
   */
  onMapStart(handler: (mapName: string) => void): void;
```

- [ ] **Step 12: Gates** — `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh` → green.

- [ ] **Step 13: Commit**
```bash
git add core/src/v8host.rs core/src/ffi.rs shim/include/s2script_core.h shim/src/s2script_mm.h shim/src/s2script_mm.cpp packages/server/index.d.ts
git commit -F - <<'EOF'
feat(server): Server.onMapStart — the framework map-start event

A POST SourceHook on INetworkServerService::StartupServer (the CSSharp
OnMapStart mechanism; signature confirmed against our iserver.h) -> a new FFI
export s2script_core_dispatch_map_start -> MAP_MUX (event_mux reuse, fixed ""
key) -> Server.onMapStart(handler(mapName)). Dispatch mirrors
dispatch_client_event verbatim (direct, notify-only, try_borrow_mut +
is_live + TryCatch); torn down on unload/shutdown. No new op — a hook + FFI
export, like the client-lifecycle events. Replaces the per-plugin mapName
poll for NEW consumers (existing pollers untouched).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Task 4: Demo plugin + full typecheck

**Files:**
- Create: `plugins/clientlist-convar-mapstart-demo/package.json`, `tsconfig.json`, `src/plugin.ts`

**Interfaces:**
- Consumes: `Clients.all()`/`Client` (feature 1 via the unchanged ops), `Server.registerCvar`/`getCvar` (feature 2), `Server.onMapStart` (feature 3), `Player.allConnected`/`fromUserId` (the CS2-level regression proof).

- [ ] **Step 1: Scaffold** — copy the shape of `plugins/gamerules-usermsg-demo/`: `package.json` (name `clientlist-convar-mapstart-demo`, same `s2script.apiVersion`, `pluginDependencies`: `@s2script/commands`, `@s2script/server`, `@s2script/clients`, `@s2script/cs2`), `tsconfig.json` extends `../../tsconfig.base.json`.

- [ ] **Step 2: `src/plugin.ts`**
```ts
import { Commands } from "@s2script/commands";
import { Server } from "@s2script/server";
import { Clients } from "@s2script/clients";
import { Player } from "@s2script/cs2";

export function onLoad(): void {
  // Feature 2: FakeConVar — register at load; read back through the 6.7 cvar_get path.
  const ok = Server.registerCvar("s2_demo_mode", {
    type: "int", default: 42, help: "clientlist-convar-mapstart demo cvar", min: 0, max: 100,
  });
  console.log(`[cl-demo] registerCvar s2_demo_mode -> ${ok} value=${Server.getCvar("s2_demo_mode")}`);

  // Feature 3: OnMapStart — boot-loaded plugins see the first map's fire; changelevel fires again.
  Server.onMapStart((map) => {
    console.log(`[cl-demo] onMapStart: ${map}`);
  });

  // Feature 1: the client list through the refactored ops (engine-generic Clients + CS2 Player).
  Commands.register("sm_clients", (ctx) => {
    const cs = Clients.all();
    ctx.reply(`[cl-demo] clients=${cs.length} players=${Player.allConnected().length} map=${Server.mapName}`);
    for (const c of cs) {
      const back = Player.fromUserId(c.userId);
      ctx.reply(`  slot=${c.slot} name=${c.name} userId=${c.userId} signon=${c.signonState} ` +
                `steamid=${c.steamId} fromUserId->slot=${back ? back.slot : -1}`);
    }
  });

  console.log("[cl-demo] onLoad — sm_clients registered");
}
```

- [ ] **Step 3: Build** — the s2script CLI is local (not published — `npx s2script` 404s). Build the CLI once then the demo: `( cd packages/cli && node build.mjs ) && node packages/cli/dist/cli.js build plugins/clientlist-convar-mapstart-demo` → produces the `.s2sp` (the full-strict typecheck gate must pass, which also validates the two new `.d.ts` surfaces).

- [ ] **Step 4: Typecheck all** — `bash scripts/check-plugins-typecheck.sh` → all plugins green.

- [ ] **Step 5: Commit**
```bash
git add plugins/clientlist-convar-mapstart-demo
git commit -F - <<'EOF'
feat(demo): clientlist-convar-mapstart-demo (sm_clients + registerCvar + onMapStart)

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Build, live gate, and merge (after the Workflow)

Not a Workflow task — the human-in-the-loop integration step.

- [ ] **Core tests** — `cd core && cargo test` → all green (serial).
- [ ] **Sniper rebuild** — `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` (core `.so` + shim `.so` — the new op + FFI export + shim refactor all need it).
- [ ] **Re-deploy** — recreate `dist/addons/s2script/configs` as gkh (the sniper build wipes the addon dir), copy the active plugins' `.s2sp` (incl. the new demo) into `dist/addons/s2script/plugins/`, copy the updated `gamedata/core.gamedata.jsonc`.
- [ ] **Restart** — `cd docker && docker compose restart cs2` (NOT `--force-recreate`); re-run `docker exec s2script-cs2 /patch-gameinfo.sh` only if `gameinfo.gi` was reset.
- [ ] **Live gate** (de_inferno → de_dust2, `bot_quota 2`, `scripts/rcon.py`) — all three features are bots-provable:
  - **Boot:** `interface OK: NetworkServerService`; **NO** `identity offsets:` line; `[s2script] map start: de_inferno (maxClients=N)` (the StartupServer hook + the typed-vtable sanity in one line — a garbage vtable would print garbage/crash here, before any command); `=== GAMEDATA VALIDATION: (N−6) ok, 0 FAILED ===`; `[cl-demo] registerCvar s2_demo_mode -> true value=42`; `[cl-demo] onMapStart: de_inferno` (boot-loaded plugin gets the first map); `RestartCount=0`.
  - **Client-list (the 2000870 regression fix):** `sm_clients` → 2 entries with real `name`/`userId`, `signon=5 or 6`, `steamid=0`, `fromUserId->slot` round-trips; `sm_who` (basecommands, UNTOUCHED code) lists the 2 bots — the consumer-level proof; `sm_slap <botname> 5` resolves the bot by name (`Player.target` works again).
  - **FakeConVar:** rcon `s2_demo_mode` → the engine prints the value + help; rcon `s2_demo_mode 77` then `sm_cvar s2_demo_mode` → `77` (the 6.7 read path sees the registered cvar); rcon `s2_demo_mode 200` → clamped to 100 (max) — verifies min/max reached the engine.
  - **OnMapStart:** rcon `changelevel de_dust2` → `[s2script] map start: de_dust2 (…)` + `[cl-demo] onMapStart: de_dust2`; then `sm_clients` on the new map → still lists the bots (client list + tracked signon survive changelevel); server ticking, no crash.
- [ ] **Document the human-client deferrals:** `kickWithReason` deliver-now over the TRACKED signon (`>= 4`) on a real client (the ban-reason flow; gates identical, mechanism preserved); a human's `name`/`ip` through `GetClientConVarValue`.
- [ ] Merge to main locally, push per the standing convention, update CLAUDE.md + memory.

## Self-Review notes (author)

- **Spec coverage:** client-list refactor (Task 1 — all 6 offsets retired, 5 op contracts preserved, signon tracked), FakeConVar (Task 2 — op + native + prelude + types + degrade test), OnMapStart (Task 3 — hook + FFI + mux + native + prelude + types + dispatch test), demo + typecheck (Task 4), live gate — all spec sections have a task.
- **No placeholders:** every code block is complete and grounded in read source (`iserver.h:218/:221`, `eiface.h:217/:247`, `icvar.h:127`, `convar.h:734` + `tier1/convar.cpp`'s pass-through proof, the CSSharp `mm_plugin.cpp` hook, the existing `dispatch_client_event`/`s2_chat_on_message`/`s2_concommand_register` bodies).
- **Type consistency:** `registerCvar` opts (`.d.ts`) ↔ prelude `tmap`/`defStr` handling ↔ the op's `type` int + string values; `onMapStart` handler `(mapName: string)` ↔ `dispatch_map_start`'s single String arg; `signonState` doc values ↔ the shim's tracked constants ↔ the preserved `>=2`/`>=4` gates.
- **Known judgment calls (flagged for review):** (a) `client_valid` switches from signon-based to `GetPlayerUserId != -1` — engine-authoritative and hook-independent, but the reviewer should confirm no JS consumer distinguishes the two during the connect window; (b) the string-default `const char*` punning is guarded by a `static_assert` and the engine-copies-at-registration argument; (c) bots' tracked signon depends on which lifecycle hooks fire for fake clients — nothing gates a bot on `>=4`, and the live gate records the observed value; (d) the identity block must end up AFTER `s_pEngine`'s declaration (Task 1 Step 1 note).

---

## Workflow Orchestration

**Execution order (strictly sequential — all of Tasks 1–3 modify `shim/src/s2script_mm.cpp`, and Tasks 2–3 both modify `core/src/v8host.rs` + `packages/server/index.d.ts`; parallelizing would conflict):**

1. **Task 1 — client-list refactor** (shim + gamedata; no core change). Must run FIRST: Task 3's hook body calls the rewritten `S2_GameServer()`, and the boot sanity line is shared.
2. **Task 2 — FakeConVar** (the slice's only ABI append; keeps the append deterministic before any later slice work).
3. **Task 3 — OnMapStart** (core mux + FFI + shim hook; depends on Task 1's `S2_GameServer`).
4. **Task 4 — demo + typecheck** (consumes all three; validates the new `.d.ts` surfaces under full strict).

**Per-task shape:** implement agent → adversarial-review agent → fix (the slice cadence). The final opus review runs after Task 4, before the sniper build + live gate.

**Adversarial-review priorities (where to spend reviewer depth):**
- **Task 1 (HIGHEST):** the shim C++ is never compiled locally — check: the `static_cast<INetworkServerService*>` on a `void*` + the `GetIGameServer()` header-vtable assumption; `s_pEngine` declaration ORDER vs the moved identity block; deletion completeness (`S2_ClientAt`/`s_off*`/pick/GamedataResult — grep must be clean); the signon set-before-dispatch / clear-after-dispatch ordering; contract preservation for all five ops (`-1`/null/0 defaults, bounds); no remaining reader of the deleted gamedata keys.
- **Task 2 (HIGH):** the 5-touchpoint ABI append (header typedef+field, Rust type+field, BOTH test structs, shim `ops.`) in exact order after `user_message_send`; the `ConVarCreation_t` fill vs `CConVar<T>::Register` (type enum per case, `SetMin/MaxValue` only when non-null, `m_Version` untouched); the `const char*`-into-`CUtlString` punning + `static_assert`; name/help/default string LIFETIMES (map-key anchor); idempotency + the invalid-ref-stays-mapped retry guard; the native's nullable-arg handling.
- **Task 3 (MEDIUM):** the `SH_DECL_HOOK3_void` signature vs `iserver.h:221` (const-ref params, POST=true at both add AND remove); forward decls in the header (META_NO_HL2SDK discipline); `dispatch_map_start` as a faithful `dispatch_client_event` mirror (snapshot-release, `try_borrow_mut`, `is_live`, TryCatch); the teardown wiring (shutdown reset + `remove_by_owner` — a missed one leaks handler Globals across reloads); the FFI null-map path.
- **Task 4 (LOW):** standard demo/typecheck review.

**What the Workflow cannot verify (deferred to the sniper build + live gate):** shim compilation, the live vtable correctness of `GetIGameServer`/`GetPlayerUserId`/`GetClientConVarValue`/`RegisterConVar`/`StartupServer` (all CSSharp-cross-validated on the same headers, and the boot `map start:` line surfaces a bad vtable immediately at first map), and the engine-side cvar visibility/clamping.
