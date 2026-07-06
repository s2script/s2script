# Sub-project 1 — `Client` object + lifecycle events — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Ship an engine-generic `@s2script/clients` module — a slot-backed `Client` handle + `onConnect`/`onActive`/`onDisconnect` lifecycle events.

**Architecture:** A `CLIENT_MUX` (reuse `event_mux::EventMux`) in core, a `__s2_client_subscribe` native, one name-keyed `dispatch_client_event(name, slot)` FFI export (notify-only, mirroring `dispatch_chat_message`), a `@s2script/clients` core prelude (the `Client` class over existing `__s2_client_*` ops + the `Clients` namespace with 6 `on*` methods), and six shim lifecycle hooks on `m_gameClients`. Additive — does not touch the 6.18 ban reject.

**Tech Stack:** Rust (core cdylib), C++ (Metamod shim + SourceHook), embedded JS (the core prelude), TypeScript (`.d.ts`).

## Global Constraints

- **Core stays engine-generic.** `Client`/`CLIENT_MUX`/the natives/the dispatch exports operate on `slot`/steamid/name — Source2-generic. No CS2 symbols in `core/`. `ISource2GameClients` + the lifecycle hooks are Source2 → shim-only. Both `check-core-boundary.sh` invocations stay green.
- **Additive only.** Do NOT modify the 6.18 `ClientConnect` reject hook or any ban behavior. This slice only ADDS the six notify hooks + the mux + the module.
- **Mirror existing patterns verbatim.** The notify-mux dispatch = `dispatch_chat_message` (`core/src/v8host.rs`, but call `handler(slot)` with NO return/suppress). The mux = `event_mux::EventMux` (`core/src/event_mux.rs`) used like `EVENT_MUX` / `CHAT_MSG_SUBS`. The subscribe native's owner/generation = `current_plugin(scope)` + the PLUGINS generation lookup (`v8host.rs:1026-1029`). The prelude sits beside `__s2pkg_events`/`__s2pkg_frame` (`v8host.rs:~592`). The shim hooks mirror `Hook_ClientCommand` (`s2script_mm.cpp:1664`) + its `SH_DECL_HOOK`/`SH_ADD_HOOK`/`SH_REMOVE_HOOK` (`:73,1158,1570`).
- **Degrade-never-crash.** A throwing handler is isolated by `TryCatch` + WARN (mirror `dispatch_chat_message`). Re-entrancy guarded by `try_borrow_mut`. Dispatch wrapped in `catch_unwind`.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn` (no backticks in `-m`; use `-F -`). ed25519 signing key already configured.

---

## Task 1: Core mux + natives + dispatch exports + `@s2script/clients` prelude + types + tests

**Files:**
- Modify: `core/src/v8host.rs` — `CLIENT_MUX` thread_local; `__s2_client_subscribe` native + registration; the `@s2script/clients` prelude; `remove_by_owner` in `unload_plugin`; reset in `shutdown`; the three `dispatch_client_*` internal fns; cargo tests.
- Modify: `core/src/ffi.rs` — the three `s2script_core_dispatch_client_*` exports.
- Create: `packages/clients/package.json` (mirror `packages/admin/package.json`).
- Create: `packages/clients/index.d.ts`.

**Interfaces produced (Task 3 consumes; the shim in Task 2 calls the FFI):**
- FFI: `s2script_core_dispatch_client_event(name: *const c_char, slot: c_int)` (`extern "C"`, `#[no_mangle]`) — the shim passes the event name literal.
- JS global `@s2script/clients` → `{ Client, Clients }` with the surface in `packages/clients/index.d.ts` (below).

### Core: the mux + subscribe native

- [ ] **Step 1 — `CLIENT_MUX` thread_local.** In the `thread_local!` block beside `EVENT_MUX` (`v8host.rs:~316`):
  ```rust
  static CLIENT_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
      = std::cell::RefCell::new(crate::event_mux::EventMux::new());
  ```
  Reset it in `shutdown` beside the `EVENT_MUX` reset: `CLIENT_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new());`. Call `remove_by_owner` in `unload_plugin` beside `EVENT_MUX.remove_by_owner` (grep for `EVENT_MUX.with` in `unload_plugin` and add the sibling `CLIENT_MUX.with(|m| { m.borrow_mut().remove_by_owner(&id); });`).

