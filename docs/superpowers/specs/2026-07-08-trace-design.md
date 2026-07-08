# Ray tracing (`@s2script/trace`) — Design

**Status:** Approved (brainstorm — all three variants + `pawn.aimTrace` + engine-generic boundary + vtable-by-name self-resolve decided), ready for the plan.
**Slice:** the SourceMod `sdktools_trace` equivalent — a `@s2script/trace` module that ray-traces the Source 2 physics world (line / aim / hull), plus a CS2 `pawn.aimTrace` convenience. The first big SDKTools-family capability.

## Goal

Let a plugin cast a ray through the world and learn what it hit: `Trace.line(start, end)` / `Trace.ray(start, angles, distance)` / `Trace.hull(start, end, mins, maxs)` → a `TraceHit` (did-hit, fraction, impact position, surface normal, hit entity, all-solid). Plus `pawn.aimTrace()` — trace from a player's eyes along their aim (the killer use case: "what is this player looking at"). Reference for the CS2 mechanism: the FUNPLAY-pro-CS2/Ray-Trace Metamod plugin.

## Motivation & context

Ray tracing is a foundational SDKTools capability that unlocks a large class of plugins: aim/line-of-sight checks, surface placement (spawn a prop where the player looks), bullet-penetration/anti-cheat, laser/beam targeting, "use" interactions. s2script has none of the SDKTools breadth yet; trace is the highest-leverage first piece and exercises a **new, reusable RE capability** (resolving a class vtable by name in our own binary), which future SDKTools slices (entity I/O, gamerules) can reuse.

## The CS2 mechanism (from the reference)

The physics trace is a virtual method on the **`CNavPhysicsInterface`** object in `libserver.so`, reached NOT via `CreateInterface` but by **RTTI vtable-by-name**:

```cpp
void** vtable = CModule(libserver).GetVirtualTableByName("CNavPhysicsInterface");
auto TraceShape = vtable[ /*gamedata offset*/ 5 /*linux*/ ];
// bool TraceShape(void* /*this = nullptr*/, Ray_t& ray, Vector& start, Vector& end,
//                 CTraceFilter* filter, CGameTrace* trace)
TraceShape(nullptr, ray, start, end, &filter, &trace);
```

- **`Ray_t`** — `ray.Init(mins, maxs)`; a **zero hull** (`mins == maxs == {0,0,0}`) is a line/ray, a non-zero hull is a swept box. (Exact struct layout resolved in the RE spike — Task 1.)
- **`CTraceFilterEx`** — `m_nInteractsAs = 0`, `m_nInteractsWith = <mask>` (uint64), `m_nInteractsExclude = 0`, plus an optional ignore-entity (`CBaseEntity*`). The mask is a bitset of `InteractionLayers` (42 flags); 7 predefined masks exist (`MASK_SHOT_PHYSICS` [default], `MASK_SHOT_HITBOX`, `MASK_SHOT_FULL`, `MASK_WORLD_ONLY`, `MASK_GRENADE`, `MASK_BRUSH_ONLY`, `MASK_PLAYER_MOVE`).
- **`CGameTrace`** (output) → `EndPos` (Vector), `HitEntity` (`CEntityInstance*`), `Fraction` (float), `AllSolid` (int), `Normal` (Vector). (Exact offsets resolved in Task 1.)

All three reference wrappers (`TraceShape` angle-based, `TraceEndShape` two-point, `TraceHullShape` box) call the **same** underlying `TraceShape` with a different `Ray_t`/start/end — so s2script needs **one** engine op, not three.

## Architecture

One-way deps (game → core). The trace mechanism is Source 2 physics → **engine-generic** (shim + core); only `pawn.aimTrace` (eye pos/angles field names) is CS2.

### One engine op — `trace_shape` (engine-generic)

Appended to `S2EngineOps` after `config_write_file` (C header + Rust mirror + both test op-structs). Signature (flat ABI + an out struct defined in `s2script_core.h`):

