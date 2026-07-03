# Slice 5D.3 — event actionability (pre-hooks: block + modify, and firing) — design

**Goal:** Bring the game-event system to parity with the `OnGameFrame` multiplexer — the **write**
direction. Add pre-hooks that can **block** (suppress client broadcast) or **modify** a game event, and
the ability to **fire** events — over the same `IGameEventManager2::FireEvent` choke point, reusing the
sig-scanned manager (5D.2) + SourceHook + the existing `HookResult` collapse machinery.

**Status:** design approved. `Events.on` (notify/post) is unchanged; this is purely additive.

**Branch base:** `main` (… + 5D.1 events + 5D.2 live events/identity merged).
**Cadence:** subagent-driven, merge-to-main-locally, live Docker CS2 gate.

---

## 1. Framing — the choke point + what's reused

Every game event flows through `IGameEventManager2::FireEvent(IGameEvent* event, bool bDontBroadcast)`.
SourceMod implements pre-hooks + blocking by hooking exactly this; we do the same. Reused, not rebuilt:

- **The manager pointer** — `s_pGameEventManager`, resolved by 5D.2's sig-scan (already live-proven;
  `AddListener` works, a strong signal the SDK `IGameEventManager2` vtable matches CS2).
- **SourceHook** — the shim already `SH_ADD_HOOK`s `ISource2Server::GameFrame`; the same mechanism +
  `SH_CALL` + `MRES_SUPERCEDE`/`RETURN_META_VALUE` (all present in the vendored `sourcehook.h`) hook
  `FireEvent`.
- **`HookResult { Continue, Changed, Handled, Stop }` + `run_chain` collapse** — `core/src/multiplexer.rs`
  (used by `OnGameFrame`). The event mux is the only multiplexer without collapse; this slice adds a
  collapsing **pre** variant alongside the existing notify path.
- **The lazy hook-install path** — `s2_request_hook(descriptor, enable)` (shim) + the core→shim
  `request_hook` callback (today only `"OnGameFrame"`); this slice adds a `"GameEvent"` descriptor that
  installs/removes the `FireEvent` hook when the first/last pre-subscriber comes/goes.
- **The `GameEvent` accessor + `s_currentEvent`** — 5D.1's block-scoped live event; gains setters.

`Events.on(name, handler)` (POST, via the 5D.1 `AddListener` listener) is UNTOUCHED.

---

## 2. The three capabilities

### 2.1 Block — `Events.onPre(name, handler)`

A **pre** subscription. `handler(ev)` returns a `HookResult` (a new engine-generic export from
`@s2script/events`); `undefined` → `Continue`. Semantics (collapsed by `run_chain`, priority-ordered,
`Monitor` observers never affect the result — identical to `OnGameFrame`):

- `Continue` / `Changed` → allow (any `set*` modifications already applied to the live event stand).
- `Handled` → **suppress client broadcast** (the event still fires server-side; post-subscribers still
  see it). Other pre-hooks still run.
- `Stop` → suppress broadcast **and** short-circuit lower-priority pre-hooks.

**No JS handler has ever returned a `HookResult` before** (`OnGameFrame.subscribe` handlers return
`void`); this slice introduces the convention. `void`/`undefined` return = `Continue`, so existing
mental models hold.

### 2.2 Modify — setters on the live event (pre-hook only)

During a pre-hook the `GameEvent` accessor exposes `ev.setInt(key,v)` / `setString` / `setBool` /
`setFloat` / `setUint64(key, decimalString)` — writing the live `IGameEvent` (the SDK `Set*` methods)
before the original `FireEvent` runs. Block-scoped exactly like the getters: valid only synchronously
inside the pre-hook; a stashed `ev` used post-`await` (or a POST/notify handler) no-ops the setter
(the manager/current-event is null → safe miss). Getters keep working in both pre and post.

### 2.3 Fire — `Events.fire(name, fields, dontBroadcast?)`

`CreateEvent(name)` → `Set*` per field → `FireEvent(ev, dontBroadcast=false)`. **Runtime type dispatch
from the JS value** (the author-time types come from the CS2 typed overlay; the runtime infers):
`boolean`→`SetBool`, `string`→`SetString`, `Number.isInteger(v)`→`SetInt`, other `number`→`SetFloat`,
`bigint` or a decimal-`string`-flagged-as-uint64 → `SetUint64`. Returns `boolean` (the `FireEvent`
result) or `false` if the manager is null / the event name is unknown (degrade, no crash). A fired
event flows through our own `FireEvent` hook, so it is itself pre-hookable + notifiable (SM parity).

