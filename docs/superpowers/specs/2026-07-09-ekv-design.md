# Slice: EKV — CEntityKeyValues-configured entity spawn

**Date:** 2026-07-09
**Status:** design approved — proceeding to plan
**Reference:** CounterStrikeSharp compiles the SDK's own `entity2/entitykeyvalues.cpp` (its `CMakeLists.txt`
lines 33-35 compile `entity2/entityidentity.cpp`, `entity2/entitysystem.cpp`, `entity2/entitykeyvalues.cpp`)
and passes the built `CEntityKeyValues*` as `DispatchSpawn`'s second argument. Builds directly on the
entity-creation slice (`createEntity`/`spawn`/`remove`, the resolved `s_pDispatchSpawn`) and the entity-I/O
slice (`acceptInput` + `Entity.onOutput` — used by the live gate).

## Motivation

`createEntity(className)` + direct schema-field writes + `DispatchSpawn(entity, nullptr)` works for entities
whose `Spawn()` needs no configuration (the `env_beam` case). It is fragile for the large class of entities
whose `Spawn()` **parses keyvalues**: props need a `model` before spawn, logic entities take `startvalue`/
`min`/`max`, anything nameable takes `targetname`, and spawn-time-only keys have no post-spawn schema
equivalent. The industry-consistent mechanism (SourceMod's `DispatchKeyValue` + `DispatchSpawn`; CSSharp's
`DispatchSpawn(entity, CEntityKeyValues)`) is: create → build a `CEntityKeyValues` map → `DispatchSpawn(entity,
keyValues)` so the entity's **own** `Spawn()` parses them through the engine's keyfield machinery.

## API (engine-generic — `CEntityKeyValues` is a Source 2 type → `@s2script/entity`, NOT a game package)

```ts
export type EntityKeyValueMap = { [key: string]: string | number | boolean };

/** With keyvalues: create + DispatchSpawn(kv) in one call — non-null result = a LIVE, SPAWNED entity.
 *  Without: unchanged (create only; caller sets fields then calls .spawn()). */
export declare function createEntity(className: string, keyvalues?: EntityKeyValueMap): EntityRef | null;

/** The explicit lower-level path: create yourself, optionally set schema fields, then spawn with kv. */
// on EntityRef:
spawn(keyvalues?: EntityKeyValueMap): boolean;
```

- **Value-type inference (JS-side, at the marshal):** `string` → `SetString`; `boolean` → `SetBool`;
  `number` → `SetInt` when `Number.isInteger(v)` and it fits int32, else `SetFloat`. Non-finite numbers,
  non-`string|number|boolean` values, empty keys, or >256 keys → the WHOLE call returns `false` (loud,
  no partial spawn — never a silently half-configured entity).
- **Keys are case-insensitive** — `EntityKeyId_t` = `CKV3MemberName`, hashed via `MurmurHash2LowerCase`
  (the shim's 5D.1 self-contained byte-for-byte copy of Valve's — the SAME function the engine uses to look
  the members back up, so hashes match by construction).
- **`createEntity(cls, kv)` failure hygiene:** if create succeeds but the kv-spawn fails, the unspawned
  entity is `remove()`d and `null` returned — the non-null result invariant above holds.
