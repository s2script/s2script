# Slice 5D.1 — The game-event system (`Events.on`)

**Status:** design approved, ready for writing-plans.
**Branch:** `slice-5d1-game-events` (off `main`: Slices 0–5A + entref-wire + 5B + 5C.1 + 5C.2 + 5B.4 + 5C.3 + 5C.4 merged).
**Family:** 5D — engine-service subsystems (events now; the engine-identity follow next; then the ptr-codegen
generalization). First of the "continue on all 3" arc; the base-plugin suite (Slice 6) is blocked on events.

---

## 1. Goal

A generic, engine-generic **game-event system**: `Events.on("player_death", (ev) => …)`. The shim hooks the
engine's `IGameEventManager2`, and each fired event is dispatched to per-name JS subscriber lists; the handler
gets a **live typed accessor** (`ev.getInt`/`getString`/`getPlayerSlot`/…) valid during the synchronous handler.
This is the SourceMod `HookEvent`/`GetEventInt` model, and it's the critical-path unblock for the base-plugin
suite.

## 2. What we build on (merged / grounded)

- **The shim already has the two patterns this needs** (`shim/src/s2script_mm.cpp`): factory-acquire of engine
  interfaces via `ismm->GetEngineFactory(false)` + a `tryGet` helper (it acquires `SchemaSystem`, `EngineCvar`,
  `NetworkServerService`, `GameResourceService`); and **SourceHook** (`SH_DECL_HOOK` + `SH_ADD_HOOK`) — how
  `OnGameFrame` hooks `ISource2Server::GameFrame`. The `S2EngineOps` table is the established extension point.
- **The SDK** (`third_party/hl2sdk/public/igameevents.h`): `IGameEvent` has `GetName()` + typed by-key accessors
  (`GetBool`/`GetInt`/`GetUint64`/`GetFloat`/`GetString`) + **`GetPlayerSlot(key) → CPlayerSlot`** (resolves a
  `userid` field to a slot). `IGameEventManager2` has `AddListener(IGameEventListener2*, name, bServerSide)` +
  `RemoveListener` + the `IGameEventListener2::FireGameEvent(IGameEvent*)` callback. `GameEventKeySymbol_t` is
  `CKV3MemberName` (const-char*-constructible — the shim wraps the JS key string).
- **The `OnGameFrame` multiplexer** (`core/src/multiplexer.rs`) is the model: a re-entrancy-safe snapshot,
  per-owner subscriber tracking, ledgered teardown. The event multiplexer mirrors it (keyed by event name).
- **`Player.fromSlot`** (5C.2): a handler resolves a player via `Player.fromSlot(ev.getPlayerSlot("attacker"))`
  — so **events do NOT depend on the engine-identity follow** (the event resolves the slot itself).

## 3. Decisions locked during brainstorming

1. **Generic string-keyed bus + a live accessor.** `Events.on(name, handler)`; the handler gets a `GameEvent`
   accessor (`ev.name`, `ev.getInt/getFloat/getBool/getString(key)`, `ev.getUint64(key)`→string,
   `ev.getPlayerSlot(key)`→number). One engine-generic API (`@s2script/events`) for all events; the event names
   are just strings. NO typed named events (`OnPlayerDeath`) + NO per-event codegen (deferred).