**The write target — one unified pointer (`s_currentEvent`).** The `set*` ops always write
`s_currentEvent`, so the SAME five setters serve both "modify during a pre-hook" and "build a
to-be-fired event". `event_create(name)` **saves** the previous `s_currentEvent` and points it at the
freshly created event; `event_fire(dontBroadcast)` fires that event and **restores** the saved
`s_currentEvent`. This makes `Events.fire` inside a pre-hook nest correctly (the outer hooked event is
saved/restored) — the same save/restore discipline as 5D.1's `FireGameEvent`. `Events.fire` runs
`create → set… → fire` synchronously in JS (no `await` between), so the temporary retarget is
race-free.

---

## 3. Components & data flow

### 3.1 Shim (engine-generic C++)

- `SH_DECL_HOOK2(IGameEventManager2, FireEvent, SH_NOATTRIB, 0, bool, IGameEvent*, bool)` +
  `Hook_FireEventPre` installed on `s_pGameEventManager` via `s2_request_hook("GameEvent", 1)` (lazy).
- **`Hook_FireEventPre(IGameEvent* ev, bool bDontBroadcast)`**:
  ```
  set s_currentEvent = ev  (mutable during the pre-dispatch; restore after — re-entrancy, like 5D.1)
  int decision = s2script_core_dispatch_game_event_pre(ev->GetName())   // 0 = allow, 1 = suppress broadcast
  restore s_currentEvent
  if (decision == 1) {
      bool ret = SH_CALL(s_pGameEventManager, &IGameEventManager2::FireEvent)(ev, /*bDontBroadcast=*/true);
      RETURN_META_VALUE(MRES_SUPERCEDE, ret);   // we called the original ourselves with broadcast off
  }
  RETURN_META_VALUE(MRES_IGNORED, true);          // original runs normally; mods already on ev
  ```
- **Event write/fire ops** (implement + wire into `S2EngineOps`, ABI-appended after the 5D.2 client
  ops): `event_set_int/float/bool/string/uint64` (write `s_currentEvent`; no-op if null),
  `event_create(name)` (→ `CreateEvent`, save+retarget `s_currentEvent`; degrade `false`/no-op if the
  manager is null or the name is unknown), `event_fire(dontBroadcast)` (→ `FireEvent` the created event,
  restore the saved `s_currentEvent`, return the result). All degrade-never-crash on a null manager /
  null current event.

### 3.2 Core (engine-generic Rust)

- A **pre-multiplexer**: `event_mux.rs` gains a HookResult-collapsing pre path (reuses `run_chain`),
  keyed by name, with priority + `remove_by_owner` teardown — mirroring both the existing notify mux
  and `multiplexer.rs`. `s2script_core_dispatch_game_event_pre(name) -> c_int` snapshots the pre-subs,
  runs them via `run_chain`, maps the collapsed `HookResult` to `0` (allow) / `1` (suppress:
  `Handled`|`Stop`).
- **Natives** (register alongside the 5D.1 event natives): `__s2_event_subscribe_pre(name, handler)` /
  `__s2_event_unsubscribe_pre`, `__s2_event_set_int/float/bool/string/uint64`, `__s2_event_create(name)`
  / `__s2_event_fire(dontBroadcast)`. The pre-subscribe native requests the `"GameEvent"` hook on the
  first sub (per the OnGameFrame lazy pattern) and auto-ledgers the subscription.
- **New `S2EngineOps`** (ABI-appended, C header + Rust mirror, same order): `event_set_int`,
  `event_set_float`, `event_set_bool`, `event_set_string`, `event_set_uint64`, `event_create`,
  `event_fire`. **New C ABI:** `s2script_core_dispatch_game_event_pre(const char*) -> int`.

### 3.3 JS API (engine-generic `@s2script/events` + CS2 typed overlay)

- `@s2script/events` (engine-generic): `Events.onPre(name, handler)`, the `GameEvent` setters, and a
  new `HookResult` const export (`{ Continue, Changed, Handled, Stop }`).