- **Numeric coercion is engine-side:** an int-tagged value read by a float keyfield goes through KV3's own
  `GetFloat`-style conversion (JS can't distinguish `10` from `10.0`). The live gate deliberately tests this
  (`{max: 10}` → `m_flMax` float32 reads back `10`).

## Feasibility — compile the SDK source into the shim (the central risk)

**Approach:** the `Set*` methods are **inline in the vendored header**
(`third_party/hl2sdk/public/entity2/entitykeyvalues.h`), but the ctor, the private `SetKeyValue`,
`Release`/dtor, and the KeyValues3 internals are non-inline, NOT exported by `libserver.so`, and in no
vendored `.a`. The dota-branch SDK **ships the source**: `third_party/hl2sdk/entity2/entitykeyvalues.cpp`
(333 lines) + `third_party/hl2sdk/tier1/keyvalues3.cpp` (2091 lines). CSSharp compiles exactly these files
into its own module — we do the same: compile them into the shim, isolated behind ONE new TU
(`shim/src/ekv.cpp`, the only file that includes `entitykeyvalues.h`), and self-shim residual undefined
symbols via the established `tier1_shims.cpp` pattern.

**Cascade inventory (from reading the sources — the spike verifies with `nm -u`):**
- **Memory is libc** — `keyvalues3.cpp` uses plain `malloc`/`free`/`realloc`/`new` directly (verified by
  grep), NOT `g_pMemAlloc`. This kills the biggest cascade risk up front.
- `GameEntitySystem()` — an extern free function the SDK expects the CONSUMER to define (CSSharp defines its
  own). We define it in `ekv.cpp`, bridging to the shim's existing per-call `GetEntitySystem()` resolver
  (GameResourceService + the gamedata offset). Null-safe: the `EKV_ALLOCATOR_NORMAL` paths we use only
  *check* it, never require it.
- `Warning(...)` / `Log_Msg` → `LoggingSystem_Log` (tier0) — referenced by `SetKeyValue` misuse paths and
  `AddRef`/`Release` logging. `libtier0.so` DOES export these on the live build (verified `nm -DC`), and the
  2000870 treadmill event proved the shim's dlopen resolves against engine-loaded libs — but Valve can drop
  exports (that's exactly what broke 2000870), so **prefer small self-shims** (fprintf/no-op) for logging;
  they're leaves.
- `Plat_FatalErrorFunc` (tier0) — KV3's genuinely-fatal overflow paths. Self-shim: log + `abort()` (Valve
  aborts too).
- `V_strncpy` / `V_StringToInt32` / `V_StringToFloat64` (tier1 strtools), `CBufferString`/`CUtlBuffer`
  methods (the 6.18 dlopen-cascade class) — referenced only from KV3 paths we never call (`ToString`,
  serialization, array-from-strings). Compile the SDK TUs with `-ffunction-sections -fdata-sections` and
  link with `-Wl,--gc-sections` so unreachable functions drop their undefined references before they can
  cascade (the shim's own exports are explicit-visibility roots, so nothing needed is GC'd). Any that
  SURVIVE GC are triaged: leaf → self-shim; genuine cascade → the stop rule.
- `CGameEntitySystem::FindFirstEntityHandleByName` (non-inline, `entitysystem.cpp`) — referenced only by
  `GetEHandle`, which we never call → expect GC; else a degrade stub. The allocator-ref methods
  (`GetEntityKeyValuesAllocator` etc.) are **inline** in the header (verified) — no `entitysystem.cpp`
  needed. We deliberately do NOT compile `entitysystem.cpp`/`entityidentity.cpp` (CSSharp needs them for
  other surface; we don't).
- `CUtlLeanVector<…, CMemAllocAllocator>` (the `m_connectionDescs` member) — may reference tier0
  `MemAlloc_*` exports. We never call `AddConnectionDesc`, so expect GC; if a memory symbol survives and is
  reachable, prefer **live-resolution from `libtier0.so`** over a malloc self-shim (same-heap-as-engine
  matters for memory in a way it doesn't for logging).

**The spike (Task 1, MUST come first) exit criterion:** the sniper-built shim **dlopens on the live CS2
container** (no fatal undefined symbol) and a load-time **EKV self-test** (`new CEntityKeyValues()` on the
stack → `SetInt`/`SetString` → `GetInt`/`GetString` round-trip) logs
`[s2script] EKV self-test: OK`. **Stop rule:** if after the GC-sections lever the surviving undefined set
needs more than ~a dozen self-shims, or ANY symbol requires real engine/tier1 behavior beyond a
self-containable leaf (e.g. reachable `CBufferString::Insert`-class methods, a live `CUtlString` object
surgery), the approach is reconsidered — the fallback direction is sig-scanning the ENGINE's own compiled
`CEntityKeyValues` ctor/`SetKeyValue`/`Release` as gamedata functions (call the engine's copy instead of
compiling our own) — and the slice STOPS for a re-design rather than bloating the shim.

## Architecture

### 1. One new core op (extend the spawn path, not `entity_create`)

`entity_create`'s existing signature is frozen (ABI discipline: append-only, never modify), and the
keyvalues belong to **DispatchSpawn** anyway (that's the engine contract). A builder-style op family
(`ekv_create`/`ekv_set_*`/`ekv_release` returning an opaque handle) was rejected: it forces cross-call
handle ownership, a ledger entry, and a leak surface for a value that never needs to outlive one call. So:
**ONE op**, the whole EKV lifecycle inside a single shim call:

```c
/* ABI-APPENDED after the current last op (entity_fire_input). types[i]: 0=string 1=int 2=float 3=bool;
   values are the stringified forms ("1"/"0" for bool); the shim converts (strtol/strtof) and calls the
   matching typed Set*. Returns 1 ok / 0 fail. */
typedef int (*s2_entity_spawn_kv_fn)(int index, int serial, int count,
    const char* const* keys, const int* types, const char* const* values);
```

Kept identical across the C header (`shim/include/s2script_core.h`), the Rust mirror (`core/src/v8host.rs`),
BOTH in-isolate test op-structs, and the shim `ops.` assignment. The existing `entity_spawn` (kv-less) op is
untouched; the JS `spawn(kv?)` dispatches between the two natives. **Zero new signatures/offsets** — the op
reuses the already-resolved `s_pDispatchSpawn`; this slice adds NO new RE and no gamedata entries.

### 2. Shim — `ekv.cpp` (the single SDK-including TU) + `Shim_EntitySpawnKv`

- `shim/src/ekv.{h,cpp}`: `GameEntitySystem()` (bridge to `GetEntitySystem()` via a small non-static
  accessor added to `s2script_mm.cpp`); `void* S2EKV_Build(count, keys, types, values)` →
  `new CEntityKeyValues()` (default ctor = NULL arena + `EKV_ALLOCATOR_NORMAL`, the CSSharp shape; the
  arena lazily self-creates in `ValidateAllocator`, no entity system required — safe even pre-map) + a
  per-pair typed `Set*` switch; `S2EKV_AddRef`/`S2EKV_ReleaseIfSafe`; `S2EKV_SelfTest()`. The
  `CEntityKeyValues*` stays `void*` outside this TU — the header blast radius is one file, and the raw
  pointer never crosses to JS (it never even leaves the one op call).
- `Shim_EntitySpawnKv` (in `s2script_mm.cpp`, beside `Shim_EntitySpawn`): null-guard `s_pDispatchSpawn` +
  args → `ResolveEntityBySerial(index, serial)` (serial-gated, the existing chunk-walk) → `S2EKV_Build` →
  **AddRef → `s_pDispatchSpawn(ent, ekv)` → guarded Release** (see Lifecycle) → 1.

### 3. Lifecycle — who frees the `CEntityKeyValues`

`m_nRefCount` starts at **0**; `Release()` is `if (--m_nRefCount <= 0) delete this`. The engine may
AddRef/Release the object it's handed (and `m_nQueuedForSpawnCount` shows a queued-spawn path exists where
the entity system holds it past the call). Contract:

1. **`AddRef()` before `DispatchSpawn`** (refcount 1) — a balanced engine AddRef/Release pair can then never
   reach 0 and engine-delete our object mid-call (engine-side `delete` on our-heap memory = the cross-heap
   hazard).
2. **After `DispatchSpawn` returns:** if `!IsQueuedForSpawn()` → our `Release()` → 0 → OUR `delete` (same
   TU/heap as the `new` — always consistent). If the engine still holds it queued (not expected for the
   UTIL_CreateEntityByName-then-DispatchSpawn synchronous path, which the entity-creation slice proved live)
   → **deliberately leak** this one small object + WARN once — a bounded leak beats a UAF or a cross-heap
   free, and CSSharp appears to leak its EKVs unconditionally.

The engine reads our object cross-module through the identical (dota-SDK) struct layout — a
**treadmill-class risk** (the SDK is pinned/vendored/patch-capable; a Valve layout change shows up as
keyvalues silently not applying, caught by the live gate's read-back, while the boot self-test catches
link/ctor integrity every update).

### 4. `@s2script/entity` (prelude + types)

- `__s2_ekv_marshal(kv)` — the inference/validation above; returns `{keys, types, values}` or `null`.
- `EntityRef.prototype.spawn(keyvalues?)` — `undefined`/`null`/empty-object → the existing
  `__s2_entity_spawn`; else marshal (null → `false`) → `__s2_entity_spawn_kv`.
- `createEntity(className, keyvalues?)` — create; if kv given, `spawn(kv)`, removing + returning `null` on
  failure.
- No hardcoded class/field names anywhere in core — boundary-clean by construction.

## Degrade-never-crash

- Op unresolved / `s_pDispatchSpawn` null / stale serial / `S2EKV_Build` failure → `0`/`false`, never a
  crash; the native is `catch_unwind`-wrapped like every prior one; interior-NUL strings fail `CString::new`
  → `false`.
- A marshal rejection returns `false` BEFORE any engine call (no partial spawn).
- The queued-spawn edge degrades to a logged bounded leak (above), never a free-while-held.
- The load-time self-test failing logs loudly but disables nothing else (per-descriptor degrade: kv-spawns
  return `false`; plain `spawn()` still works).

## Boundary analysis (both CI gates stay green)

`CEntityKeyValues`/`KeyValues3`/`DispatchSpawn` are Source 2 engine types → the op, natives, and prelude are
engine-generic core; the SDK `.cpp`s compile into the SHIM (which already owns every SDK touchpoint). No CS2
schema class/field name enters `core/src` or `packages/entity` (keys are caller parameters). The demo plugin
names CS2 entities (`point_worldtext`, `math_counter`) — plugins are outside both gates (the entityio-demo
precedent). `check-core-boundary.sh` + `test-boundary-nameleak.sh` stay green.

## Testing

- **In-isolate (core):** (1) degrade — `spawn({health: 42})` → `false` with no op; `createEntity("x", {a: 1})`
  → `null`. (2) **marshal capture** — install a fake `entity_spawn_kv` op (the `EV_SUBSCRIBED` capture-buffer
  pattern; tests run serial) and assert `{name: "bob", health: 42, scale: 1.5, enabled: true}` crosses as
  keys/types/values `[0,1,2,3]` / `["bob","42","1.5","1"]`. (3) inference edges — int32-overflow integer →
  float tag; non-finite / bad value type / empty key → `false` with the op NEVER invoked; `{}`/omitted →
  the plain `entity_spawn` path.
- **Load-time self-test (shim, permanent):** construct + set + get round-trip at `Load()` →
  `[s2script] EKV self-test: OK` — catches link/layout drift on every update (treadmill).
- **Live gate — fully bot-provable, keyvalue-took-effect by READ-BACK + BEHAVIOR** (rcon `sm_ekv`):
  1. `createEntity("point_worldtext", { message: "s2-ekv-proof", enabled: true, fullbright: true })` →
     read back `m_messageText` (**char[512] @ live-resolved offset**, `EntityRef.readString`) `===
     "s2-ekv-proof"` (the STRING path parsed by the entity's own Spawn) + `m_bFullbright` reads `true`
     (the BOOL path).
  2. `createEntity("math_counter", { startvalue: 5, min: 1, max: 10 })` → read back `m_flMax === 10` /
     `m_flMin === 1` (float32 — also proving engine-side int→float KV3 coercion); then
     `Entity.onOutput("math_counter", "OnHitMax", …)` + `mc.acceptInput("Add", "5")` → **`OnHitMax` fires**
     (5 + 5 = the kv-configured max) — the INT path proven **behaviorally through the entity's own logic**,
     composed entirely from already-shipped primitives (entity-creation + entity-I/O).
  3. Both entities removed after ~3s; `RestartCount=0`, server ticking, no crash;
     `GAMEDATA VALIDATION` unchanged (no new entries).
  - Fallbacks if a keyvalue name doesn't take on this build: the two entities are independent proofs
    (either alone proves the mechanism); key names are FGD-standard and case-insensitive.

## Non-goals (YAGNI / deferred, do NOT build ahead)

- Typed setters beyond string/int/float/bool: `SetVector`/`SetColor`/`SetQAngle`/`SetEHandle`/
  `SetStringToken`/`SetPtr`/`SetUint64` (an explicit `{key: {type, value}}` escape-hatch form is the named
  follow-up if inference proves too coarse).
- Reading keyvalues back from a `CEntityKeyValues` (Get*/iteration); receiving engine-created EKVs (e.g. a
  spawn pre-hook exposing the map's kv) — a separate, detour-shaped slice.
- Attributes (`bAsAttribute`), `AddConnectionDesc` entity-I/O connections (`acceptInput("AddOutput", …)`
  already covers runtime wiring), `CopyFrom`.
- A persistent EKV builder handle / reusing one EKV across spawns.
- Compiling `entitysystem.cpp`/`entityidentity.cpp` (only if the spike forces a needed symbol).

## Sequencing — spike-first, one slice

0/1. **Compile-spike** (the front-loaded central risk) — SDK `.cpp`s + `ekv.cpp` + CMake + triage +
   self-shims → the dlopen + self-test exit criterion. STOP the slice if the cascade is unbounded.
2. **Core** — the `entity_spawn_kv` op (ABI-appended), native, prelude marshal/`spawn(kv?)`/`createEntity`
   overload, `.d.ts`, in-isolate tests.
3. **Shim wiring** — `Shim_EntitySpawnKv` + AddRef/Release lifecycle + `ops.` assignment; sniper build.
4. **Demo + live gate** — `plugins/ekv-demo` (`sm_ekv`), the read-back + behavioral bot gate.

Needs one sniper rebuild cycle (the spike build is the same rebuild, iterated). Related:
[[re-gamedata-strategy]] (zero new signatures here — the win of reusing `s_pDispatchSpawn`),
[[cs2-schema-entity-access]], the entity-creation slice (`2026-07-09-entity-creation-beam-design.md`), the
entity-I/O slice (`2026-07-09-entity-io-design.md`), `shim/src/tier1_shims.cpp` (the self-shim pattern),
[[cs2-update-metamod-treadmill]] (engine-export volatility → prefer self-shims for leaves).