2. **Live accessor, synchronous-only.** `ev` reads the **live** `IGameEvent` held by the shim during the
   synchronous handler; the raw `IGameEvent*` **never crosses to JS**. A stashed `ev` used post-`await` reads
   defaults (the shim's current-event pointer is null outside dispatch). Same block-scoped, no-`await` discipline
   as entity raw-live views. NO field enumeration / eager snapshot (deferred).
3. **Notify-only.** Deliver post-fire to all subscribers; NO blocking / pre-hooks / `HookResult` for events
   (deferred). Multiple plugins may subscribe to the same event.
4. **`uint64` → decimal string** (the 5B.4 lesson: SM-parity + wire-safe).
5. **Engine-generic module.** `IGameEvent`/`IGameEventManager2` are Source2-generic (not CS2-specific), so
   `@s2script/events` lives in the core prelude (`__s2pkg_events`) alongside entity/frame/…; the event *names*
   (`player_death`) are game facts the plugin supplies as strings.

## 4. Architecture — the shim (C++, `s2script_mm.cpp`)

- **Acquire** `IGameEventManager2*` in `Load()` via the engine factory (the `tryGet`/factory path used for
  `SchemaSystem`); store `s_pGameEventManager` (degrade-never-crash: null on failure → all event ops no-op).
- **One `IGameEventListener2` subclass** `S2ScriptEventListener` whose `FireGameEvent(IGameEvent* ev)` saves the
  previous current-event pointer, sets a file-scope `s_currentEvent = ev`, calls
  `s2script_core_dispatch_game_event(ev->GetName())`, then **restores** the previous pointer (re-entrancy-safe;
  nested fires are not expected this slice since firing is deferred, but the save/restore is cheap insurance).
- **`S2EngineOps` gains** (all null-degrade): `event_subscribe(name)` → `s_pGameEventManager->AddListener(&s_listener,
  name, /*bServerSide*/true)` (idempotent — track the set of subscribed names to avoid double-add);
  `event_unsubscribe(name)` (drop from the tracked set; the listener stays registered — `RemoveListener` is
  all-names, so precise per-name removal is not attempted this slice); and the accessor ops
  `event_get_int(key)→i32`, `event_get_float(key)→f32`, `event_get_bool(key)→i32` (0/1),
  `event_get_string(key)→const char*` (valid during dispatch; core copies immediately), `event_get_uint64(key)→u64`,
  `event_get_player_slot(key)→i32` (`GetPlayerSlot(key).Get()`, −1 if absent). Each reads `s_currentEvent`
  (returns the default if it's null). Keys are wrapped `CKV3MemberName(key)`.
- **Teardown:** `RemoveListener(&s_listener)` on `Unload`.

## 5. Architecture — the core (engine-generic, Rust)

- **The event multiplexer** (`core/src/event_mux.rs` or fold into `multiplexer.rs`): `Map<String, Vec<Subscriber>>`
  where a `Subscriber` is `(plugin_id, generation, v8::Global<Function>)`. Mirrors the `OnGameFrame` multiplexer's
  re-entrancy-safe snapshot + liveness check.
- **`s2script_core_dispatch_game_event(name)`** (C-ABI, shim→core): snapshot the `name`'s subscriber list; for
  each live subscriber, enter its context and call the handler with a fresh `GameEvent` accessor (`new GameEvent(name)`);
  skip dead plugins (`REGISTRY.is_live`).
- **The accessor natives** (JS-facing, read the current-dispatch event via the engine-ops):
  `__s2_event_get_int(key)`→number, `_get_float`→number, `_get_bool`→bool, `_get_string`→string (copied),
  `_get_uint64`→**decimal string** (format the u64), `_get_player_slot`→number. Each calls the matching engine-op;
  with no ops / no current event → the default (`0`/`0.0`/`false`/`""`/`"0"`/`-1`).
- **Subscribe/teardown:** `Events.on(name, handler)` adds to the multiplexer + (if the name's list was empty)
  calls the engine-op `event_subscribe(name)`; **auto-ledgers** the subscription. `Events.off(name, handler)`
  removes (+ `event_unsubscribe` if the list is now empty). On plugin unload, the ledger walks the plugin's event
  subscriptions and removes its handlers (teardown authority — doesn't depend on the plugin's own cleanup).

## 6. Architecture — the `@s2script/events` module

- Types-only package `packages/events/{package.json,index.d.ts}`. The runtime lives in `INJECTED_STD_PRELUDE`
  (`__s2pkg_events = { Events }`) — a `GameEvent` constructor (`function GameEvent(name){ this.name = name; }` +
  the accessor methods calling the natives) and an `Events` object (`on(name, handler)`, `off(name, handler)`
  delegating to `__s2_event_subscribe`/`__s2_event_unsubscribe` natives that manage the multiplexer).
- **Types:** `Events.on(name: string, handler: (ev: GameEvent) => void): void` / `off(...)`; `GameEvent` with
  `readonly name: string`, `getInt(key): number`, `getFloat(key): number`, `getBool(key): boolean`,
  `getString(key): string`, `getUint64(key): string`, `getPlayerSlot(key): number`.

## 7. Data flow

engine fires `player_death` → `IGameEventManager2` calls `S2ScriptEventListener::FireGameEvent(ev)` → shim sets
`s_currentEvent = ev`, calls `s2script_core_dispatch_game_event("player_death")` → core snapshots the
`"player_death"` subscribers → for each, `new GameEvent("player_death")` → the JS handler runs →
`ev.getPlayerSlot("attacker")` → `__s2_event_get_player_slot("attacker")` native → engine-op
`event_get_player_slot` → shim `s_currentEvent->GetPlayerSlot("attacker").Get()` → the slot → the plugin does
`Player.fromSlot(slot)`. Dispatch returns → shim restores `s_currentEvent`. A stashed `ev` afterward → the
engine-op sees `s_currentEvent == null` → returns defaults.

## 8. Safety

- **The raw `IGameEvent*` never crosses to JS.** It stays a file-scope pointer in the shim; JS accessors call
  back through the engine-ops, which read it. Only copied primitives/strings return.
- **The live accessor is a block-scoped view.** Valid ONLY during the synchronous dispatch; a handler must read
  fields synchronously (no `await` before reading). Post-dispatch, the shim's current-event pointer is null →
  accessors degrade to defaults (never a use-after-free).
- **Degrade-never-crash:** null manager / null event / absent key → the field's default; a broken event op
  disables event delivery with a logged reason, the framework keeps running.
- **Teardown is ledger-driven:** a plugin's subscriptions are removed on unload regardless of its own cleanup.

## 9. Testing & acceptance

- **In-isolate (core, `#[cfg(test)]`):** with a MOCK `S2EngineOps` whose `event_get_*` return fixed values and
  whose `event_subscribe` records the name, drive `s2script_core_dispatch_game_event("player_death")` and assert:
  a subscribed handler runs and reads the mocked fields (`getInt`/`getString`/`getPlayerSlot`); an unsubscribed
  handler doesn't; `uint64`→a decimal string; accessors outside dispatch (no current event) → defaults; teardown
  removes a plugin's subscriptions (a later dispatch doesn't call it).
- **`@s2script/events` (in-isolate / vm):** `Events.on`/`off` register/unregister; the `GameEvent` accessor
  methods call the right natives; degrade to defaults without ops.
- **Live gate (sniper-rebuilt):** a plugin subscribes to `player_death` (+ a reliably-early event like
  `player_spawn`/`player_connect_full` as a fallback) on Docker CS2 (`bot_quota 2`; bots fight → `player_death`
  fires). The handler logs `ev.getPlayerSlot("attacker")`/`"userid"` resolved through `Player.fromSlot` (the
  attacker/victim names), `ev.getString("weapon")`, `ev.getBool("headshot")`. Prove: the event fires, fields
  marshal correctly, player resolution works, server ticking; unsubscribe/`onUnload` stops delivery, no crash.

**Acceptance:** `cargo test -p s2script-core` green (multiplexer + accessor + teardown in-isolate); the CLI
`node:test` suite green; both boundary gates + `check-schema-generated.sh` green; the sniper build clean; the
live gate passes; README + CLAUDE updated.

## 10. Scope & deferrals

**Scope:** the shim `IGameEventManager2` acquire + `IGameEventListener2` + the event `S2EngineOps` ops; the core
event multiplexer + `s2script_core_dispatch_game_event` + the accessor natives; the `@s2script/events` module
(`Events.on/off` + the `GameEvent` live accessor) + types; ledgered teardown; the live gate.

**Deferred — do NOT build:** **blocking / pre-hooks / `HookResult`** for events (notify-only now); **firing /
creating** events (`Events.fire`, `CreateEvent`/`FireEvent`/`Set*`); **typed named events** (`OnPlayerDeath`) +
per-event schema codegen; an **eager stashable snapshot** + `GetDataKeys` full-field enumeration; the
engine-identity follow (5D.2 — `userId`/`fromUserId`/pawnless-enum); the ptr-codegen generalization (5D.3); the
`tsc` gate; the registry (5.5); the base suite (6).

## 11. Global constraints (bind every task)

- **Core stays engine-generic.** `IGameEvent`/`IGameEventManager2` + the event ops + `@s2script/events` are
  Source2-generic; event NAMES + keys are plugin-supplied strings. NO CS2 identifiers in `core/src`. Both
  boundary gates green.
- **Never expose a raw pointer across time.** The `IGameEvent*` stays in the shim; the JS accessor is a
  block-scoped live view (synchronous-only, no `await`); only copied values cross. Degrade to defaults
  post-dispatch.
- **The ledger is the teardown authority.** Every event subscription is auto-ledgered; unload removes a plugin's
  handlers regardless of its own cleanup.
- **Re-entrancy-safe dispatch.** Snapshot the subscriber list before invoking handlers (mirrors `OnGameFrame`);
  a handler that subscribes/unsubscribes doesn't corrupt the in-flight dispatch.
- **Degrade per-descriptor.** Null manager / event op → events disabled with a logged reason; the framework runs.
- **cdylib:** core tests inline `#[cfg(test)] mod`. The shim change needs a sniper rebuild.
- **Naming:** PascalCase types (`Events`, `GameEvent`), camelCase methods (`on`, `off`, `getInt`, `getPlayerSlot`).
- **Commit trailer** on every commit; commit only on `slice-5d1-game-events`; do NOT push.
