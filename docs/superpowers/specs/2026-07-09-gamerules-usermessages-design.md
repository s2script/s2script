# Game Rules + General UserMessages — design

**Date:** 2026-07-09
**Status:** approved (combined slice; full CS2 sugar incl. `sm_blind`)

## Goal

Close two CSSharp-parity gaps in one slice:

1. **Game rules** — read `CCSGameRules` state (warmup / freeze / round time / rounds played / game phase / bomb / scores) from a plugin.
2. **General UserMessages** — send *any* protobuf user message (not just chat) to clients, with typed CS2 sugar for the common ones (`Fade`→`sm_blind`, `Shake`, `HintText`).

Both are generalizations of mechanisms we already own: game rules reuses the 5C.5 pointer-chain nav (`readVia`) plus one new "find entity by class" primitive; usermessages generalize the proven SayText2 / `client_print` protobuf-reflection send path.

## Background / feasibility (confirmed)

- **Game rules access (CSSharp's way):** there is no exported `g_pGameRules`. CSSharp finds the `cs_gamerules` proxy entity (`FindAllEntitiesByDesignerName<CCSGameRulesProxy>`) and reads its `m_pGameRules` pointer. Our schema catalog already has `CCSGameRulesProxy.m_pGameRules` **and** `CCSGameRules` (189 fields incl. `m_bWarmupPeriod`, `m_bFreezePeriod`, `m_iRoundTime`, `m_iFreezeTime`, `m_totalRoundsPlayed`, `m_gamePhase`, `m_bBombPlanted`, `m_nRoundsPlayedThisPhase`, `m_bGameRestart`, `m_flGameStartTime`, `m_bMatchWaitingForResume`, `m_bHasMatchStarted`). So access = **proxy `EntityRef` → nav `m_pGameRules` → CCSGameRules schema fields**, exactly the serial-gated `readVia` pattern (self-healing across map changes: the proxy dies and re-resolves). The one missing primitive is **finding an entity by class/designer-name**.
- **The entity iteration** is available: the shim already has the `GameEntitySystem()` bridge (built for EKV) and the SDK's `CEntityIdentity` layout (`m_designerName`, the `CEHandle`). Iterating identities + filtering by designer-name is engine-generic.
- **UserMessage send:** `shim/src/s2script_mm.cpp` already does the full path for SayText2 (`FindNetworkMessagePartial(name)` → `AllocateMessage()` → protobuf reflection field-set → `IGameEventSystem::PostEventAbstract`). Generalizing it to an arbitrary named message + arbitrary scalar fields is wiring, not new RE.
- **The common CS2 messages are all-scalar** (verified against the vendored SDK protos): `CUserMessageFade{ duration:uint32, hold_time:uint32, flags:uint32, color:fixed32 }`, `CUserMessageShake{ command:uint32, amplitude:float, frequency:float, duration:float }`, `CUserMessageHudMsg`, `CUserMessageTextMsg`. Scalar-only reflection (int / uint / fixed / int64 / bool / string / float) covers every message we ship sugar for. **Nested-message fields (e.g. a `CMsgRGBA`/`CMsgVector` sub-object) are out of scope this slice** — none of our sugar needs them (`Fade.color` is a packed `fixed32`, not nested).

## Architecture

### New engine primitives (ops — ABI-appended after `entity_spawn_kv`, the current last op)

The ABI-append discipline is mandatory: each new op is byte-identical across the C header (`shim/include/s2script_core.h`), the Rust mirror (`core/src/v8host.rs`), **both** in-isolate test op-structs, and the shim `ops.` assignment — appended in this exact order, never inserted mid-struct.

1. `int entity_find_by_class(const char* className, int* outIndices, int* outSerials, int maxCount)` — the shim walks the entity-identity list via `GameEntitySystem()`, compares each `CEntityIdentity::m_designerName` to `className` (exact, case-sensitive — designer-names are canonical), writes `(index, serial)` pairs for the first `maxCount` matches, and returns the **total match count**. The caller consumes `min(returned, maxCount)` entries and can detect truncation when `returned > maxCount`. Engine-generic.
2. `int user_message_create(const char* name)` — `FindNetworkMessagePartial(name)` → `AllocateMessage()` into a shim-side `s_currentUserMessage` target (mirrors the 5D.3 `s2_currentEvent` single-target model). Returns 1 on success, 0 if the message name is unknown. Frees any previously-allocated-but-unsent message first (defensive).
3. `int user_message_set_int(const char* field, int64_t value)` — reflection: find `field` on the current message, dispatch by the field's protobuf CppType (`INT32`→SetInt32, `UINT32`/`fixed32`→SetUInt32, `INT64`→SetInt64, `UINT64`/`fixed64`→SetUInt64, `ENUM`→SetEnumValue, `BOOL`→SetBool, and `FLOAT`/`DOUBLE`→SetFloat/SetDouble as a coercion). Returns 1 if set, 0 if the field is absent. Covers Fade's uint32/fixed32 fields and Shake's `command`.
4. `int user_message_set_float(const char* field, double value)` — reflection dispatch to SetFloat/SetDouble (covers Shake `amplitude`/`frequency`/`duration`).
5. `int user_message_set_string(const char* field, const char* value)` — reflection SetString.
6. `int user_message_set_bool(const char* field, int value)` — reflection SetBool (completeness; TextMsg-family).
7. `int user_message_send(const int* slots, int slotCount)` — `PostEventAbstract` the current message to the given client slots; `slotCount < 0` = broadcast to all connected non-bot clients (loop live slots like `client_print`). **Bot-skip guarded** (`GetPlayerNetInfo(slot) != null`) — a send to a fake client's null netchannel can crash (the 6.1c live finding). Deallocates/clears `s_currentUserMessage` after send. Returns 1 on send, 0 if no message is currently built.

`s_currentUserMessage` is a build-then-send single target with no `await` in between — the JS `UserMessage.send()` performs `create → set* → send` atomically in one synchronous burst, so there is no cross-message aliasing (same guarantee the block-scoped `Events.fire` relies on).

### Engine-generic JS modules

- **`@s2script/entity`** gains `Entity.findByClass(className: string): EntityRef[]` — calls `entity_find_by_class` with a bounded out-buffer (e.g. 1024), builds serial-gated `EntityRef`s via the existing `build_entity_ref` path, returns `[]` on no-op/degrade. Broadly reusable (props, doors, triggers, controllers — everything CSSharp's `FindAllEntitiesByDesignerName` unlocks).
- **`@s2script/usermessages`** (new types-only package + prelude module) — a `UserMessage` builder:
  ```
  const m = new UserMessage("CUserMessageFade");
  m.setInt("duration", 1536).setInt("flags", FFADE_OUT | FFADE_PURGE).setInt("color", 0xFF000000);
  m.send([slot]);        // or m.sendAll();
  ```
  Fields accumulate in a JS list `{name, type, value}`; `.send(slots)` flushes `user_message_create(name)` → one typed `set_*` per field → `user_message_send(slots)` in a single synchronous burst. `.set(field, value)` infers the set op by JS type (number-integer→setInt, number-float→setFloat, string→setString, boolean→setBool). Degrades to no-op if `create` returns 0 (unknown message). Engine-generic — no CS2 dependency (message *names* are the caller's).

### CS2 layer (`@s2script/cs2`, `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`)

- **`GameRules`** — `GameRules.get(): GameRules | null` finds the `cs_gamerules` proxy via `Entity.findByClass("cs_gamerules")[0]` and returns a wrapper whose getters read `CCSGameRules` fields through `m_pGameRules` using the serial-gated pointer-chain reads (`readBoolVia`/`readInt32Via`/`readFloat32Via`, path `[schemaOffset("CCSGameRulesProxy","m_pGameRules")]`, final = `schemaOffset("CCSGameRules","<field>")`). Offsets live-resolved per access (self-healing); reads `null` if the proxy is gone. Exposed fields (idiomatic names): `warmupPeriod`, `freezePeriod`, `roundTime`, `freezeTime`, `totalRoundsPlayed`, `gamePhase`, `bombPlanted`, `roundsPlayedThisPhase`, `gameRestart`, `gameStartTime`, `matchWaitingForResume`, `hasMatchStarted`. `GameRules.get()` re-finds the proxy each call (game rules is not a hot path); returns `null` when no proxy exists (e.g. pre-map-load). **CS2 field names live only here.**
- **UserMessage sugar** — typed helpers over the generic builder:
  - `Fade.to(slot, {duration, holdTime?, color?, flags?})` and `Fade.blind(slot, durationMs)` (= a black `FFADE_OUT|FFADE_PURGE` fade — the `sm_blind` effect). `color` defaults to opaque black.
  - `Shake.to(slot, {amplitude, frequency, duration})`.
  - `HintText.to(slot, text)` — the plan resolves the exact CS2 hint message (the `CUserMessageHudMsg`/hint-channel used by SM's `PrintHintText`) during the shim spike; if no scalar-only hint message resolves cleanly, HintText falls back to a center-print and is noted as a follow-up (Fade + Shake are the load-bearing sugar).
  - Fade/Shake flag constants (`FFADE_IN=1`, `FFADE_OUT=2`, `FFADE_MODULATE`, `FFADE_STAYOUT`, `FFADE_PURGE=16`).
- **`sm_blind <target> [duration]`** wired into the existing `@s2script/funcommands` plugin (`registerAdmin(ADMFLAG.SLAY)`, `forEachPawn` target resolution like the other funcommands) → `Fade.blind(slot, duration*1000)`. Closes the funcommands deferral.

### Demo plugin

`plugins/gamerules-usermsg-demo/` — `sm_gamerules` prints `GameRules.get()` fields (warmup/freeze/roundTime/totalRoundsPlayed/gamePhase) + `Entity.findByClass("cs_gamerules").length`; `sm_umsg <slot>` sends a `Fade` + a `HintText` to a slot (bots-provable send path). Exercises findByClass + GameRules + the generic UserMessage builder end-to-end.

## Boundary (both gates must stay green)

- **Core / shim (engine-generic):** `entity_find_by_class` + the entity-identity iteration; the whole UserMessage send machinery (protobuf reflection over a *named* message, a Source2 concept). `CEntityIdentity`/`CGameEntitySystem`/`IGameEventSystem` are Source2 engine types → shim-only.
- **CS2 layer:** the `CCSGameRules`/`CCSGameRulesProxy` field names, the `cs_gamerules` designer-name, the `CUserMessageFade`/`CUserMessageShake` message names, all the sugar, and `sm_blind`.
- Litmus: `findByClass` and `UserMessage("<name>")` would be true on any Source2 game; only the specific class/message/field strings are CS2.

## Testing

**In-isolate (core, `RUST_TEST_THREADS=1`):** the natives degrade correctly with no engine (`entity_find_by_class` → `[]`; `user_message_create`/`set_*`/`send` → 0/no-op, never panic); the `s2_ent_ref_*`-style build path for the returned refs; op-struct ABI parity (both test op-structs updated with the 7 new fields in order). No V8-dependent behavioral test for the reflection path (that's the live gate).

**Live gate (de_inferno / de_dust2, `bot_quota 2`, rcon):**
- `sm_gamerules` → `GameRules.get()` reads plausible live values (`warmupPeriod`/`freezePeriod` boolean, `roundTime` the configured value, `totalRoundsPlayed`≥0, `gamePhase` an int), and `findByClass("cs_gamerules").length === 1`. **Fully bots-provable** — game-rules state exists and changes without a human.
- `findByClass` sanity: a common class (e.g. `cs_gamerules`, or a spawned entity's class) returns the expected count; a nonsense class returns 0.
- `sm_blind <bot>` / `sm_umsg <slot>` → the send path executes without crash and fields resolve (bots-provable **construction + reflection + send**). GAMEDATA validation count increases by the new sigs (if any) and stays `N ok, 0 FAILED`; `RestartCount=0`.

**Human-client deferral (documented, same ceiling as SayText2's visible chat line):** *visually seeing* the blackout / shake / hint on a real client. The construction, field-resolution, and send path are all live-proven on bots; the pixels need a human.

## Deferred (do NOT build ahead)

- Nested-message user-message fields (`CMsgRGBA`/`CMsgVector` sub-objects); repeated fields.
- Writing `CCSGameRules` fields (round control — `TerminateRound`, warmup toggles); those need game-function RE.
- `GameRules` as a codegen (navgen) target rather than the hand-written wrapper — a cleanup once the field set stabilizes.
- A general `Entity` wrapper / typed `findByClass<T>` returning typed CS2 entities (returns raw `EntityRef[]` this slice; callers wrap).
- The human-client visual verification of Fade/Shake/Hint.

## Slice shape

One combined slice. One sniper rebuild (7 new ops + shim entity-iteration + reflection generalization). Built via Workflow (per-task implement → adversarial-review → fix), opus final review, live gate, merge, push, document.
