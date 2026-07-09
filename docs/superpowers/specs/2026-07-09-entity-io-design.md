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

## Direction 1 — `AcceptInput` (fire inputs)

**ABI (confirmed from CSSharp's C++ native):**
```cpp
AcceptInput(CEntityInstance* this, const char* inputName, CEntityInstance* activator,
            CEntityInstance* caller, variant_t* value, int outputID /*=0*/, void* /*=nullptr*/)
// CSSharp: variant_t _value = variant_t(value /*string*/); AcceptInput(this, name, act, caller, &_value, id, nullptr);
```
The `variant_t` is **the SDK type** (`third_party/hl2sdk/public/datamap.h`) constructed from a string —
**no RE for the value** (Source parses the string per the input's expected field type). A value-less input
passes an empty string → empty variant.

**The one spike:** the CSSharp `CEntityInstance_AcceptInput` signature is **STALE on our build (0 matches)**.
Re-derive it on the pinned `libserver.so` — cleanest via the **`FireOutputInternal` xref** (`@0x2132a60`,
UNIQUE — an output firing a targeted input calls `AcceptInput`/`AddEntityIOEvent`), or RTTI on the
`CEntityInstance` vtable. **Fallback:** `CEntitySystem_AddEntityIOEvent` (UNIQUE ✓ `@0x2128170`) queues the
input (delay 0 → next-think) if the direct `AcceptInput` re-derive proves hard.

**Op** (ABI-appended after the last item op): `entity_accept_input(int idx, int serial, const char* inputName,
const char* value, int actIdx, int actSerial, int callerIdx, int callerSerial) → int (bool)`. Shim: serial-gate
the target; resolve activator/caller serial-gated (`<0` → null); `variant_t(value)`; call the re-derived
`AcceptInput(target, inputName, activator, caller, &variant, 0, nullptr)`. **`EntityRef.acceptInput(inputName,
value?, activator?, caller?)`** (engine-generic, `@s2script/entity`; activator/caller are optional `EntityRef`s).

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
`remove_by_owner` on unload; dispatched **post-drain** (the `dispatch_pending_*` pattern — the detour queues,
the fan-out runs HOST-free after `frame_async_drain`, dodging the isolate-borrow re-entrancy). The collapsed
`HookResult` (via `run_chain`) ≥ `Handled` → the shim `MRES_SUPERCEDE`s / skips the original `FireOutputInternal`
(suppresses the output). New ops (ABI-appended) for subscribe + the detour→core dispatch, mirroring the event ops.

**API:** `Entity.onOutput(classname: string, output: string, handler: (ev) => HookResultValue | void)` where
`ev = { output, activator: EntityRef|null, caller: EntityRef|null, value: string, delay: number }`. `Entity` is
a new namespace in `@s2script/entity` (alongside `createEntity`). `classname`/`output` accept `"*"`.

## The RE (validated offline against the pinned `libserver.so`, 2026-07-09)

- `CEntityIOOutput_FireOutputInternal` — UNIQUE ✓ `@0x2132a60` (the output-hook detour target).
- `CEntitySystem_AddEntityIOEvent` — UNIQUE ✓ `@0x2128170` (the queued-input fallback).
- `CEntityInstance_AcceptInput` — **STALE (0 matches)** → re-derive (spike; the CSSharp ABI + the SDK
  `variant_t` are the givens, only the byte pattern moved).
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
  This proves **AcceptInput (input fired), the FireOutput detour (output caught), the mux dispatch, and the
  EntityRef marshalling** with zero human client / no map dependency. Also fire `acceptInput("Kill")` on a
  spawned entity → it's gone (`isValid()` false). `GAMEDATA VALIDATION` grows; `RestartCount=0`, no crash.

## Non-goals (do NOT build)

- Delayed I/O beyond `delay 0` (the `AddEntityIOEvent` delay/queue surface — use `@s2script/timers` +
  `acceptInput` for delays); entity I/O connection editing (`AddOutput` works via `acceptInput` — no separate API).
- Full typed `CVariant` marshalling both ways — the output value is exposed as a **string** (MVP); input value
  is a **string** (Source parses it). Typed value getters deferred.
- `activator`/`caller` defaulting to anything but null when omitted (SM often defaults caller=self; we pass null
  unless given — the spike confirms whether null caller is safe for common inputs, else default caller=target).

## Sequencing (spike-first, one slice)

0. **Spike** — re-derive `AcceptInput` on the pinned binary (FireOutputInternal-xref / RTTI; AddEntityIOEvent
   fallback); confirm the `CEntityIOOutput::m_pDesc->m_pName` + `GetClassname` accessors + the `CVariant` read.
   Record the gamedata (the re-derived AcceptInput sig + the FireOutputInternal sig).
1. **Core ops + `EntityRef.acceptInput` + `output_mux` + `Entity.onOutput` + degrade/mux tests.**
2. **Shim — `AcceptInput` call (variant_t via SDK) + the `FireOutputInternal` detour** (extract name/class/
   activator/caller/value, dispatch to core, `HookResult`→supersede); sniper build.
3. **Demo + the both-directions bot-provable live gate** (`logic_relay` + `OnTrigger` + `acceptInput`).

Needs one sniper rebuild (the ops + the detour). Related: [[cs2-damage-hooks-detour]], [[entity-creation-and-beam]],
[[js-dispatch-isolate-borrow-reentrancy]], [[re-gamedata-strategy]].
