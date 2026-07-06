# Sub-project 1 — the `Client` object + client lifecycle events (`@s2script/clients`)

**Goal:** Introduce a first-class **engine-generic `Client`** handle (slot-backed) and the **client lifecycle events** (`onConnect` / `onActive` / `onDisconnect`) as a new `@s2script/clients` module. This is the foundation for the ban-reason feature (sub-project 3) and is independently valuable — any plugin that wants join/leave notifications plus a typed client handle instead of bare `slot` numbers.

**Non-goals (explicitly deferred):**
- **Console-print, `kickWithReason`, and the `ip`/`client_address` op → sub-project 2.** This slice adds no new engine primitives beyond the lifecycle hooks + mux; `Client` wraps only ops that already ship.
- **The ban-plugin refactor and flipping the shim `ClientConnect` to always-admit → sub-project 3.** This slice does **not** touch the 6.18 ban reject — it is purely additive.
- **Deduping CS2's `Player` onto `Client` (Player-extends-Client) → a separate future refactor.** `Player` stays exactly as shipped.

---

## Background — why `slot` stays the id

A "client" is the connection occupying a `CPlayerSlot` (0-based index into the server's client array); 5D.2 confirmed `clientElems[slot]` index == the player slot. So **`slot` is the identifier (a number, = `CPlayerSlot`); `Client` is the object for the participant at it.** The project already uses `slot` as the universal id (`Player.fromSlot`, `Chat.toSlot`, `ctx.callerSlot`) and `client_*` as the op/native prefix (`client_kick(slot)`). `Client` hangs those slot-keyed ops on one object; `slot` remains the id everywhere. No rename.

---

## Architecture

Three layers, all mirroring proven patterns:

1. **Shim (Source2-specific):** three new `SH_ADD_HOOK`s on the already-held `m_gameClients` — `OnClientConnected`, `ClientActive`, `ClientDisconnect` — each forwarding to a new core FFI export (`s2script_core_dispatch_client_connect/active/disconnect`). Mirrors the existing `ClientCommand`/`ClientConnect` hooks. **The 6.18 `ClientConnect` reject hook is untouched.**
2. **Core (engine-generic):** a `CLIENT_MUX` (reuse `event_mux::EventMux<v8::Global<v8::Function>>`, keyed by `"connect"`/`"active"`/`"disconnect"`) + a `__s2_client_subscribe(event, handler)` native + the three `dispatch_client_*` exports (notify-only, mirroring `dispatch_game_event`: snapshot-release-borrow, `try_borrow_mut` re-entrancy guard, `REGISTRY.is_live` liveness, per-sub `TryCatch`) + `remove_by_owner` teardown on unload + reset on shutdown.
3. **Core prelude (`@s2script/clients` runtime, embedded JS in `v8host.rs`):** the `Client` class (slot-backed, wrapping the existing `__s2_client_*` natives) + `Clients.onConnect/onActive/onDisconnect` (thin wrappers over `__s2_client_subscribe` that construct a `Client` from the dispatched primitives) + `Clients.fromSlot(slot)` + `Clients.all()`. `globalThis.__s2pkg_clients = { Client, Clients }`.

### Data flow

```
player connects → engine → OnClientConnected(slot,name,xuid,netid,addr,fake)  [shim hook]
  → s2script_core_dispatch_client_connect(slot)                               [ffi — slot only]
  → CLIENT_MUX snapshot "connect" → each JS wrapper: handler(new Client(slot)) [core, notify-only]
  → the plugin's onConnect handler runs (may be async — Promise handled by the microtask drain)

player spawns/goes active → ClientActive(slot,...) → dispatch_client_active(slot) → onActive handlers
player leaves → ClientDisconnect(slot,...) → dispatch_client_disconnect(slot) → onDisconnect handlers
```

The dispatch exports carry **only the slot** — `Client` reads name/steamId/etc. live through the shipped ops. (These are recompile-together shim→core exports, not ABI-ordered `S2EngineOps`, so a later slice can widen a signature freely with one sniper build if a cached-at-event payload is ever needed — YAGNI for v1.)

The lifecycle events are **notify-only** (no `HookResult` collapse, no reject in this slice) — exactly `dispatch_game_event`'s shape, not the pre-hook multiplexer's.

---

## Components

### `Client` class (engine-generic; core prelude in `v8host.rs`)

Slot-backed; every accessor reads **live** through an existing native (no cached engine state crosses time). Constructed as `new Client(slot)`.

| Member | Backing native (already ships) | Returns |
|---|---|---|
| `slot` | (the constructor arg) | `number` |
| `isValid()` | `__s2_client_valid(slot)` | `boolean` |
| `steamId` (getter) | `__s2_client_steamid(slot)` | `string` (decimal SteamID64; `"0"` = bot/unauth) |
| `name` (getter) | `__s2_client_name(slot)` | `string` (`""` if unavailable) |
| `userId` (getter) | `__s2_client_userid(slot)` | `number` (`-1` if none) |
| `signonState` (getter) | `__s2_client_signon(slot)` | `number` (`-1` if none) |
| `isBot` (getter) | derived: `steamId === "0"` | `boolean` |
| `kick(reason?)` | `__s2_client_kick(slot, reason)` | `void` |
| `chat(message)` | `__s2_client_print(slot, message)` (SayText2) | `void` |

**No new natives.** `Client` is a pure JS wrapper over the shipped `client_*` ops. Two `Client`s with the same slot are `.slot`-equal; identity is the slot, so no handle/serial machinery is needed (a client's slot is stable for the life of its connection; a reused slot is a new connection, surfaced by a fresh `onConnect`).

**Live-op caveat (documented):** during `onDisconnect` the client is mid-teardown, so live getters other than `slot` are best-effort (may read `""`/`"0"`/`-1`). Handlers that need identity at disconnect should capture it at `onConnect`/`onActive`, keyed by slot. `slot` itself is always valid in the disconnect handler.

### `Clients` namespace (engine-generic; same prelude)

```ts
const Clients = {
  onConnect(handler: (c: Client) => void | Promise<void>): void,     // __s2_client_subscribe("connect", wrap)
  onActive(handler: (c: Client) => void | Promise<void>): void,      // __s2_client_subscribe("active", wrap)
  onDisconnect(handler: (c: Client) => void): void,                  // __s2_client_subscribe("disconnect", wrap)
  fromSlot(slot: number): Client | null,                             // valid slot → new Client(slot), else null
  all(): Client[],                                                   // slots 0..MAX where __s2_client_valid → Client[]
};
```

Each `on*` wrapper registers a function that the dispatch invokes with the raw slot; the wrapper does `handler(new Client(slot))`. `MAX` for `all()` = 64 (the Source2 slot-array cap; the same bound `pawn.js` uses).

### Core mux + natives + dispatch (`v8host.rs` + `ffi.rs`) — engine-generic

- `CLIENT_MUX: thread_local RefCell<EventMux<v8::Global<v8::Function>>>` beside `EVENT_MUX` (reset in `shutdown`; `remove_by_owner` in `unload_plugin`).
- `__s2_client_subscribe(event: string, handler: function)` native → `CLIENT_MUX.subscribe(event, owner, generation, handler)` (owner = the loading plugin id, generation from the registry — mirror `__s2_event_subscribe`). No engine-op on first-subscribe (the shim hooks are installed unconditionally at Load, unlike the lazy game-event manager).
- `s2script_core_dispatch_client_connect(slot)` / `_active(slot)` / `_disconnect(slot)` FFI exports (mirror `s2script_core_dispatch_game_event`): `catch_unwind`; snapshot the mux for the event name (release the borrow); `try_borrow_mut` HOST re-entrancy guard; per-sub `REGISTRY.is_live(owner, generation)` + `TryCatch`; invoke `handler(slot)` (the JS wrapper builds the `Client`). Each carries only the slot; the JS wrapper reads name/steamId live.

### Shim (`s2script_mm.cpp` + `.h`) — Source2-specific

- `SH_DECL_HOOK6_void(ISource2GameClients, OnClientConnected, SH_NOATTRIB, 0, CPlayerSlot, const char*, uint64, const char*, const char*, bool)` (matches `eiface.h:568`).
- `SH_DECL_HOOK4_void(ISource2GameClients, ClientActive, SH_NOATTRIB, 0, CPlayerSlot, bool, const char*, uint64)` (matches `eiface.h:583`).
- `SH_DECL_HOOK5_void(ISource2GameClients, ClientDisconnect, SH_NOATTRIB, 0, CPlayerSlot, ENetworkDisconnectionReason, const char*, uint64, const char*)` (matches `eiface.h:588`).
- `Hook_OnClientConnected` / `Hook_ClientActive` / `Hook_ClientDisconnect` members → call the matching `s2script_core_dispatch_client_*`; `RETURN_META(MRES_IGNORED)` (notify-only, never alters flow). Confirm the exact signatures against `eiface.h:560-590`; use `unsigned long long` for `uint64` under `META_NO_HL2SDK` (the 6.18 header gotcha).
- Three `SH_ADD_HOOK`s beside the `ClientCommand`/`ClientConnect` adds (`s2script_mm.cpp:1158-1164`); three symmetric `SH_REMOVE_HOOK`s beside their removes (`:1568-1578`). Declare `s2script_core_dispatch_client_*` in `s2script_core.h`.

### `@s2script/clients` types (`packages/clients/{package.json,index.d.ts}`) — engine-generic

`Client` (class with the members above) + `Clients` (the namespace). Mirrors `packages/admin` / `packages/events` structure. `MAX_PLAYERS` etc. are internal — not exported.

---

## Testing

**In-isolate (cargo, hermetic):**
- `EventMux` reuse: subscribe to `"connect"`, `dispatch_client_connect` invokes the handler with the right slot; `remove_by_owner` drops it; a second event name is independent (mirror the existing `event_mux` tests).
- The `@s2script/clients` prelude exists and `Clients.fromSlot`/`all`/`onConnect` are defined; a subscribed handler receives a `Client` whose `.slot` matches; `Client.isBot` reflects `steamId === "0"`.
- Re-entrancy: a handler that triggers another dispatch is guarded (`try_borrow_mut` graceful-skip), mirroring the game-event test.

**Boundary:** `@s2script/clients`, `CLIENT_MUX`, the natives, and the dispatch exports are all engine-generic (slot/steamid/name are Source2-generic). `ISource2GameClients` + the shim hooks are Source2 — shim-only. Both `check-core-boundary.sh` invocations stay green.

**Live gate (de_dust2, the user as a real client):** a tiny demo plugin (or a temporary log in an existing one) subscribes to all three events and logs. Verify: `onConnect` fires on join with a `Client` whose `.steamId`/`.name`/`.userId` read correctly; `onActive` fires once spawned; `onDisconnect` fires on leave with the right `.slot` (the demo also logs the disconnect `Client`'s `.name`/`.steamId` to confirm whether live-ops are still populated mid-teardown — informs whether a cached-payload follow-up is warranted); `Clients.all()` lists the connected clients; bots surface with `isBot === true` / `steamId === "0"`; `RestartCount=0`. (No ban behavior changes in this slice — the 6.18 reject still works exactly as before.)

---

## Risks / decisions

- **Additive, low-risk:** no existing behavior changes; the 6.18 ban path is untouched. The only new engine surface is three notify hooks + a mux (a well-worn pattern).
- **`OnClientConnected` vs `ClientConnect` as the connect source:** use `OnClientConnected` — it fires post-accept for **all** clients incl. bots (has the `bFakePlayer` flag + address), whereas `ClientConnect` is the reject gate and skips bots. The connect **event** wants the inclusive callback.
- **Async connect handlers:** notify-only dispatch fire-and-forgets a returned Promise (the microtask drain resolves it), same as other handlers — no awaiting inside the engine callback.
- **`Client` identity = slot:** no serial/handle needed. A client's slot is stable for its connection; a reused slot is a distinct connection announced by a fresh `onConnect`. (Contrast `EntityRef`, which needs a serial because entity pointers/indices recycle within a life.)
- **Disconnect live-op caveat** (documented above): only `.slot` is guaranteed in `onDisconnect`.

## Build order (for the plan)

- **Task 1 — core mux + natives + dispatch exports + the `@s2script/clients` prelude + `packages/clients` types + cargo tests + boundary gates.** (No shim yet — testable in-isolate.)
- **Task 2 — the shim lifecycle hooks + header decls + sniper build.** Wires the engine callbacks to Task 1's dispatch exports.
- **Task 3 — a minimal live-gate demo** (subscribe + log) and the live validation.
- Then: merge + push.
