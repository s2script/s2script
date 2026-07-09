# Slice: entity I/O — fire inputs (`AcceptInput`) + hook outputs (`FireOutputInternal` detour)

**Date:** 2026-07-09
**Status:** design approved (both directions, one slice — user-chosen) — proceeding to plan
**Reference:** CounterStrikeSharp's `CEntityInstance::AcceptInput` native + its `FireOutputInternal` funchook
(`src/scripting/natives/natives_entities.cpp`, `src/core/managers/entity_manager.cpp`). Builds on the
entity-creation slice (`EntityRef`, `createEntity`, `remove`) and the damage-detour engine (`shim/src/detour.cpp`).

## Goal

Both directions of Source 2 entity I/O, engine-generic (`CEntityInstance`/`CEntityIOOutput` are Source 2):
1. **Fire inputs** — `EntityRef.acceptInput(input, value?, activator?, caller?) → boolean`: trigger any entity
   input (`Kill`/`Ignite`/`SetHealth`/`Enable`/`Open`/`FireUser1`/`AddOutput`/…).
2. **Hook outputs** — `Entity.onOutput(classname, output, handler)`: react when an entity fires an output
   (`func_button`→`OnPressed`, `trigger_multiple`→`OnStartTouch`, `logic_relay`→`OnTrigger`, …); the handler
   may return a `HookResult ≥ Handled` to SUPPRESS the output (parity with the damage/event pre-hooks).

## Direction 1 — fire inputs (via `AddEntityIOEvent`)

