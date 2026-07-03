# Slice 5D.1 — The game-event system (`Events.on`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A generic engine-generic game-event system — `Events.on("player_death", ev => …)` — where the shim hooks `IGameEventManager2` and per-name JS subscriber lists receive a live `GameEvent` accessor; plus a reference-sourced `event-catalog.json` codegen'd into a typed `GameEvents` overlay in `@s2script/cs2` for IntelliSense.

**Architecture:** The shim holds the live `IGameEvent*` (never crosses to JS) and calls `s2script_core_dispatch_game_event(name)`; a core event multiplexer (notify-only; mirrors the `OnGameFrame` snapshot+ledger discipline) dispatches to subscribers; JS accessors read the event via new `S2EngineOps` ops. The typed layer is a types-only codegen over a committed catalog. Touches core + shim → one sniper rebuild.

**Tech Stack:** Rust `cdylib` core, the C++ Metamod shim (SourceHook + `IGameEventManager2`), the injected JS prelude, the TypeScript codegen (`packages/cli`), `node:test`, the Docker CS2 live gate.

## Global Constraints

Every task's requirements implicitly include these (spec §11):

- **Core stays engine-generic.** `IGameEvent`/`IGameEventManager2` + the event ops + `@s2script/events` are Source2-generic; event NAMES + keys are plugin-supplied strings. The CS2 event catalog + typed overlay live ONLY in `games/cs2/gamedata/event-catalog.json` + `packages/cs2` (never in `core/src` or `@s2script/events`). Both gates green: `bash scripts/check-core-boundary.sh`, `bash scripts/test-boundary-nameleak.sh`.
- **Never expose a raw pointer across time.** The `IGameEvent*` stays a file-scope pointer in the shim; JS accessors call back through the engine-ops; only copied values cross. The live accessor is block-scoped (synchronous-only, no `await`); post-dispatch the shim's current-event pointer is null → accessors degrade to defaults.
- **The ledger is the teardown authority.** Every event subscription is auto-ledgered; unload removes a plugin's handlers regardless of its own cleanup.
- **Re-entrancy-safe dispatch.** Snapshot the subscriber list before invoking handlers (mirror `OnGameFrame`).
- **Degrade per-descriptor.** Null manager / event op → events disabled with a logged reason; framework runs.
- **The `S2EngineOps` struct is a Rust↔C ABI contract.** Its Rust definition (`core/src/v8host.rs`) and C definition (`shim/include/s2script_core.h`) MUST stay in lockstep (same field order + types). T1 adds the Rust event ops; **T3 MUST add the identical C fields** — a mismatch is a silent ABI corruption at the live gate.
- **Deterministic codegen + freshness gate.** Same `event-catalog.json` → byte-identical `events.generated.d.ts`; types only.
- **cdylib:** core tests inline `#[cfg(test)] mod`. **Naming:** PascalCase types (`Events`, `GameEvent`), camelCase methods (`on`, `off`, `getInt`, `getPlayerSlot`).
- **Commit trailer:** every commit ends EXACTLY with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. Commit only on `slice-5d1-game-events`; do NOT push.

**Deferred — do NOT build:** an auto-dump of the catalog (VPK/KV3 extraction or runtime-RE enumeration); blocking/pre-hooks/`HookResult` for events; firing/creating events; typed `OnPlayerDeath` *event objects* + runtime per-event codegen; an eager snapshot + `GetDataKeys` enumeration; the engine-identity follow (5D.2); the ptr-codegen generalization (5D.3); the `tsc` gate; the registry (5.5); the base suite (6).

**Test runners:** core = `cargo test -p s2script-core -- --test-threads=1`; CLI = `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs` (scoped glob).

---

## Task 1: Core event mechanism — engine-ops + multiplexer + dispatch + natives (cargo-in-isolate)

**Files:**
- Create: `core/src/event_mux.rs`
- Modify: `core/src/v8host.rs` (`S2EngineOps` event fn-ptrs + fields; the subscribe/unsubscribe/accessor natives + install; a `MockOps`-driven test), `core/src/ffi.rs` (the dispatch export), `core/src/lib.rs` (`mod event_mux;`)