```c
typedef struct { int didHit; float fraction; float endpos[3]; float normal[3];
                 int allSolid; int hitEntHandle; } S2TraceResult;
int trace_shape(const float* start /*3*/, const float* end /*3*/,
                const float* mins /*3*/, const float* maxs /*3*/,
                unsigned long long interactsWith, unsigned long long interactsExclude,
                int ignoreEntIdx, int ignoreEntSerial, S2TraceResult* out);
```

The shim: build `Ray_t` (`Init(mins,maxs)`), `CTraceFilterEx` (the masks + the ignore entity resolved from the `(idx,serial)` via the existing entity-system lookup), `CGameTrace`; call `TraceShape(nullptr, ray, start, end, &filter, &trace)`; copy the result into `*out` (`hitEntHandle` = the hit `CEntityInstance`'s `GetRefEHandle().ToInt()`, or `-1` if none). **`!s_pTraceShape`-guarded** → returns `0` (op unavailable) so the native degrades. The raw `CGameTrace`/pointers **never cross to JS** — only the flat `S2TraceResult`.

### The native + `@s2script/trace` module (engine-generic, core prelude)

`__s2_trace(startArr, endArr, minsArr, maxsArr, interactsWith, interactsExclude, ignoreIdx, ignoreSerial)` → builds a `TraceHit` object: `endPos`/`normal` → `Vector` (via `__s2pkg_math`), `hitEntHandle` → decode to a serial-gated `EntityRef | null` (the `__s2_handle_decode` / `DamageInfo.victim` path), `didHit`/`allSolid` → bool, `fraction` → number. Returns a "miss" `TraceHit` (`didHit:false, fraction:1, entity:null`) when the op is unavailable (degrade).

`@s2script/trace` (types-only `.d.ts` + the prelude runtime, `__s2pkg_trace`):
- `Trace.line(start, end, opts?)` → the op with `mins=maxs={0,0,0}`.
- `Trace.ray(start, angles, distance, opts?)` → compute `end = start + forward(angles) * distance` (a new pure `QAngle`→forward helper in `@s2script/math`: `x=cos(pitch)cos(yaw)`, `y=cos(pitch)sin(yaw)`, `z=-sin(pitch)`, degrees→radians), then a line trace.
- `Trace.hull(start, end, mins, maxs, opts?)` → the op with the given hull.
- `opts = { mask?: TraceMask, ignoreEntity?: EntityRef }` (default `TraceMask.ShotPhysics`); `ignoreEntity` → its `(index, serial)`, else `(-1,-1)`.
- `TraceMask` = the 7 predefined uint64 masks (as JS **numbers**? — the masks fit in 53 bits per the layer set, but to be safe pass them as the op's `unsigned long long`; the module holds them as numeric constants and the native marshals to u64). `TraceHit` shape as above.

### CS2 layer — `pawn.aimTrace(opts?)` (`@s2script/cs2`)

In `pawn.js` + `packages/cs2/index.d.ts`: `eye = pawn.sceneNode.absOrigin + {0,0,viewOffsetZ}` (the standing/crouching eye height — read the pawn's view-offset field, else a sane constant), `angles = pawn.eyeAngles`; `return Trace.ray(eye, angles, opts?.distance ?? 8192, opts)` with `ignoreEntity` defaulting to the pawn's own ref (don't hit yourself). CS2 field names stay in the game layer.

## RE + validation (the self-resolve doctrine)

The main new capability is **vtable-by-name resolution via an RTTI scan** (`shim/src/`). Recon (`nm`/`strings` on the pinned `libserver.so`) confirmed: the module's `.symtab` is **stripped** and the game-class vtables are **not** in `.dynsym` (so `dlsym` cannot find `CNavPhysicsInterface`) — but the Itanium **RTTI typeinfo-name string `20CNavPhysicsInterface`** IS present in `.rodata` (`0x7f5460` in this build; `20` = the mangled-length prefix). So `GetVTableByName(module, "CNavPhysicsInterface")` walks RTTI, exactly like the reference's vendored **DynLibUtils `module_linux.cpp::GetVirtualTableByName`** (port/adapt it): (1) find the `<len><name>` typeinfo-name string in `.rodata`; (2) find the `type_info` object that references it (scan the module for a pointer to the name string — that location is the typeinfo's `name` field); (3) find the vtable whose typeinfo slot points to that typeinfo (scan for a pointer to the typeinfo; the vtable's first virtual fn is at `that_location + 8`). This **self-resolves the vtable in our own binary** by RTTI (doctrine-compliant, no borrowed address); the reusable helper future SDKTools slices reuse. It uses the existing `dl_iterate_phdr` module-range walk (as `FindModuleText` does) to bound the `.rodata`/`.data.rel.ro` scans. Degrade-safe: if the typeinfo/vtable isn't found, the op is disabled.

The **`TraceShape` vtable index** (gamedata `offsets`, `CNavPhysicsInterface_TraceShape: { linuxsteamrt64: 5 }`) is a borrowed constant (the `sm_slay`-index risk), so it is **validated**, not trusted:
1. **Load-time:** the resolved `vtable[index]` pointer must land in `libserver.so`'s executable segment (the `.text` `PF_X` range from `FindModuleText`), else the op is disabled + a loud gamedata-validation line (degrade-per-descriptor). Joins the `=== GAMEDATA VALIDATION ===` gate count.
2. **First-use smoke test:** the first `Trace` call after map-live runs a fixed known ray (e.g. `(0,0,64)`→`(0,0,-16384)`) and asserts `fraction` is finite ∈ `[0,1]` and `endpos` is finite; on garbage it marks the op degraded + warns once (guards a wrong-but-valid index / a bad struct layout returning garbage). A hard crash from a totally wrong index isn't catchable in-process — that surfaces at the live gate (the treadmill).

The `Ray_t`/`CTraceFilterEx`/`CGameTrace` **struct layouts** are defined in the shim from the reference and proven at the **live gate** (a real aim-trace returning sane world coords); a per-update layout/offset change is a gamedata/struct regeneration (the treadmill).

## Testing & gate

- **Core unit tests** (in-isolate, like the damage/event natives): `__s2_trace` with no op → a miss `TraceHit` (`didHit:false, fraction:1, entity:null`); the `TraceHit` object shape (Vector endPos/normal, EntityRef-or-null); the `QAngle`→forward math (a known angle → a known direction); `TraceMask` constant values.
- **Live gate (bots-provable):** on de_* with `bot_quota 2` (rcon-triggered via a demo/basecommands debug): `pawn.aimTrace()` from a bot's eyes → `didHit=true` at plausible world coords + a unit-ish `normal` (a bot faces a wall/floor); `Trace.line` straight down from above a bot → hits the floor (`fraction<1`); a `Trace.line` into open sky → `didHit=false, fraction=1`; `ignoreEntity=<the pawn>` doesn't self-hit; if the aim crosses another bot, `entity` is that bot's `EntityRef` (validate `.readInt32(m_iHealth-ish)` or the controller nav). `=== GAMEDATA VALIDATION ===` count +1 (green). `RestartCount=0`, no crash.
- **Gates:** core-boundary (the trace op/module are engine-generic — Source 2 physics), name-leak, `scripts/check-plugins-typecheck.sh`, full `cargo test`. One sniper (the shim vtable-resolution + the op + the prelude).

## Boundary & safety summary

The physics trace (`CNavPhysicsInterface`, `Ray_t`, `CGameTrace`, `InteractionLayers`) is Source 2 → the `trace_shape` op + `@s2script/trace` are **engine-generic** (core/shim). Only `pawn.aimTrace` (CS2 eye-offset/`eyeAngles`/`m_hOwnerEntity` field names) is game-layer. No raw pointer crosses to JS — the op returns a flat `S2TraceResult`; the hit entity crosses as an `(index,serial)` `EntityRef`, serial-gated on every access (a stashed hit entity that dies reads `null`, never garbage). The ignore-entity is passed as `(idx,serial)` and re-resolved in-shim. Masks are opaque uint64 bitsets. Degrade-never-crash: an unresolved vtable/index disables the op (miss result); a stale layout is caught by the smoke test + the live gate. Both boundary gates stay green.