**Mechanism: `CEntitySystem::AddEntityIOEvent`** — UNIQUE ✓ (`@0x2128170`, validated), the game's OWN
input-firing path (it's what map I/O and `FireOutputInternal` route through). Chosen over the direct
`CEntityInstance::AcceptInput` because that sig is **STALE on our build (0 matches — its prologue changed
past a re-derivable prefix)**; `AddEntityIOEvent` needs NO re-derivation and we already hold everything it
needs. `delay 0` = the same-tick I/O pump → effectively immediate for entity I/O (this is how the engine
itself fires connected inputs; a synchronous-within-the-call `AcceptInput` is a deferred optimization).

**ABI (confirmed from CSSharp's C++, `entity_manager.h`):**
```cpp
void AddEntityIOEvent(CEntitySystem* entitySystem, CEntityInstance* target, const char* inputName,
                      CEntityInstance* activator, CEntityInstance* caller, variant_t* value,
                      float delay, int outputID, void* /*=null*/, void* /*=null*/)
// CSSharp: variant_t _value = variant_t(value/*string*/);
//          AddEntityIOEvent(GameEntitySystem(), target, name, act, caller, &_value, delay, id, nullptr, nullptr);
```
The `entitySystem` = the `CGameEntitySystem*` the shim ALREADY resolves for every entity lookup
(GameResourceService + offset 0x50). The `variant_t` is **the SDK type** (`third_party/hl2sdk/public/datamap.h`)
built from a string — **no RE for the value** (Source parses the string per the input's field type; a
value-less input passes an empty string).

**Op** (ABI-appended after the last item op): `entity_fire_input(int idx, int serial, const char* inputName,
const char* value, int actIdx, int actSerial, int callerIdx, int callerSerial, float delay) → int (bool)`.
Shim: serial-gate the target; resolve activator/caller serial-gated (`<0` → null); `variant_t(value)`; call
`AddEntityIOEvent(entitySystem, target, inputName, activator, caller, &variant, delay, 0, nullptr, nullptr)`.
**`EntityRef.acceptInput(inputName, value?, activator?, caller?, delay?)`** (engine-generic, `@s2script/entity`;
activator/caller optional `EntityRef`s; `delay` defaults 0 = same-tick). The name mirrors SM's `AcceptEntityInput`
familiarity; the doc notes it queues same-tick (not synchronous-within-the-call).

## Direction 2 — output hooks (`FireOutputInternal` detour)

**Mechanism:** detour `CEntityIOOutput::FireOutputInternal` (sig UNIQUE ✓ `@0x2132a60`) with the existing
`shim/src/detour.cpp` inline-detour engine (the 6.6 damage-hook pattern), NOT a borrowed vtable index.

**ABI (confirmed):**
```cpp
void FireOutputInternal(CEntityIOOutput* this, CEntityInstance* activator, CEntityInstance* caller,
                        const CVariant* value, float delay, void* unk1, char* unk2)
```
**Extraction (SDK/schema):** output name = `this->m_pDesc->m_pName`; source class = `caller->GetClassname()`
(the `CEntityInstance` classname — `GetClassname()` inline / the `m_pEntity->m_designerName` `string_t`; the
spike confirms the accessor); activator/caller → serial-gated `EntityRef`s (`GetRefEHandle().ToInt()`, never a
raw ptr to JS); value → a string (read `CVariant::m_type` + the typed field, formatted to a string — the
`natives_cvariant.cpp` pattern; the MVP exposes the value as a string).

**Core `output_mux`** (mirrors `event_mux`/`damage_mux`): keyed by `(classname, outputName)` with `"*"`
wildcards (matches CSSharp's search keys — `(class,output)`, `(class,"*")`, `("*",output)`, `("*","*")`);
`remove_by_owner` on unload. Because the hook can **block**, dispatch is **SYNCHRONOUS during the detour** —
the damage/event pre-hook pattern (`dispatch_damage`/`dispatch_game_event_pre`), NOT the post-drain
`dispatch_pending_*` path (that's notify-only). The detour calls core → runs matching subscribers under the
isolate borrow, collapsing `HookResult` via `run_chain`; a **`try_borrow_mut` graceful-skip** guards
re-entrancy (a handler that fires another output skips the nested dispatch — the documented `Events.fire`
limitation). Collapsed `HookResult` ≥ `Handled` → the shim **skips the original `FireOutputInternal`**
(suppresses the output; `MRES_SUPERCEDE`-equivalent). New ops (ABI-appended) for subscribe/unsubscribe + the
detour→core dispatch, mirroring the damage ops.

**API:** `Entity.onOutput(classname: string, output: string, handler: (ev) => HookResultValue | void)` where
`ev = { output, activator: EntityRef|null, caller: EntityRef|null, value: string, delay: number }`. `Entity` is
a new namespace in `@s2script/entity` (alongside `createEntity`). `classname`/`output` accept `"*"`.

## The RE (validated offline against the pinned `libserver.so`, 2026-07-09)

- `CEntitySystem_AddEntityIOEvent` — UNIQUE ✓ `@0x2128170` (**the input-firing primary**; ABI known, entity
  system already resolved).
- `CEntityIOOutput_FireOutputInternal` — UNIQUE ✓ `@0x2132a60` (the output-hook detour target).
- `CEntityInstance_AcceptInput` — **STALE (0 matches; prefix-matching found only a false candidate)** →
  NOT used this slice; the direct synchronous input call is a deferred optimization (`AddEntityIOEvent`
  covers firing inputs).
- `variant_t` — in the vendored SDK (`datamap.h`); `CVariant` read pattern from `natives_cvariant.cpp`.

## Boundary (both gates green)

Everything is Source 2 (`CEntityInstance`/`CEntityIOOutput`/`variant_t`), so `EntityRef.acceptInput` +
`Entity.onOutput` are **engine-generic `@s2script/entity`**; the ops take `(idx, serial, string, …)` — no CS2
schema names in core. The sigs + the `m_pDesc`/`m_pName` offsets are gamedata/SDK. `check-core-boundary.sh` +
`test-boundary-nameleak.sh` stay green. (There are NO CS2-specific names here at all — even the demo is generic.)

## Testing

- **In-isolate (core):** `entity_accept_input` degrades (no op → false / stale → false); the `output_mux`
  subscribe/dispatch/`remove_by_owner` + the `(class,output)` wildcard matching as pure units (mirroring the
  `event_mux` tests); `Entity.onOutput` registers + `HookResult` collapse.
- **Live gate — BOTH directions in ONE self-contained bot-provable flow:** the demo `createEntity("logic_relay")`,
  `Entity.onOutput("logic_relay", "OnTrigger", …)`, `relay.spawn()`, then `relay.acceptInput("Trigger")` →
  `FireOutputInternal` fires `OnTrigger` → our detour → the JS hook logs `caller=logic_relay output=OnTrigger`.
  This proves **the input fire (`AddEntityIOEvent`), the FireOutput detour (output caught), the mux dispatch,
  and the EntityRef marshalling** with zero human client / no map dependency. Also fire `acceptInput("Kill")`
  on a spawned entity → it's gone next tick (`isValid()` false). `GAMEDATA VALIDATION` grows; `RestartCount=0`,
  no crash.

## Non-goals (do NOT build)

- Delayed I/O beyond `delay 0` (the `AddEntityIOEvent` delay/queue surface — use `@s2script/timers` +
  `acceptInput` for delays); entity I/O connection editing (`AddOutput` works via `acceptInput` — no separate API).
- Full typed `CVariant` marshalling both ways — the output value is exposed as a **string** (MVP); input value
  is a **string** (Source parses it). Typed value getters deferred.
- `activator`/`caller` defaulting to anything but null when omitted (SM often defaults caller=self; we pass null
  unless given — the spike confirms whether null caller is safe for common inputs, else default caller=target).

## Sequencing (spike-first, one slice)

0. **Spike (small — the input sig is already validated)** — confirm the `CEntityIOOutput::m_pDesc->m_pName`
   + `CEntityInstance::GetClassname` accessors + the `CVariant` read against the pinned binary / the vendored
   SDK types (the FireOutput detour's extraction). Record the gamedata (`AddEntityIOEvent` + `FireOutputInternal`
   sigs, both already UNIQUE-validated).
1. **Core ops + `EntityRef.acceptInput` + `output_mux` + `Entity.onOutput` + degrade/mux tests.**
2. **Shim — `AcceptInput` call (variant_t via SDK) + the `FireOutputInternal` detour** (extract name/class/
   activator/caller/value, dispatch to core, `HookResult`→supersede); sniper build.
3. **Demo + the both-directions bot-provable live gate** (`logic_relay` + `OnTrigger` + `acceptInput`).

Needs one sniper rebuild (the ops + the detour). Related: [[cs2-damage-hooks-detour]], [[entity-creation-and-beam]],
[[js-dispatch-isolate-borrow-reentrancy]], [[re-gamedata-strategy]].
