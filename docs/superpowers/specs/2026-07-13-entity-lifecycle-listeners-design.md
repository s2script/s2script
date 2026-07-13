# Entity lifecycle listeners — `Entity.onCreate` / `onSpawn` / `onDelete`

**Slice date:** 2026-07-13
**Branch / worktree:** `feat/entity-lifecycle-listeners` @ `/home/gkh/projects/s2script-entity-listeners`
**Status:** design approved — proceeding to plan.

## Goal

Let plugins **observe** the engine creating / spawning / deleting entities, delivering each entity
to JS as a serial-gated `EntityRef`. This closes the clearest remaining CSSharp/ModSharp parity
gap: we can already **create and manipulate** entities (`createEntity`/`spawn`/EKV/`teleport`/
`remove`, `findByClass`, entity I/O, the `Weapon` object) but have no way to **react** to the
engine's own entity lifecycle. Unlocks reactive plugins: weapon drops, grenade/projectile throws,
prop spawns, ragdolls, temp-entity effects, etc.

Reference parity: CSSharp `Listeners.OnEntitySpawned` / `OnEntityCreated` / `OnEntityDeleted`
(via `IEntityListener` on `CGameEntitySystem`); ModSharp's equivalent entity hooks.

## Non-goals (YAGNI — do NOT build ahead)

- **Vetoing** a create/spawn/delete. The `IEntityListener` callbacks return `void` — the interface
  *cannot* block. Blocking a spawn would require detouring a different path; out of scope.
- **`OnEntityParentChanged`** — the 4th `IEntityListener` slot. We implement it as a no-op override
  (vtable completeness) but do not expose it to JS this slice. Trivially addable later.
- A general (non-CS2) helper or per-class typed generics (`onSpawn<T>`). Class is a plain string.

## Mechanism landscape (verified against both references, most-recent versions)

CS2's Source-2 entity system exposes (vendored SDK, `third_party/hl2sdk/public/entity2/entitysystem.h`):

```cpp
class IEntityListener {
    virtual void OnEntityCreated(CEntityInstance* pEntity) {};        // slot 0
    virtual void OnEntitySpawned(CEntityInstance* pEntity) {};        // slot 1
    virtual void OnEntityDeleted(CEntityInstance* pEntity) {};        // slot 2
    virtual void OnEntityParentChanged(CEntityInstance*, CEntityInstance*) {};  // slot 3
};
class CGameEntitySystem : public CEntitySystem {
    void AddListenerEntity(IEntityListener* pListener);      // non-virtual; guards Find, AddToTail
    void RemoveListenerEntity(IEntityListener* pListener);   // non-virtual; FindAndRemove
    CUtlVector<IEntityListener*> m_entityListeners;
};
```

All three lifecycle events are **distinct, hookable vtable slots** — confirmed in the header.

The two references diverge on how they call `AddListenerEntity`:

- **CounterStrikeSharp v1.0.363** *compiles* `entity2/entitysystem.cpp` into its own binary
  (`CMakeLists.txt:33`) and calls `es->AddListenerEntity(&listener)` — its own compiled copy, using
  the **header's compile-time `m_entityListeners` offset** and the inline `CUtlVector::AddToTail`
  (allocator = `g_pMemAlloc`). (`src/mm_plugin.cpp:210`, `src/core/managers/entity_manager.{h,cpp}`.)
- **ModSharp git118** ships a re-validatable **byte signature** for
  `CGameEntitySystem::AddListenerEntity` *and* `RemoveListenerEntity`
  (`sharp/gamedata/server.games.jsonc:489,524`) — a full, distinctive, non-inlined prologue — and
  calls the resolved engine function directly.

### Decision: signature-scan `AddListenerEntity` (ModSharp route), not the header offset

Per the RE doctrine ([[re-gamedata-strategy]] / `docs/re-strategy.md`): **self-resolve against our
binary and load-validate; never a silent borrowed constant.** A byte-signature resolves into
`libserver.so`'s own `.text` and is validated **UNIQUE at load** by the existing GAMEDATA VALIDATION
gate → *loud* on drift. The header-offset route (CSSharp) is silent on drift — the exact failure mode
that stale-d the 5D.2 client-list offsets on 2000870. ModSharp's pattern is a **hint**: we borrow it,
re-resolve + re-validate UNIQUE against *our* pinned `libserver.so`, `.text`-range-guard before
calling — identical to how `GameEventManager` / `DispatchTraceAttack` / `HostSay` /
`LegacyGameEventListener` / `CommitSuicide` are already handled.