- `@s2script/cs2` typed overlay (`events.generated.d.ts` + `pawn.js` re-export): `Events.onPre<K>`
  (key-typed like `Events.on<K>`), `Events.fire<K>(name, fields)` (fields typed to that event's fields),
  the setter key-types, and `HookResult`. The runtime stays the generic bus; uncatalogued events use
  the string API.

### 3.4 Data flow (a hooked event)

```
engine fires E ─► IGameEventManager2::FireEvent(E, bDontBroadcast)
                    └─(SourceHook Pre)─► Hook_FireEventPre
                         s_currentEvent=E; dispatch_game_event_pre("E")
                              └─► pre-subs run (ev.getX / ev.setX; return HookResult) ─► collapse
                         decision==suppress? SH_CALL FireEvent(E, true) + SUPERCEDE : IGNORED
                    └─► original FireEvent(E, bDontBroadcast') ─► our AddListener listener.FireGameEvent
                              └─► dispatch_game_event("E") ─► POST/notify subs (Events.on)
```

---

## 4. Boundary (the core rule)

| Concern | Lives in | Why |
|---|---|---|
| The `FireEvent` SourceHook, `SH_CALL`/`SUPERCEDE`, set/create/fire ops | shim (engine-generic) | `IGameEventManager2`/`IGameEvent` are Source2 ENGINE types |
| The pre-multiplexer + collapse + the `_pre` dispatch + natives | `core/src` (engine-generic) | events + `HookResult` are engine-generic |
| `Events.onPre`/`fire`/`HookResult` runtime | `@s2script/events` | events are engine-generic |
| The typed `Events.onPre<K>`/`fire<K>` overlay | `@s2script/cs2` | the event catalog is CS2 gamedata |

`check-core-boundary.sh` + `test-boundary-nameleak.sh` stay green — no CS2 identifier enters `core/src`.

---

## 5. Degrade-never-crash

- Null manager → the `FireEvent` hook is never installed; `onPre` subscriptions register but never fire;
  `fire`/`set*` no-op (return `false`/undefined). Identical safety to today's degrade.
- `set*` outside a pre-hook (null `s_currentEvent`) → no-op. `create`/`fire` on an unknown event name →
  `false`. A pre-hook JS error → that sub is dropped from the collapse (the `run_chain` `Err` path),
  the rest run, the event is not wrongly suppressed.
- **Re-entrancy:** `Events.fire` inside a pre-hook re-enters `FireEvent` → the hook runs again on the
  new event; `s_currentEvent` is saved/restored per dispatch (5D.1 discipline). A pre-hook that fires
  the SAME event it's handling is the author's own infinite loop (as in SourceMod) — not guarded.

## 6. The one real risk (live-gate validated)

The hook assumes the vendored SDK's `IGameEventManager2` vtable order matches CS2 for `FireEvent`'s
index. **Mitigation:** `AddListener` (a lower vtable index on the same interface) already works live
(5D.2), so the layout matches through at least that point; the live gate confirms `FireEvent`
specifically (a `Handled` pre-hook demonstrably suppresses + a `set*` demonstrably changes a field). If
the index is wrong the symptom is a wrong-vfunc call → we validate before trusting it. No gamedata
signature is needed (unlike the manager pointer) — the vtable index is compiler-derived from the SDK
header, part of the pinned-hl2sdk treadmill.

---

## 7. Testing & live gate

- **In-isolate (cargo):** the pre-multiplexer collapse (Continue/Changed/Handled/Stop + Monitor +
  priority → 0/1 suppress decision, via `run_chain`); the set/create/fire natives degrade to
  no-op/`false` with no ops; `dispatch_game_event_pre` with no subs → 0 (allow).
- **In-isolate (node/vm):** `Events.onPre` registers; `HookResult` export; `Events.fire` shape.
- **One sniper rebuild** (shim hook + the new natives).
- **Live gate (de_inferno, bot_quota 2):** (a) `onPre("player_hurt")` returning `Handled` → the hurt
  event is suppressed to clients (server still processes — a post `Events.on("player_hurt")` still
  logs it); (b) a pre-hook `ev.setInt` on a modifiable field → the change is observed downstream; (c)
  `Events.fire("<a known event>", {...})` → a post-subscriber receives it; (d) `bot_kick` → clean
  degrade, server ticking, no crash.

## 8. Rough task decomposition (~5; the plan finalizes)

1. **Core pre-multiplexer + `dispatch_game_event_pre`** (reuse `run_chain`; `event_mux` pre path) +
   in-isolate collapse tests.
2. **Set/create/fire ops + natives** (C header + Rust mirror ABI-append; the `HookResult` const; the
   pre-subscribe/set/create/fire natives) + in-isolate degrade tests.
3. **Shim: the `FireEvent` SourceHook** (`SH_DECL_HOOK2`, `Hook_FireEventPre` with `SH_CALL`/`SUPERCEDE`,
   the `"GameEvent"` request-hook branch) + the set/create/fire op impls wired into `S2EngineOps`.
4. **JS API + typed overlay** (`@s2script/events` `Events.onPre`/`fire`/`HookResult` + setters; the
   CS2 `Events.onPre<K>`/`fire<K>` typed overlay) + vm tests.
5. **Demo + one sniper build + live gate + README/CLAUDE.**

## 9. Explicitly out of scope (do not build ahead)

The newer `IGameEventSystem` protobuf path; `OnTakeDamage` / entity-IO / non-event hooks; firing to a
specific recipient filter; per-listener ordering guarantees beyond priority; `FireEventClientSide`;
event *descriptor* editing; the `tsc` typecheck gate; config/permissions/reload-state; the
registry/platform (5.5); the base-plugin suite (6). Note later needs as TODOs and stop.