- [ ] **Step 2 — the `__s2_client_subscribe(event, handler)` native.** Mirror how `s2_subscribe` (`v8host.rs:~979`) gets the owner (`current_plugin(scope)`) and the generation (`PLUGINS...generation`, `v8host.rs:1026-1029`). The native:
  - reads `event` (arg 0, string) and `handler` (arg 1, function → `v8::Global::new`);
  - `let owner = current_plugin(scope).unwrap_or_else(|| "legacy".to_string());`
  - `let generation = PLUGINS.with(|p| p.borrow().get(&owner).map(|pi| pi.generation)).unwrap_or(0);`
  - `CLIENT_MUX.with(|m| { m.borrow_mut().subscribe(&event, owner, generation, global_handler); });` (ignore the "first" return — the shim hooks are installed unconditionally at Load; there is no engine-op to toggle).
  - Register it in the natives block beside `__s2_event_subscribe`: `set_native(scope, global_obj, "__s2_client_subscribe", s2_client_subscribe);`

- [ ] **Step 3 — the ONE dispatch internal fn.** Add `pub(crate) fn dispatch_client_event(event: &str, slot: i32)` — **copy `dispatch_chat_message` verbatim** (`v8host.rs`) but: snapshot `CLIENT_MUX` for the passed `event`; call `func.call(tc, recv, &[slot_val])` where `slot_val = v8::Integer::new(tc, slot).into()`; there is **no return value / no suppress** (drop the `suppress` bool and the numeric-return handling — a client handler's return is ignored). Keep the `try_borrow_mut` re-entrancy guard, the per-sub `is_live` + context clone + `HandleScope`/`ContextScope`/`TryCatch` + WARN-on-throw exactly as `dispatch_chat_message` has them. The event name comes from the shim; no per-event Rust wrappers are needed.
  ```rust
  pub(crate) fn dispatch_client_event(event: &str, slot: i32) {
      let snap = CLIENT_MUX.with(|m| m.borrow().snapshot(event));
      if snap.is_empty() { return; }
      HOST.with(|h| {
          let Ok(mut borrow) = h.try_borrow_mut() else { return };
          let Some(host) = borrow.as_mut() else { return };
          for (owner, generation, handler_g) in &snap {
              if !REGISTRY.with(|r| r.borrow().is_live(owner, *generation)) { continue; }
              let Some(g_ctx) = PLUGINS.with(|p| p.borrow().get(owner).map(|pi| pi.context.clone())) else { continue; };
              // ... HandleScope + ContextScope + TryCatch exactly as dispatch_chat_message ...
              let slot_val: v8::Local<v8::Value> = v8::Integer::new(tc, slot).into();
              let func = v8::Local::new(tc, handler_g);
              if func.call(tc, recv, &[slot_val]).is_none() {
                  let msg = tc.exception().map(|e| e.to_rust_string_lossy(&*tc)).unwrap_or_else(|| "handler threw".into());
                  log_warn(&format!("WARN: dispatch_client({}): handler '{}': {}", event, owner, msg));
              }
          }
      });
  }
  ```

### Core: the FFI exports

- [ ] **Step 4 — the ONE export in `ffi.rs`** (mirror `s2script_core_dispatch_game_event`'s name handling + `catch_unwind`; the name arrives as a C string, parse it with `CStr` like `dispatch_game_event`):
  ```rust
  #[no_mangle]
  pub extern "C" fn s2script_core_dispatch_client_event(name: *const c_char, slot: c_int) {
      let _ = std::panic::catch_unwind(|| {
          let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
          v8host::dispatch_client_event(&name, slot as i32);
      });
  }
  ```

### Core: the `@s2script/clients` prelude

- [ ] **Step 5 — the embedded JS prelude.** In the prelude area beside where `__s2pkg_events`/`__s2pkg_frame` are set (`v8host.rs:~592`), add the `@s2script/clients` module (verbatim):
  ```js
  function Client(slot) { this.slot = slot | 0; }
  Client.prototype.isValid = function () { return __s2_client_valid(this.slot); };
  Object.defineProperty(Client.prototype, "steamId",     { get: function () { return __s2_client_steamid(this.slot); } });
  Object.defineProperty(Client.prototype, "name",        { get: function () { var n = __s2_client_name(this.slot); return n == null ? "" : n; } });
  Object.defineProperty(Client.prototype, "userId",      { get: function () { return __s2_client_userid(this.slot); } });
  Object.defineProperty(Client.prototype, "signonState", { get: function () { return __s2_client_signon(this.slot); } });
  Object.defineProperty(Client.prototype, "isBot",       { get: function () { return __s2_client_steamid(this.slot) === "0"; } });
  Client.prototype.kick = function (reason)  { __s2_client_kick(this.slot, reason == null ? "" : String(reason)); };
  Client.prototype.chat = function (message) { __s2_client_print(this.slot, String(message)); };
  var __s2_MAX_CLIENTS = 64;
  function __s2_client_on(event, h) { __s2_client_subscribe(event, function (slot) { return h(new Client(slot)); }); }
  var __s2_clients = {
    onConnect:         function (h) { __s2_client_on("connect", h); },
    onPutInServer:     function (h) { __s2_client_on("putinserver", h); },
    onActive:          function (h) { __s2_client_on("active", h); },
    onFullyConnect:    function (h) { __s2_client_on("fullyconnect", h); },
    onDisconnect:      function (h) { __s2_client_on("disconnect", h); },
    onSettingsChanged: function (h) { __s2_client_on("settingschanged", h); },
    fromSlot: function (slot) { slot = slot | 0; return __s2_client_valid(slot) ? new Client(slot) : null; },
    all: function () { var out = []; for (var s = 0; s < __s2_MAX_CLIENTS; s++) { if (__s2_client_valid(s)) out.push(new Client(s)); } return out; }
  };
  globalThis.__s2pkg_clients = { Client: Client, Clients: __s2_clients };
  ```
  (Confirm the exact insertion syntax matches the surrounding prelude string — it is one big embedded JS literal; append inside it before/after the `__s2pkg_events` assignment.)

### Core: tests

- [ ] **Step 6 — cargo tests.** Mirror the existing `event_mux`/prelude tests:
  1. `CLIENT_MUX`/`EventMux` reuse: subscribe two owners to `"connect"`, `snapshot("connect")` has 2, `remove_by_owner` drops one; `"active"` is independent. (Can reuse the `event_mux` unit tests as the model — the mux is already tested; add a focused test only if `event_mux` coverage doesn't already assert this.)
  2. Prelude presence: in a context, `typeof globalThis.__s2pkg_clients === "object"`, `typeof __s2pkg_clients.Client === "function"`, `typeof __s2pkg_clients.Clients.onConnect === "function"`, `__s2pkg_clients.Clients.fromSlot(0)` is `null` (no engine → `__s2_client_valid` degrades false), `__s2pkg_clients.Clients.all()` is `[]`.
  3. A subscribed `onConnect` handler, when `dispatch_client_connect(3)` is called, receives a `Client` with `.slot === 3` (use an in-isolate eval that stashes the received slot into a global; then call `dispatch_client_connect(3)` and assert). Mirror how existing dispatch tests drive a handler.
  Run `cargo test` (core). Expect green (the ledger notes ~149 core tests pass).

- [ ] **Step 7 — boundary gate.** `bash scripts/check-core-boundary.sh` — expect green (no CS2 symbols; `client`/`slot` are engine-generic, same as the existing `client_*` natives).

### `@s2script/clients` types

- [ ] **Step 8 — `packages/clients/package.json`** (mirror `packages/admin/package.json`): name `@s2script/clients`, types `index.d.ts`.
- [ ] **Step 9 — `packages/clients/index.d.ts`:**
  ```ts
  /**
   * @s2script/clients — engine-generic client handle + lifecycle events.
   * Resolved at runtime via globalThis.__s2pkg_clients. Import: import { Client, Clients } from "@s2script/clients";
   */
  /** A connected client, identified by its 0-based slot (CPlayerSlot). Slot-backed; getters read live. */
  export declare class Client {
    readonly slot: number;
    /** True while a client occupies this slot. */
    isValid(): boolean;
    /** Decimal SteamID64; "0" for a bot or an unauthenticated client. */
    readonly steamId: string;
    /** Display name; "" if unavailable. */
    readonly name: string;
    /** Engine user-id; -1 if none. */
    readonly userId: number;
    /** Raw signon state; -1 if none. */
    readonly signonState: number;
    /** True for a fake client (bot) — derived from steamId === "0". */
    readonly isBot: boolean;
    /** Disconnect this client. */
    kick(reason?: string): void;
    /** Send a chat (SayText2) line to this client. */
    chat(message: string): void;
  }
  export declare const Clients: {
    /** Fires when a client connects (all clients incl. bots; carries name/xuid). May be async. */
    onConnect(handler: (client: Client) => void | Promise<void>): void;
    /** Fires when a client is put in the server (controller/pawn context now exists). May be async. */
    onPutInServer(handler: (client: Client) => void | Promise<void>): void;
    /** Fires when a client goes active (spawned / in-game). May be async. */
    onActive(handler: (client: Client) => void | Promise<void>): void;
    /** Fires when a client is fully connected. May be async. */
    onFullyConnect(handler: (client: Client) => void | Promise<void>): void;
    /** Fires when a client disconnects. Only `.slot` is guaranteed live here — capture identity earlier if needed. */
    onDisconnect(handler: (client: Client) => void): void;
    /** Fires when a client's settings (name/cvars) change. */
    onSettingsChanged(handler: (client: Client) => void): void;
    /** The client in `slot`, or null if the slot is empty. */
    fromSlot(slot: number): Client | null;
    /** Every currently-connected client. */
    all(): Client[];
  };
  ```

- [ ] **Step 10 — commit Task 1.**
  ```
  feat(clients): core Client mux + natives + @s2script/clients prelude + types

  CLIENT_MUX (event_mux reuse) + __s2_client_subscribe + one name-keyed
  dispatch_client_event FFI export (mirror dispatch_chat_message) + the
  @s2script/clients prelude (a slot-backed Client over existing client_* ops +
  onConnect/onPutInServer/onActive/onFullyConnect/onDisconnect/onSettingsChanged
  + fromSlot/all) + packages/clients types. Engine-generic; additive.
  ```

---

## Task 2: Shim lifecycle hooks + header decl + sniper build

**Files:**
- Modify: `shim/include/s2script_core.h` — declare the one export.
- Modify: `shim/src/s2script_mm.cpp` (+ its class header, where `Hook_ClientCommand` is declared) — the six hooks.

**Interfaces consumed:** the `s2script_core_dispatch_client_event(name, slot)` FFI export (Task 1).

- [ ] **Step 1 — declare the export** in `s2script_core.h` beside `s2script_core_dispatch_client_command`:
  ```c
  void s2script_core_dispatch_client_event(const char* name, int slot);
  ```
- [ ] **Step 2 — six `SH_DECL_HOOK`s** beside the `ClientConnect`/`ClientCommand` decls (`s2script_mm.cpp:73-78`). Signatures verbatim from `third_party/hl2sdk/public/eiface.h` (confirmed against CSSharp's live SourceHook param-info; `uint64` → `unsigned long long` under `META_NO_HL2SDK`):
  ```cpp
  SH_DECL_HOOK6_void(ISource2GameClients, OnClientConnected, SH_NOATTRIB, 0, CPlayerSlot, const char*, unsigned long long, const char*, const char*, bool);      // :567
  SH_DECL_HOOK4_void(ISource2GameClients, ClientPutInServer, SH_NOATTRIB, 0, CPlayerSlot, const char*, int, unsigned long long);                                  // :578
  SH_DECL_HOOK4_void(ISource2GameClients, ClientActive, SH_NOATTRIB, 0, CPlayerSlot, bool, const char*, unsigned long long);                                       // :582
  SH_DECL_HOOK1_void(ISource2GameClients, ClientFullyConnect, SH_NOATTRIB, 0, CPlayerSlot);                                                                        // :584
  SH_DECL_HOOK5_void(ISource2GameClients, ClientDisconnect, SH_NOATTRIB, 0, CPlayerSlot, ENetworkDisconnectionReason, const char*, unsigned long long, const char*);// :587
  SH_DECL_HOOK1_void(ISource2GameClients, ClientSettingsChanged, SH_NOATTRIB, 0, CPlayerSlot);                                                                     // :599
  ```
- [ ] **Step 3 — the six `Hook_*` members** (mirror `Hook_ClientCommand` at `s2script_mm.cpp:1664`; declare each in the class header beside `Hook_ClientCommand`). Each forwards `("<name>", slot.Get())` to the one dispatch and `RETURN_META(MRES_IGNORED)` (notify-only, never alters flow):
  ```cpp
  void S2ScriptPlugin::Hook_OnClientConnected(CPlayerSlot slot, const char* name, unsigned long long xuid, const char* netid, const char* addr, bool fake) {
      s2script_core_dispatch_client_event("connect", slot.Get()); RETURN_META(MRES_IGNORED);
  }
  void S2ScriptPlugin::Hook_ClientPutInServer(CPlayerSlot slot, const char* name, int type, unsigned long long xuid) {
      s2script_core_dispatch_client_event("putinserver", slot.Get()); RETURN_META(MRES_IGNORED);
  }
  void S2ScriptPlugin::Hook_ClientActive(CPlayerSlot slot, bool bLoadGame, const char* name, unsigned long long xuid) {
      s2script_core_dispatch_client_event("active", slot.Get()); RETURN_META(MRES_IGNORED);
  }
  void S2ScriptPlugin::Hook_ClientFullyConnect(CPlayerSlot slot) {
      s2script_core_dispatch_client_event("fullyconnect", slot.Get()); RETURN_META(MRES_IGNORED);
  }
  void S2ScriptPlugin::Hook_ClientDisconnect(CPlayerSlot slot, ENetworkDisconnectionReason reason, const char* name, unsigned long long xuid, const char* netid) {
      s2script_core_dispatch_client_event("disconnect", slot.Get()); RETURN_META(MRES_IGNORED);
  }
  void S2ScriptPlugin::Hook_ClientSettingsChanged(CPlayerSlot slot) {
      s2script_core_dispatch_client_event("settingschanged", slot.Get()); RETURN_META(MRES_IGNORED);
  }
  ```
- [ ] **Step 4 — six `SH_ADD_HOOK`s** beside the `ClientConnect` add (`s2script_mm.cpp:1163`), all `false` (post): `OnClientConnected`, `ClientPutInServer`, `ClientActive`, `ClientFullyConnect`, `ClientDisconnect`, `ClientSettingsChanged`, each `SH_MEMBER(this, &S2ScriptPlugin::Hook_<name>)`; then `META_CONPRINTF("[s2script] client lifecycle hooks installed (6 notify)\n");`. Six symmetric `SH_REMOVE_HOOK`s beside the `ClientConnect` remove (`s2script_mm.cpp:1577`).

- [ ] **Step 5 — sniper build.** `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`. Expect the shim + core to compile/link cleanly (additive; the `S2EngineOps` ABI struct is untouched). If a `SH_DECL_HOOK`N arity or a type mismatches, reconcile against `eiface.h`.

- [ ] **Step 6 — commit Task 2.**
  ```
  feat(clients): six shim lifecycle hooks (connect/putinserver/active/fullyconnect/disconnect/settingschanged)

  Six notify SourceHooks on m_gameClients → the Task-1 dispatch_client_event FFI.
  MRES_IGNORED (never alter flow). The 6.18 ClientConnect reject is untouched.
  ```

---

## Task 3: Live-gate demo + validation

**Files:**
- Create: `examples/clients-demo/{package.json,tsconfig.json,src/plugin.ts}` (mirror an existing `examples/*` plugin + `plugins/basecomm/tsconfig.json`).

- [ ] **Step 1 — the demo plugin.** Pure ESM. Subscribe to ALL SIX events (logs confirm firing + order). `import { Clients } from "@s2script/clients";`
  ```ts
  export function onLoad(): void {
    Clients.onConnect((c) => console.log(`[clients-demo] connect slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot}`));
    Clients.onPutInServer((c) => console.log(`[clients-demo] putInServer slot=${c.slot} name=${c.name}`));
    Clients.onActive((c) => console.log(`[clients-demo] active slot=${c.slot} name=${c.name}`));
    Clients.onFullyConnect((c) => console.log(`[clients-demo] fullyConnect slot=${c.slot} name=${c.name}`));
    Clients.onDisconnect((c) => console.log(`[clients-demo] disconnect slot=${c.slot} name=${c.name} steamId=${c.steamId}`));
    Clients.onSettingsChanged((c) => console.log(`[clients-demo] settingsChanged slot=${c.slot} name=${c.name}`));
    console.log(`[clients-demo] onLoad — all()=${Clients.all().length} clients`);
  }
  export function onUnload(): void { console.log("[clients-demo] onUnload"); }
  ```
- [ ] **Step 2 — typecheck + build.** `bash scripts/check-plugins-typecheck.sh` (auto-includes `examples/*`) → expect `examples/clients-demo` OK + PASS. `node packages/cli/dist/cli.js build examples/clients-demo` → expect the `.s2sp` path.
- [ ] **Step 3 — commit Task 3.**
  ```
  test(clients): clients-demo example — subscribe + log all three lifecycle events
  ```

---

## Post-tasks (controller — not a subagent task)

- Deploy: `package-addon.sh` (picks up Task 2's fresh sniper binaries) → recreate `dist/addons/s2script/configs` (chmod 777) + `admins.json` → copy the 7 base `.s2sp` + `_demo_clients-demo.s2sp` into `dist/addons/s2script/plugins/` → `docker compose -f docker/docker-compose.yml restart cs2`.
- Live gate (de_dust2, the user as a real client): `onConnect` fires with a `Client` whose `.steamId`/`.name`/`.userId` read correctly; `onActive` on spawn; `onDisconnect` on leave (note whether `.name`/`.steamId` are still populated mid-teardown); `Clients.all()` reflects the roster; a bot shows `isBot=true`/`steamId="0"`; `RestartCount=0`, 6.18 ban path still works.
- Final whole-branch review → merge to `main` → push.