Two new `signatures` entries in `gamedata/core.gamedata.jsonc` (`resolve: "direct"`):
`AddListenerEntity`, `RemoveListenerEntity`. The GAMEDATA VALIDATION count bumps by 2.

## Architecture — the 4th notify-mux

This is the established pattern, now for the fourth time: a shim hook → a core notify-mux → JS
subscribers, mirroring `event_mux.rs` + `CLIENT_MUX` (`dispatch_client_event`) + `MAP_MUX`
(`dispatch_map_start`) + `OUTPUT_MUX` (`dispatch_output`). Notify-only, so it follows the
`dispatch_client_event` shape (void handler, no `HookResult` collapse) — **not** `dispatch_output`
(which collapses `HookResult`, because IO outputs can be suppressed and entity-lifecycle cannot).

### Shim (engine-generic — Source2 types only; NO CS2 identifiers)

- **`S2EntityListener`** — a static instance of a class deriving from the real `IEntityListener`
  (an isolated SDK-including TU, `shim/src/entity_listener.cpp`, mirroring `ekv.cpp`'s isolation
  discipline so the heavy header stays out of the normal TUs; inheriting the real interface
  guarantees the vtable layout matches the SDK rather than hand-asserting slot order). The three
  exposed overrides do, per fire:
  1. read the entity's cheap designer/class name (the `CEntityIdentity::m_designerName` /
     `GetClassname` path the shim already uses for `s2_entity_find_by_class` + the entity-I/O
     detour's caller-class),
  2. pack the handle: `pEntity->GetRefEHandle().ToInt()` (the shim's existing idiom),
  3. call the core FFI export `s2script_core_dispatch_entity_event(kind, className, handle)`.
  `OnEntityParentChanged` = no-op override (vtable completeness).
  *This TU does NOT instantiate `CUtlVector::AddToTail` — registration is a sig-call, so no
  `g_pMemAlloc`-grow / template-cascade concern (the engine grows its own vector inside the resolved
  `AddListenerEntity`).*
- **Registration:** resolve `AddListenerEntity`/`RemoveListenerEntity` via the two new gamedata
  signatures at Load (validated UNIQUE + `.text`-range-guarded, degrade-to-unavailable if
  missing/stale). Call `AddListenerEntity(es, &g_listener)` — the standard `(this, arg)`
  member-as-free-fn call the shim uses for `CommitSuicide`/`GiveNamedItem`/etc.
- **Lazy install + per-map re-assert:**
  - One new tail engine op `entity_listener_install()`. Core calls it on the **first-ever**
    entity subscribe. The shim: sets a persistent `s_wantEntityListener = true` and, if the entity
    system currently exists (`GetEntitySystem() != null`), calls `AddListenerEntity` now.
  - The shim's existing `StartupServer` POST hook (the `Server.onMapStart` mechanism) re-asserts:
    if `s_wantEntityListener`, call `AddListenerEntity` again each map. `AddListenerEntity` is
    idempotent (its `Find`-guard), so this is safe whether the entity system persists across a
    changelevel or is recreated with a fresh empty `m_entityListeners`.
  - `RemoveListenerEntity(es, &g_listener)` on shim `Unload` (avoids a dangling vtable call if
    s2script is unloaded while the entity system lives).
  - **Zero cost when unused:** with no subscriber the op is never called, so the `IEntityListener`
    is never registered → the engine never dispatches to us.

### Core (`core/src/v8host.rs` + `core/src/ffi.rs` — engine-generic, no CS2 identifiers)

- **`ENTITY_MUX: RefCell<EventMux<v8::Global<v8::Function>>>`** keyed `"kind\0className"`, the exact
  `OUTPUT_MUX` keying. `kind` ∈ `{"create","spawn","delete"}`.
- **`s2script_core_dispatch_entity_event(kind: *const c_char, className: *const c_char, handle: c_int)`**
  (ffi.rs) → `dispatch_entity_event(kind, className, handle)` (v8host.rs). Per fire:
  1. snapshot `"kind\0className"` + `"kind\0*"` (dedup); **return early if empty** — no `EntityRef`
     built, no JS entered (this is what makes a class-filtered subscriber near-free);
  2. `HOST.try_borrow_mut()` re-entrancy guard (a handler that synchronously `createEntity`/`remove`s
     re-enters this dispatch while the isolate is borrowed → skip gracefully, engine-side lifecycle
     still happens);
  3. per subscriber: `is_live(owner, generation)`, clone the context Global out of `PLUGINS`,
     per-subscriber `HandleScope` + `ContextScope` + `TryCatch` (WARN-on-throw), build a serial-gated
     `EntityRef` from the handle (`decode_handle` + `entity_resolve_ptr` null-check + `build_entity_ref`;
     if the slot is already stale → pass `null`), invoke `handler(entityRef)`. **Void return —
     notify-only.**
- **Natives** `__s2_entity_listener_on(kind, className, handler)` / `__s2_entity_listener_off(...)`:
  `on` calls `ENTITY_MUX.subscribe`; if it was the **first-ever** subscription (mux was empty),
  call the `entity_listener_install()` op. `off` uses `remove_by_owner_on`. On plugin unload,
  `remove_by_owner(owner)` (the existing per-mux teardown loop). Reset `ENTITY_MUX` on shutdown.
- **ABI:** one op appended at the current struct tail (after `entity_set_model` — the zones-real-trigger
  slice's last op) across the C header (`shim/include/s2script_core.h`), the Rust mirror (both
  `S2EngineOps` init sites in `v8host.rs`), and both in-isolate test op-structs.

### `@s2script/entity` module + `packages/entity/index.d.ts`

The runtime `Entity` object is assembled in the **core prelude JS string inside `core/src/v8host.rs`**
(`globalThis.__s2pkg_entity = { EntityRef, createEntity, Entity }`, alongside `createEntity` and the
`Entity.onOutput`/`findByClass` definitions) — engine-generic, **not** `games/cs2/js/pawn.js`. The
three new methods are added there next to `Entity.onOutput`; the `EntityRef` type is unchanged.

```ts
/**
 * Fire when the engine CREATES an entity of `className` ("*" = all). Earliest hook — the entity is
 * barely constructed; schema fields may be zero/default. Prefer onSpawn to read fields.
 */
Entity.onCreate(className: string, handler: (entity: EntityRef) => void): void;

/**
 * Fire after the engine SPAWNS an entity of `className` ("*" = all) — Spawn() has run, so schema
 * fields and keyvalues are populated. The useful hook for reading state.
 */
Entity.onSpawn(className: string, handler: (entity: EntityRef) => void): void;

/**
 * Fire as the engine DELETES an entity of `className` ("*" = all). The entity still exists during
 * the synchronous handler — read what you need NOW; a stashed ref reads null once the slot is freed
 * (serial gate), never garbage.
 */
Entity.onDelete(className: string, handler: (entity: EntityRef) => void): void;
```

`className` is required; pass `"*"` for a global listener. Handlers are `void` (notify-only). The
delivered `EntityRef` is the same serial-gated handle used everywhere; reads are `T | null`.

## Safety / edge cases

- **Serial gate:** the entity crosses only as a packed handle (index+serial); core rebuilds a
  serial-gated `EntityRef`. A ref stashed past the handler (esp. from `onDelete`) reads `null` once
  the slot is freed/reused — never a raw pointer, never garbage, never a crash. (Charter: no raw
  pointer or cross-time reference crosses to JS.)
- **`onCreate` timing:** the entity is allocated + its identity/handle assigned (so `GetRefEHandle`
  is valid) but not spawned; fields are default. Documented — steer readers to `onSpawn`.
- **`onDelete` timing:** fires during teardown; the entity is still readable synchronously. Documented.
- **Re-entrancy:** a handler that `createEntity`/`remove`s synchronously re-enters `dispatch_entity_event`
  while `HOST` is borrowed → `try_borrow_mut` skips the nested JS re-dispatch (engine-side lifecycle
  unaffected). Same trap + guard as `dispatch_output`/`Events.fire`.
- **Volume:** with ≥1 subscriber, every create/spawn/delete costs one FFI call + a className marshal
  + a HashMap lookup; JS is entered only for a matching class key. A map load bursts ~2000 entities
  (a one-time cost); per-round projectiles/effects are dozens. Acceptable. Zero cost with no
  subscriber (lazy install).
- **Degrade-never-crash:** a missing/stale signature → `AddListenerEntity` unresolved → the op is a
  no-op (subscribe silently delivers nothing), never a crash. A null/garbage handle → `null` ref.

## Boundary (both gates must stay green)

- Core/shim are **engine-generic**: `IEntityListener`/`CGameEntitySystem`/`CEntityInstance` are
  Source2 types (shim-only); the core `ENTITY_MUX` + `dispatch_entity_event` speak only `(kind,
  className, handle)` strings/ints. **NO CS2 identifiers in `core/src`.**
- `className` is an opaque string end-to-end — a CS2 class name is data, not a core identifier.
- The `@s2script/entity` module + `packages/entity` types are engine-generic (the module already
  owns `onOutput`/`findByClass`).

## Testing

**Core in-isolate (`cargo test`, single-threaded per `.cargo/config.toml`):**
- `ENTITY_MUX` keying + `"*"` wildcard snapshot (mirror the `OUTPUT_MUX` tests).
- `dispatch_entity_event` with no op / no subscriber degrades to a no-op (no panic).
- first-subscribe triggers the install op exactly once; teardown `remove_by_owner` empties the mux.
- (Handle→EntityRef build reuses the `dispatch_output`-tested path.)

**Live gate (Docker CS2, de_inferno, `bot_quota 2`, rcon):**
- Boot: GAMEDATA VALIDATION count is +2 (both `AddListenerEntity`/`RemoveListenerEntity` sigs resolve
  UNIQUE + `.text`-validated). No `[s2script]` errors.
- A demo plugin: `Entity.onSpawn("*", e => log(e.className))` (global) + an **exact-class** filtered
  sub, e.g. `Entity.onSpawn("logic_relay", …)` (matching is exact-className-or-`"*"` — no glob).
  Trigger: `createEntity("logic_relay")` + spawn (self-contained, mirrors `sm_iotest`), and/or give a
  bot a weapon (`pawn.giveNamedItem`) and observe `weapon_*` classes via the global sub. Expect: the
  create/spawn fires with a **valid `EntityRef`** + the correct `className`; `onDelete` fires on
  `remove()`.
- `RestartCount=0`, server ticking, no panic/abort.

**Human-client deferral (mechanism-proven, not e2e — the standing ceiling):** real bullet/grenade
gameplay spawns (bots don't fight); the mechanism is proven by the synthetic `createEntity`+spawn
demo and the weapon-give. Record in [[deferred-live-tests]] if applicable.

## Files touched (anticipated)

- `gamedata/core.gamedata.jsonc` — +2 signatures.
- `shim/include/s2script_core.h` — +1 op typedef + struct member (tail).
- `shim/src/entity_listener.cpp` (new, isolated SDK-including TU) — `S2EntityListener` + register/
  unregister; `shim/CMakeLists.txt` — add the TU.
- `shim/src/s2script_mm.cpp` — the `entity_listener_install` op impl, the `s_wantEntityListener`
  flag + `StartupServer` POST re-assert, `RemoveListenerEntity` on Unload, `ops.entity_listener_install`
  wiring, sig resolution.
- `core/src/ffi.rs` — `s2script_core_dispatch_entity_event` export.
- `core/src/v8host.rs` — `ENTITY_MUX`, `dispatch_entity_event`, `__s2_entity_listener_on/off`
  natives, op mirror (both init sites + test structs), unload/shutdown teardown, **and the
  `Entity.onCreate/onSpawn/onDelete` methods in the core prelude JS string** (next to
  `Entity.onOutput`; engine-generic — not `pawn.js`).
- `packages/entity/index.d.ts` — the three method signatures + doc.
- A demo plugin under `examples/` — `entity-listeners-demo`.
- Core in-isolate tests.

## Cost

One sniper rebuild (shim + core). One new tail op. Two gamedata signatures. `packages/entity`
`.d.ts` changes → a **minor changeset** (`@s2script/entity`) + a PR (per the slice cadence).