**Interfaces:**
- Produces (for T2/T3): the C-ABI export `s2script_core_dispatch_game_event(name: *const c_char)`; the `S2EngineOps` event fn-ptrs (exact signatures below — T3's C header must match); the natives `__s2_event_subscribe(name, handler)`, `__s2_event_unsubscribe(name, handler)`, `__s2_event_get_int(key)`, `_get_float`, `_get_bool`, `_get_string`, `_get_uint64`, `_get_player_slot`.

- [ ] **Step 1: Add the `S2EngineOps` event fn-ptr types + fields** (`v8host.rs`). Next to the existing op typedefs, add (C-ABI):

```rust
pub type EventSubscribeFn = extern "C" fn(name: *const c_char) -> c_int;
pub type EventUnsubscribeFn = extern "C" fn(name: *const c_char) -> c_int;
pub type EventGetIntFn = extern "C" fn(key: *const c_char) -> i32;
pub type EventGetFloatFn = extern "C" fn(key: *const c_char) -> f32;
pub type EventGetBoolFn = extern "C" fn(key: *const c_char) -> c_int;      // 0/1
pub type EventGetStringFn = extern "C" fn(key: *const c_char) -> *const c_char; // valid during dispatch; copy now
pub type EventGetUint64Fn = extern "C" fn(key: *const c_char) -> u64;
pub type EventGetPlayerSlotFn = extern "C" fn(key: *const c_char) -> i32;   // -1 if absent
```
Append to `struct S2EngineOps` (AFTER the existing fields — order is the ABI, do not reorder existing):

```rust
    pub event_subscribe: Option<EventSubscribeFn>,
    pub event_unsubscribe: Option<EventUnsubscribeFn>,
    pub event_get_int: Option<EventGetIntFn>,
    pub event_get_float: Option<EventGetFloatFn>,
    pub event_get_bool: Option<EventGetBoolFn>,
    pub event_get_string: Option<EventGetStringFn>,
    pub event_get_uint64: Option<EventGetUint64Fn>,
    pub event_get_player_slot: Option<EventGetPlayerSlotFn>,
```

- [ ] **Step 2: Create the event multiplexer** `core/src/event_mux.rs`. A notify-only per-name registry (NO priority ladder / HookResult — mirror `multiplexer.rs`'s *snapshot-before-invoke* + *remove_by_owner* discipline, not its collapse logic):

```rust
//! Notify-only game-event multiplexer: name → subscribers. Re-entrancy-safe (snapshot before invoke),
//! liveness-checked, and remove_by_owner for ledgered teardown. Mirrors multiplexer.rs's discipline
//! without the priority/HookResult machinery (events don't collapse).
use std::collections::HashMap;

pub struct EventSub<H> { pub owner: String, pub generation: u64, pub handler: H }

#[derive(Default)]
pub struct EventMux<H> { by_name: HashMap<String, Vec<EventSub<H>>> }

impl<H: Clone> EventMux<H> {
    pub fn new() -> Self { Self { by_name: HashMap::new() } }
    /// Returns true iff this is the FIRST subscriber for `name` (caller then calls the engine-op event_subscribe).
    pub fn subscribe(&mut self, name: &str, owner: String, generation: u64, handler: H) -> bool {
        let list = self.by_name.entry(name.to_string()).or_default();
        let first = list.is_empty();
        list.push(EventSub { owner, generation, handler });
        first
    }
    /// A snapshot of the handlers for `name` (empty if none) — the set that runs for this fire.
    pub fn snapshot(&self, name: &str) -> Vec<(String, u64, H)> {
        self.by_name.get(name).map(|v| v.iter().map(|s| (s.owner.clone(), s.generation, s.handler.clone())).collect()).unwrap_or_default()
    }
    /// Remove all of an owner's subscriptions (teardown). Returns the names that became empty
    /// (caller then calls the engine-op event_unsubscribe for each).
    pub fn remove_by_owner(&mut self, owner: &str) -> Vec<String> {
        let mut emptied = Vec::new();
        for (name, list) in self.by_name.iter_mut() {
            let before = list.len();
            list.retain(|s| s.owner != owner);
            if before > 0 && list.is_empty() { emptied.push(name.clone()); }
        }
        emptied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subscribe_first_then_snapshot_then_remove_by_owner() {
        let mut m: EventMux<&'static str> = EventMux::new();
        assert!(m.subscribe("player_death", "p".into(), 1, "h1"));   // first for the name
        assert!(!m.subscribe("player_death", "q".into(), 1, "h2"));  // not first
        assert_eq!(m.snapshot("player_death").len(), 2);
        assert_eq!(m.snapshot("round_start").len(), 0);
        let emptied = m.remove_by_owner("p");
        assert!(emptied.is_empty(), "still q for player_death");
        assert_eq!(m.snapshot("player_death").len(), 1);
        let emptied = m.remove_by_owner("q");
        assert_eq!(emptied, vec!["player_death".to_string()]);       // now empty → event_unsubscribe
    }
}
```
Add `mod event_mux;` to `core/src/lib.rs`.

- [ ] **Step 3: Run the pure mux test** — `cargo test -p s2script-core event_mux:: -- --test-threads=1` → PASS.

- [ ] **Step 4: Wire the isolate-side event state + natives** (`v8host.rs`). Add a thread-local `EventMux<v8::Global<v8::Function>>` (next to the frame multiplexer's storage). Implement:
  - `s2_event_subscribe(name, handler)`: capture the calling plugin's `(id, generation)` from the context slot (as the async resolvers do); `mux.subscribe(name, id, gen, handler_global)`; if it returned `true` (first for the name), call `ENGINE_OPS.event_subscribe(name)` (null-degrade). Ledger the subscription (mirror how interface imports/timers are auto-ledgered so teardown's `remove_by_owner` runs).
  - `s2_event_unsubscribe(name, handler)`: remove that handler; if the name emptied, call `event_unsubscribe(name)`.
  - The accessor natives `s2_event_get_int(key)`/`_float`/`_bool`/`_string`/`_uint64`/`_player_slot`: read `key` (a string), call the matching engine-op (null-degrade → the default: `0`/`0.0`/`false`/`""`/`"0"`/`-1`). `_get_uint64` → format the `u64` as a **decimal string** (`v8::String` of `format!("{}", v)`); `_get_string` → copy the returned `*const c_char` into a `v8::String` immediately (CStr, lossy). Register all in `install_natives`.
- [ ] **Step 5: The dispatch export** (`ffi.rs`, mirror `s2script_core_dispatch_game_frame`):

```rust
/// Shim → core: called by the shim's IGameEventListener2 when an event fires (the shim has already
/// stashed the live IGameEvent* for the accessor engine-ops). Dispatches to the name's JS subscribers.
#[no_mangle]
pub extern "C" fn s2script_core_dispatch_game_event(name: *const c_char) {
    if name.is_null() { return; }
    let Ok(name) = (unsafe { std::ffi::CStr::from_ptr(name) }).to_str() else { return };
    v8host::dispatch_game_event(name);   // snapshot subscribers, run each live handler with new GameEvent(name)
}
```
Add `v8host::dispatch_game_event(name)`: snapshot `mux.snapshot(name)`; for each `(owner, gen, handler)` that is still `REGISTRY.is_live(owner, gen)`, enter its context and call the handler with a fresh `new GameEvent(name)` — read the context's `GameEvent` constructor from `globalThis.__s2pkg_events.GameEvent` (added to the prelude in Step 5b below).

- [ ] **Step 5b: Add the `GameEvent` constructor to the prelude** (`INJECTED_STD_PRELUDE`, so the dispatch has it). `Events.on/off` land in Task 2; `GameEvent` lands HERE because the dispatch constructs it:

```js
  function GameEvent(name) { this.name = name; }
  GameEvent.prototype.getInt        = function (k) { return __s2_event_get_int(k); };
  GameEvent.prototype.getFloat      = function (k) { return __s2_event_get_float(k); };
  GameEvent.prototype.getBool       = function (k) { return __s2_event_get_bool(k); };
  GameEvent.prototype.getString     = function (k) { return __s2_event_get_string(k); };
  GameEvent.prototype.getUint64     = function (k) { return __s2_event_get_uint64(k); };   // decimal string
  GameEvent.prototype.getPlayerSlot = function (k) { return __s2_event_get_player_slot(k); };
```
and add `globalThis.__s2pkg_events = { GameEvent: GameEvent };` to the registration block (Task 2 extends it with `Events`).

- [ ] **Step 6: In-isolate mock test** (`v8host.rs` `#[cfg(test)] mod frame_tests`). Install a mock `S2EngineOps` whose `event_get_int`/`_string`/`_player_slot` return fixed values and whose `event_subscribe` records the name in a static; subscribe a handler via `__s2_event_subscribe` from a plugin context that stores what it reads into a global; call `dispatch_game_event("player_death")`; assert the handler ran and read the mocked `getInt`/`getString`/`getPlayerSlot`/`getUint64`(→string); assert an un-dispatched name doesn't call it; assert teardown (`remove_by_owner` via plugin unload) stops delivery. (Model this on the existing frame-dispatch + degrade tests; a `MockOps` builder likely already exists for the entity tests — reuse it, extending with the event fn-ptrs.)

- [ ] **Step 7: Run + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/event_mux.rs core/src/v8host.rs core/src/ffi.rs core/src/lib.rs
git commit -m "feat(slice5d1): core event mechanism — engine-ops + event_mux + dispatch_game_event + accessor natives

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 2: `@s2script/events` module — `Events` + `GameEvent` prelude + package (cargo-in-isolate)

**Files:**
- Modify: `core/src/v8host.rs` (`INJECTED_STD_PRELUDE`: `Events` + `GameEvent` + `__s2pkg_events`; an in-isolate test)
- Create: `packages/events/package.json`, `packages/events/index.d.ts`

**Interfaces:**
- Consumes: Task-1 natives (`__s2_event_subscribe`/`_unsubscribe`, `__s2_event_get_*`); the `dispatch_game_event` path (which constructs `new GameEvent(name)`).
- Produces (for T3/T4): `__s2require("@s2script/events")` → `{ Events, GameEvent }`; `Events.on(name, handler)` / `off`; the `GameEvent` accessor.

- [ ] **Step 1: Write the failing in-isolate test** (`frame_tests`): with the T1 mock ops, `__s2require("@s2script/events").Events.on("player_death", ev => { globalThis.__saw = ev.name + ":" + ev.getInt("attacker") })`; `dispatch_game_event("player_death")`; assert `read_global_string("__saw")` matches the mocked value; `Events.off` then dispatch → unchanged.

- [ ] **Step 2: Add `Events` to the prelude** (`INJECTED_STD_PRELUDE`). `GameEvent` already exists (Task 1 Step 5b); this task adds `Events` (delegating to the subscribe natives) and EXTENDS `__s2pkg_events`:

```js
  var Events = {
    on:  function (name, handler) { __s2_event_subscribe(name, handler); },
    off: function (name, handler) { __s2_event_unsubscribe(name, handler); },
  };
```
Change the existing `globalThis.__s2pkg_events = { GameEvent: GameEvent };` (from Task 1) to `globalThis.__s2pkg_events = { GameEvent: GameEvent, Events: Events };`. Confirm `Events.on`→`__s2_event_subscribe`→the mux→dispatch→the handler gets a `new GameEvent(name)` whose accessors resolve to the T1 natives.

- [ ] **Step 3: Run to verify pass** — `cargo test -p s2script-core frame_tests::<the new test> -- --test-threads=1` → PASS.

- [ ] **Step 4: The types-only package.** `packages/events/package.json` (mirror `packages/entity`): `{ "name": "@s2script/events", "version": "0.1.0", "types": "index.d.ts", "description": "Type stubs for the injected @s2script/events game-event API. No runtime code." }`. `packages/events/index.d.ts`:

```ts
/** @s2script/events — author-time stubs for the injected game-event API. NO runtime code. */

/** A live game-event accessor. Valid ONLY during the synchronous handler — read fields before any `await`;
 *  a stashed GameEvent used later reads defaults. The raw engine event never crosses to JS. */
export declare class GameEvent {
  readonly name: string;
  getInt(key: string): number;
  getFloat(key: string): number;
  getBool(key: string): boolean;
  getString(key: string): string;
  /** A 64-bit field as a decimal string (SM-parity, wire-safe). */
  getUint64(key: string): string;
  /** A player field (e.g. "userid"/"attacker") as a 0-based slot, or -1 if absent. Resolve with Player.fromSlot. */
  getPlayerSlot(key: string): number;
}

export declare const Events: {
  /** Subscribe to a game event by name. The handler runs synchronously when the event fires. */
  on(name: string, handler: (ev: GameEvent) => void): void;
  off(name: string, handler: (ev: GameEvent) => void): void;
};
```

- [ ] **Step 5: Run + gates + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add core/src/v8host.rs packages/events/package.json packages/events/index.d.ts
git commit -m "feat(slice5d1): @s2script/events module — Events.on/off + live GameEvent accessor in the prelude

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 3: The shim — `IGameEventManager2` + listener + engine-ops (compiles; validated at T5)

**Files:**
- Modify: `shim/include/s2script_core.h` (the C `S2EngineOps` event fields — MUST match T1's Rust order/types; the `s2script_core_dispatch_game_event` decl), `shim/src/s2script_mm.cpp` (acquire + listener + ops + dispatch + teardown), `shim/src/s2script_mm.h` (the listener member)

**Interfaces:**
- Consumes: T1's `s2script_core_dispatch_game_event(name)` + the `S2EngineOps` event fn-ptr contract.
- Produces: the populated event ops + the live event dispatch.

- [ ] **Step 1: Match the C `S2EngineOps`.** In `shim/include/s2script_core.h`, add the event fn-ptr typedefs + struct fields in the SAME order/types as T1's Rust (`s2_event_subscribe_fn(const char*)->int`, … `s2_event_get_string_fn(const char*)->const char*`, `s2_event_get_uint64_fn(const char*)->uint64`, `s2_event_get_player_slot_fn(const char*)->int`). Add `void s2script_core_dispatch_game_event(const char* name);`.

- [ ] **Step 2: Acquire `IGameEventManager2`.** In `Load()`, via the engine-factory `tryGet` path used for `SchemaSystem` (the community interface string is `GAMEEVENTSMANAGER002` — try it through the engine factory; degrade-never-crash to null). Store `s_pGameEventManager`. `#include <igameevents.h>`.

- [ ] **Step 3: The listener + current-event pointer.** Add a `class S2ScriptEventListener : public IGameEventListener2` whose `FireGameEvent(IGameEvent* ev)` does:

```cpp
void S2ScriptEventListener::FireGameEvent(IGameEvent* ev) {
    if (!ev) return;
    IGameEvent* prev = s_currentEvent;      // save (re-entrancy)
    s_currentEvent = ev;
    s2script_core_dispatch_game_event(ev->GetName());
    s_currentEvent = prev;                  // restore
}
```
File-scope `static IGameEvent* s_currentEvent = nullptr;` + a `static S2ScriptEventListener s_eventListener;` + a tracked `std::set<std::string> s_subscribedNames;`.

- [ ] **Step 4: Implement the event ops** (file-scope C functions, wired into the `S2EngineOps ops` table in Step 5). Each accessor reads `s_currentEvent` (default if null); keys wrap `CKV3MemberName(key)`:

```cpp
static int  s2_event_subscribe(const char* name) {
    if (!s_pGameEventManager || !name) return -1;
    if (s_subscribedNames.insert(name).second)          // first time for this name
        s_pGameEventManager->AddListener(&s_eventListener, name, /*bServerSide*/true);
    return 0;
}
static int  s2_event_unsubscribe(const char* name) { if (name) s_subscribedNames.erase(name); return 0; } // listener stays; RemoveListener is all-names
static int   s2_event_get_int(const char* k)        { return (s_currentEvent && k) ? s_currentEvent->GetInt(CKV3MemberName(k), 0) : 0; }
static float s2_event_get_float(const char* k)      { return (s_currentEvent && k) ? s_currentEvent->GetFloat(CKV3MemberName(k), 0.0f) : 0.0f; }
static int   s2_event_get_bool(const char* k)       { return (s_currentEvent && k) ? (s_currentEvent->GetBool(CKV3MemberName(k), false) ? 1 : 0) : 0; }
static const char* s2_event_get_string(const char* k){ return (s_currentEvent && k) ? s_currentEvent->GetString(CKV3MemberName(k), "") : ""; }
static uint64 s2_event_get_uint64(const char* k)    { return (s_currentEvent && k) ? s_currentEvent->GetUint64(CKV3MemberName(k), 0) : 0; }
static int   s2_event_get_player_slot(const char* k){ return (s_currentEvent && k) ? s_currentEvent->GetPlayerSlot(CKV3MemberName(k)).Get() : -1; }
```
(If `CKV3MemberName(const char*)` isn't directly constructible, adapt per the SDK — Step 1 of the live gate is where this is confirmed; the spike log in Step 6 helps.)

- [ ] **Step 5: Populate the ops table + teardown.** In the `S2EngineOps ops = {}` assembly, set the 8 event fields to the functions above. In `Unload()`, `if (s_pGameEventManager) s_pGameEventManager->RemoveListener(&s_eventListener);` before core shutdown.

- [ ] **Step 6: A shim-side diagnostic** (removed later or left behind a debug flag): in `FireGameEvent`, before dispatch, `Msg("[s2script] event fired: %s\n", ev->GetName());` so the live gate can confirm the listener fires *independently* of the core dispatch. Build must compile: it's validated live in T5 (no unit test — SourceHook/engine).

- [ ] **Step 7: Commit** (compiles; not yet live-verified):

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp shim/src/s2script_mm.h
git commit -m "feat(slice5d1): shim — acquire IGameEventManager2 + IGameEventListener2 + event engine-ops + dispatch

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 4: The event catalog + typed codegen (node:test)

**Files:**
- Create: `games/cs2/gamedata/event-catalog.json`, `packages/cli/src/eventgen/{model.ts,emit-dts.ts,gen.ts}`, `packages/cli/test/eventgen.test.mjs`, `packages/cs2/events.generated.d.ts` (generated), `scripts/check-events-generated.sh`
- Modify: `packages/cli/src/cli.ts` (a `gen-events` command), `packages/cs2/index.d.ts` (expose the typed `Events`), `games/cs2/js/pawn.js` (the one-line `Events` runtime re-export into `__s2pkg_cs2`)

**Interfaces:**
- Consumes: the `@s2script/events` `GameEvent` type (T2).
- Produces: `packages/cs2/events.generated.d.ts` (the `GameEvents` map + per-event interfaces + the typed `on<K>` overload).

- [ ] **Step 1: The catalog** `games/cs2/gamedata/event-catalog.json` — reference-sourced, the documented CS2 events → `{ field: type }`, `type ∈ "bool"|"int"|"float"|"string"|"uint64"|"player"`. Seed with the well-known, confidently-sourced events (accuracy over breadth — OMIT anything uncertain; the generic string API covers the tail). At minimum:

```json
{
  "player_death": { "userid": "player", "attacker": "player", "assister": "player", "weapon": "string", "headshot": "bool", "penetrated": "int", "noscope": "bool", "thrusmoke": "bool", "attackerblind": "bool" },
  "player_hurt": { "userid": "player", "attacker": "player", "health": "int", "armor": "int", "weapon": "string", "dmg_health": "int", "dmg_armor": "int", "hitgroup": "int" },
  "player_spawn": { "userid": "player" },
  "player_team": { "userid": "player", "team": "int", "oldteam": "int", "disconnect": "bool" },
  "player_connect_full": { "userid": "player" },
  "player_disconnect": { "userid": "player", "reason": "int", "name": "string", "networkid": "string" },
  "round_start": { "timelimit": "int", "fraglimit": "int", "objective": "string" },
  "round_end": { "winner": "int", "reason": "int", "message": "string" },
  "round_freeze_end": {},
  "bomb_planted": { "userid": "player", "site": "int" },
  "bomb_defused": { "userid": "player", "site": "int" },
  "weapon_fire": { "userid": "player", "weapon": "string", "silenced": "bool" }
}
```
(Extend with more confidently-sourced events; the live gate validates whichever fire.)

- [ ] **Step 2: Write the failing codegen tests** (`packages/cli/test/eventgen.test.mjs`):

```js
import { test } from "node:test";
import assert from "node:assert";
import { buildEventModel } from "../src/eventgen/model.ts";
import { emitEventDts } from "../src/eventgen/emit-dts.ts";

const CAT = { player_death: { userid: "player", attacker: "player", weapon: "string", headshot: "bool", penetrated: "int" } };

test("buildEventModel groups fields by accessor + interface name", () => {
  const m = buildEventModel(CAT);
  const e = m.find(x => x.event === "player_death");
  assert.equal(e.iface, "PlayerDeathEvent");
  assert.deepEqual(e.byGetter.getPlayerSlot.sort(), ["attacker", "userid"]);
  assert.deepEqual(e.byGetter.getString, ["weapon"]);
  assert.deepEqual(e.byGetter.getBool, ["headshot"]);
  assert.deepEqual(e.byGetter.getInt, ["penetrated"]);
});

test("emitEventDts emits typed per-event interfaces + the GameEvents map + the typed overload", () => {
  const dts = emitEventDts(buildEventModel(CAT));
  assert.match(dts, /export interface PlayerDeathEvent extends GameEvent \{/);
  assert.match(dts, /getPlayerSlot\(key: "attacker" \| "userid"\): number;/);
  assert.match(dts, /getString\(key: "weapon"\): string;/);
  assert.match(dts, /export interface GameEvents \{[^}]*player_death: PlayerDeathEvent;/s);
  assert.match(dts, /export function on<K extends keyof GameEvents>\(name: K, handler: \(ev: GameEvents\[K\]\) => void\): void;/);
});
```

- [ ] **Step 3: Implement `model.ts`** (pure): `buildEventModel(catalog)` → per event `{ event, iface: PascalCase(event)+"Event", byGetter: { getInt:[], getFloat:[], getBool:[], getString:[], getUint64:[], getPlayerSlot:[] } }`. Map `type → getter`: `bool→getBool`, `int→getInt`, `float→getFloat`, `string→getString`, `uint64→getUint64`, `player→getPlayerSlot`. Sort field lists + events deterministically (alphabetical). `PascalCase("player_death")` = `PlayerDeathEvent`.

- [ ] **Step 4: Implement `emit-dts.ts`** (pure): the header `import type { GameEvent } from "@s2script/events";`; per event, `export interface <Iface> extends GameEvent { <getter>(key: <"a" | "b">): <retType>; … }` (only emit a getter line if that getter has fields; `retType`: number for int/float/playerSlot, boolean for bool, string for string/uint64); the `export interface GameEvents { <event>: <Iface>; … }` map; and the typed overload `export function on<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => void): void;` + a fallback `export function on(name: string, handler: (ev: GameEvent) => void): void;`.

- [ ] **Step 5: `gen.ts` + the CLI command.** `gen.ts` exports `runGenEvents({ check })` (mirror `schemagen/gen.ts`): read `event-catalog.json` → `emitEventDts(buildEventModel(...))` → write `packages/cs2/events.generated.d.ts`; `--check` regenerates to a temp + diffs. Wire `gen-events` into `packages/cli/src/cli.ts` (mirror `gen-schema`). Update the usage string.

- [ ] **Step 6: Run the codegen tests** — `cd packages/cli && node --experimental-strip-types --no-warnings --test test/eventgen.test.mjs` → PASS.

- [ ] **Step 7: Generate + expose the typed `Events` + freshness gate.**

```bash
cd /home/gkh/projects/s2script/packages/cli && node build.mjs
cd /home/gkh/projects/s2script && node packages/cli/dist/cli.js gen-events
```
**Wire the re-export (runtime + types) so `import { Events } from "@s2script/cs2"` works:**
  - **Runtime** (`games/cs2/js/pawn.js`): add `Events: __s2require("@s2script/events").Events` to the `globalThis.__s2pkg_cs2 = { … }` object (so the cs2 runtime re-exports the injected `Events`). This is the ONE runtime line — everything else is types.
  - **Types** (`packages/cs2/index.d.ts`): `export { GameEvent } from "@s2script/events";` + a typed `Events` assembled from the generated overload — import the generated `on` overloads + `GameEvents` map from `./events.generated` and declare `export declare const Events: { on<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => void): void; on(name: string, handler: (ev: GameEvent) => void): void; off(name: string, handler: (ev: GameEvent) => void): void; };`. (Re-export approach — explicit and reliable; TS module-augmentation of `@s2script/events` was considered and rejected for cross-package fragility.)
  - **The eventgen node:test (Step 2) is the type-shape verification** (no `tsc` gate yet): it asserts the generated interfaces + key-unions + the `GameEvents` map + the `on<K>` overload are structurally correct. A plugin using `Events.on("player_death", ev => ev.getPlayerSlot("attacker"))` typechecks against this — full `tsc` verification awaits the deferred tsc gate.
Create `scripts/check-events-generated.sh` (mirror `check-schema-generated.sh`: regenerate + `git diff --exit-code` on `packages/cs2/events.generated.d.ts`). Run it → PASS. (Adding the `Events` re-export to `pawn.js` must NOT disturb `check-schema-generated.sh` — `pawn.js` is hand-written, not generated.)

- [ ] **Step 8: Full CLI suite + gates + commit**

```bash
cd /home/gkh/projects/s2script/packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-events-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add games/cs2/gamedata/event-catalog.json packages/cli/src/eventgen packages/cli/src/cli.ts \
        packages/cli/test/eventgen.test.mjs packages/cs2/events.generated.d.ts packages/cs2/index.d.ts \
        games/cs2/js/pawn.js scripts/check-events-generated.sh
git commit -m "feat(slice5d1): reference-sourced event-catalog.json + event codegen -> typed GameEvents overlay in @s2script/cs2

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Task 5: Sniper build + live gate + docs (LIVE-ONLY, controller-driven)

**Files:**
- Modify: `examples/demo-plugin/src/plugin.ts`, `README.md`, `CLAUDE.md`; **create** a dated spike-findings doc.

**Needs ONE sniper rebuild** (core natives + prelude + shim).

- [ ] **Step 1: Sniper build + package.** `docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh` (GLIBC ≤ 2.31). Confirm both `libs2script_core.so` + `s2script.so` rebuilt (the shim changed).

- [ ] **Step 2: The demo** (`examples/demo-plugin/src/plugin.ts`) — subscribe to `player_death` (+ `player_spawn`/`player_connect_full` as early-firing fallbacks):

```ts
import { Events } from "@s2script/cs2";   // typed overlay
import { Player } from "@s2script/cs2";

export function onLoad(): void {
  console.log("[demo] onLoad (game events)");
  Events.on("player_spawn", (ev) => {
    const p = Player.fromSlot(ev.getPlayerSlot("userid"));
    console.log("[demo] player_spawn slot=" + ev.getPlayerSlot("userid") + " name=" + (p ? p.playerName : "?"));
  });
  Events.on("player_death", (ev) => {
    const victim = Player.fromSlot(ev.getPlayerSlot("userid"));
    const attacker = Player.fromSlot(ev.getPlayerSlot("attacker"));
    console.log("[demo] player_death victim=" + (victim ? victim.playerName : "?")
      + " attacker=" + (attacker ? attacker.playerName : "?")
      + " weapon=" + ev.getString("weapon") + " headshot=" + ev.getBool("headshot"));
  });
}
export function onUnload(): void { console.log("[demo] onUnload"); }
```
Build the `.s2sp`, deploy (`package-addon.sh` to refresh the addon; copy the `.s2sp` into `dist/addons/s2script/plugins/`), restart, wait past the boot window, `bot_quota 2`, let a round run (bots fight → `player_death`; `mp_warmup_end`/`mp_freezetime 0` to speed it). Read `docker logs s2script-cs2 | grep '\[s2script\]'` for the shim `event fired:` diagnostic AND the `[demo]` lines.
**Verdict:** events fire → reach JS → fields marshal (a real weapon string, a resolved attacker/victim name via `getPlayerSlot`+`Player.fromSlot`), server ticking. **Catalog validation:** for each event that fired, confirm the logged fields match `event-catalog.json`; correct the catalog + regenerate if a field is absent/renamed (then re-run `check-events-generated.sh`). If the shim diagnostic never logs, the listener/acquire is wrong — HALT and diagnose (the interface string `GAMEEVENTSMANAGER002`, the factory, the `CKV3MemberName` key). Record findings in `docs/superpowers/specs/2026-07-02-slice-5d1-spike-findings.md`.

- [ ] **Step 3: Degrade/teardown.** Reload/unload the plugin (or `bot_kick`) → `onUnload`, subscriptions removed (a later event doesn't reach the unloaded handler), server ticking, no crash. Capture the log. If the live infra won't cooperate after reasonable attempts, get the non-live deliverables done and report BLOCKED with commands/errors.

- [ ] **Step 4: README + CLAUDE.**
  - `README.md`: a `## Game events (Slice 5D.1)` section — the `Events.on` bus, the shim `IGameEventManager2` hook, the live `GameEvent` accessor (synchronous-only, raw event never crosses), `Player.fromSlot(ev.getPlayerSlot(...))` resolution, the typed catalog overlay (IntelliSense via `@s2script/cs2`; reference-sourced), and the captured live log. Note blocking/pre-hooks, firing events, and the auto-dump catalog are deferred.
  - `CLAUDE.md` "## Current state": Slice 5D.1 done (game events — shim hooks `IGameEventManager2`, core notify-only `event_mux` + `dispatch_game_event` + accessor natives, `@s2script/events` `Events.on/off` + live `GameEvent`, reference-sourced `event-catalog.json` → typed `GameEvents` overlay in cs2; player resolution via `getPlayerSlot`+`Player.fromSlot`; live-validated). "Current focus" → 5D.2 engine-identity, then 5D.3 ptr-codegen generalization. Do NOT alter the standing conventions.

- [ ] **Step 5: Final verification + commit**

```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs
cd /home/gkh/projects/s2script && bash scripts/check-events-generated.sh && bash scripts/check-schema-generated.sh && bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh
git add examples/demo-plugin/src/plugin.ts README.md CLAUDE.md docs/superpowers/specs/2026-07-02-slice-5d1-spike-findings.md games/cs2/gamedata/event-catalog.json packages/cs2/events.generated.d.ts
git commit -m "feat(slice5d1): live gate PASSED — game events fire to JS + typed catalog; spike + README + CLAUDE

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn"
```

---

## Acceptance (spec §9)

1. `cargo test -p s2script-core` green (event_mux + the mock-dispatch + accessor + teardown in-isolate); the CLI `node:test` suite green (eventgen); both boundary gates + `check-schema-generated.sh` + `check-events-generated.sh` green; sniper build clean.
2. `Events.on(name, handler)` delivers live events; the `GameEvent` accessor reads fields; `uint64`→string; teardown removes a plugin's subscriptions.
3. The typed catalog overlay gives IntelliSense (`Events.on("player_death", ev => ev.getPlayerSlot("attacker"))` typechecks against `events.generated.d.ts`); freshness-gated.
4. Live gate: an event fires → reaches a JS handler → fields marshal + `Player.fromSlot(ev.getPlayerSlot(...))` resolves; catalog validated; teardown stops delivery; server ticking, no crash.
5. README + CLAUDE updated.
