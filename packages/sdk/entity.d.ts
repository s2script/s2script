/**
 * @s2script/entity — author-time type stubs for the injected entity API.
 * NO runtime code: the engine injects the implementation at load time.
 */

import type { Vector } from "./math";

/**
 * A host-liveness-gated handle to a live entity. `id` is a HOST-MINTED monotonic liveness
 * id — liveness is decided by the host's books (fed by engine create/delete notifications,
 * cleared at map transition), NEVER by reading the entity's own memory. Every access
 * re-resolves: books first, then identity-slot validation, instance last. A stale ref
 * degrades to null/false — including across a changelevel.
 *
 * The framework mints every `EntityRef`; plugin code never constructs one (the constructor
 * is intentionally not part of the public surface — a hand-built ref is the "raw ref across
 * time" footgun). Obtain refs from the engine (events, `findByClass`, `readHandle`, …).
 */
export declare class EntityRef {
  /** This entity's slot index in the game entity system. Paired with the host-minted
   *  {@link EntityRef.id} for slot validation (books first, then identity-slot match). */
  readonly index: number;
  /** The host-minted liveness id for this ref (books key). Not the raw engine serial. */
  readonly id: number;
  /** This entity's targetname (`CEntityIdentity::m_name`) — e.g. a map trigger's `"map_start"`. `""` if
   *  the entity has no targetname; `null` if the ref is stale/invalid. */
  readonly name: string | null;
  /** This entity's target (`CBaseEntity::m_target`) — the targetname of the entity it acts on, e.g. a
   *  `func_button`'s target entity. `""` if the entity has no target; `null` if the ref is stale/invalid. */
  readonly target: string | null;
  /** This entity's world-space position (`CGameSceneNode::m_vecAbsOrigin`, reached via
   *  `CBaseEntity::m_CBodyComponent` -> `CBodyComponent::m_pSceneNode`) — the same field CS2's
   *  `Pawn.origin` exposes for player pawns, readable here on any entity (e.g. ranking candidate
   *  `func_button`s by distance to a reference point). There is no meaningful "empty" position, so
   *  unlike {@link EntityRef.name}/{@link EntityRef.target}, `null` means exactly one thing: could not
   *  read (the ref is stale/invalid, or the field/chain didn't resolve) — never "no position". */
  readonly origin: Vector | null;
  /** @internal The host mints refs; this is not part of the public API surface. */
  private constructor();
  /** True iff the host's books say live AND the identity slot still matches. */
  isValid(): boolean;
  /** Read an i32 at `offset` bytes into the entity, or null if the ref is stale. */
  readInt32(offset: number): number | null;
  /** Write an i32 at `offset` bytes into the entity. Returns true on success, false if stale. */
  writeInt32(offset: number, value: number): boolean;
  /** Read an f32 at `offset` bytes into the entity, or null if the ref is stale. */
  readFloat32(offset: number): number | null;
  /** Write an f32 at `offset`. Returns true on success, false if stale. */
  writeFloat32(offset: number, value: number): boolean;
  /** Read a bool at `offset`, or null if the ref is stale. */
  readBool(offset: number): boolean | null;
  /** Write a bool at `offset`. Returns true on success, false if stale. */
  writeBool(offset: number, value: boolean): boolean;
  /** Read an i8 (sign-extended to number) at `offset`, or null if the ref is stale. */
  readInt8(offset: number): number | null;
  /** Read an i16 (sign-extended to number) at `offset`, or null if the ref is stale. */
  readInt16(offset: number): number | null;
  /** Read a u8 at `offset`, or null if the ref is stale. */
  readUInt8(offset: number): number | null;
  /** Read a u16 at `offset`, or null if the ref is stale. */
  readUInt16(offset: number): number | null;
  /** Read a u32 at `offset`, or null if the ref is stale. */
  readUInt32(offset: number): number | null;
  /** Write an i8 (truncated) at `offset`. Returns true on success, false if stale. */
  writeInt8(offset: number, value: number): boolean;
  /** Write an i16 (truncated) at `offset`. Returns true on success, false if stale. */
  writeInt16(offset: number, value: number): boolean;
  /** Write a u8 (truncated) at `offset`. Returns true on success, false if stale. */
  writeUInt8(offset: number, value: number): boolean;
  /** Write a u16 (truncated) at `offset`. Returns true on success, false if stale. */
  writeUInt16(offset: number, value: number): boolean;
  /** Write a u32 at `offset`. Returns true on success, false if stale. */
  writeUInt32(offset: number, value: number): boolean;
  /** Read a u64 at `offset` as a BigInt, or null if the ref is stale. */
  readUInt64(offset: number): bigint | null;
  /** Read an i64 at `offset` as a BigInt, or null if the ref is stale. */
  readInt64(offset: number): bigint | null;
  /** Read an f64 at `offset`, or null if the ref is stale. */
  readFloat64(offset: number): number | null;
  /** Read a NUL-terminated string (up to `maxLen` bytes) at `offset`, or null if the ref is stale. */
  readString(offset: number, maxLen: number): string | null;
  /** Write a bounded, NUL-terminated string into an inline `char[maxLen]` field at `offset` (truncated to
   *  `maxLen-1` bytes + always NUL-terminated). Returns true on success, false if the ref is stale. */
  writeString(offset: number, maxLen: number, value: string): boolean;
  /** Read `count` (1..4) contiguous float32s at `offset` into a number[], or null if the ref is stale. */
  readFloats(offset: number, count: number): number[] | null;
  /** Follow a chain of pointer derefs (each an offset into the current target), then read `count` (1..4) floats
   *  at `finalOff` into a number[]. All in-core (raw pointers never cross); null if the root is stale or any hop
   *  is null. */
  readFloatsChain(ptrOffs: number[], finalOff: number, count: number): number[] | null;
  /** Follow a pointer chain (each an offset), then read a scalar at `finalOff`. null if the root is stale or any
   *  hop is null. `readHandleVia` decodes a handle field → a liveness-gated EntityRef; vectors use readFloatsChain. */
  readInt32Via(pathOffs: number[], finalOff: number): number | null;
  /** Write an int32 at the end of a pointer chain (each hop deref'd, liveness-gated at the root). Returns
   *  false on a stale ref or a null hop. Used to clear a flag on a pointer-referenced sub-object. */
  writeInt32Via(pathOffs: number[], finalOff: number, value: number): boolean;
  /** Read an i8 (sign-extended) at the end of a pointer chain (each hop deref'd, liveness-gated at the root); null if the root is stale or any hop is null. */
  readInt8Via(pathOffs: number[], finalOff: number): number | null;
  /** Read an i16 (sign-extended) through a pointer chain; null if the root is stale or any hop is null. */
  readInt16Via(pathOffs: number[], finalOff: number): number | null;
  /** Read a u8 through a pointer chain; null if the root is stale or any hop is null. */
  readUInt8Via(pathOffs: number[], finalOff: number): number | null;
  /** Read a u16 through a pointer chain; null if the root is stale or any hop is null. */
  readUInt16Via(pathOffs: number[], finalOff: number): number | null;
  /** Read a u32 through a pointer chain; null if the root is stale or any hop is null. */
  readUInt32Via(pathOffs: number[], finalOff: number): number | null;
  /** Read an f32 through a pointer chain; null if the root is stale or any hop is null. */
  readFloat32Via(pathOffs: number[], finalOff: number): number | null;
  /** Read a bool through a pointer chain; null if the root is stale or any hop is null. */
  readBoolVia(pathOffs: number[], finalOff: number): boolean | null;
  /** Read a u64 as a BigInt through a pointer chain; null if the root is stale or any hop is null. */
  readUInt64Via(pathOffs: number[], finalOff: number): bigint | null;
  /** Read an i64 as a BigInt through a pointer chain; null if the root is stale or any hop is null. */
  readInt64Via(pathOffs: number[], finalOff: number): bigint | null;
  /** Decode a `CEntityHandle` at the end of a pointer chain into a liveness-gated {@link EntityRef}; null if the root is stale, any hop is null, or the handle is dead. */
  readHandleVia(pathOffs: number[], finalOff: number): EntityRef | null;
  /** Write a scalar through a pointer chain (write mirror of `read*Via`). Liveness-gated at the root;
   *  returns false on a stale ref, an unresolved hop, or a bad offset/kind. Does NOT notifyStateChanged —
   *  the caller decides (many sub-object fields, e.g. the fire gate, are server-authoritative).
   *  (`writeInt32Via` is declared above alongside `readInt32Via`.) */
  writeFloat32Via(pathOffs: number[], finalOff: number, value: number): boolean;
  /** Write a bool through a pointer chain (write mirror of {@link EntityRef.readBoolVia}; shares the
   *  no-notify semantics on {@link EntityRef.writeFloat32Via}). False on a stale ref, an unresolved hop, or a bad offset/kind. */
  writeBoolVia(pathOffs: number[], finalOff: number, value: boolean): boolean;
  /** Read a `CEntityHandle` at `offset`, decode it, and return a live `EntityRef` — or null if stale/invalid. */
  readHandle(offset: number): EntityRef | null;
  /** Notify the engine that the field at `offset` changed (triggers network replication). No-op if stale. */
  notifyStateChanged(offset: number): void;
  /** Raw identity-slot flags (engine m_flags), or null when stale/unavailable. Bit meanings are game-specific. */
  identityFlags(): number | null;
  /**
   * CLEAR identity-slot flag bits; returns the flags after the write, or null when stale.
   *
   * Clear-only by design: `mask` names bits to DROP and nothing can be raised, because handing a
   * plugin the ability to set arbitrary identity bits is a far larger foot-gun than the one this
   * closes. The invalid-ehandle bit is refused outright — a dead slot must never be presentable as
   * live.
   *
   * The case this exists for is the staging bit. `setModel` asserts that an entity is NOT in the
   * staging list, and a created-but-unspawned entity is, so the create -> setModel -> spawn ordering
   * (what CS2's own body spawner does) is otherwise unavailable and the alternatives produce
   * half-initialised entities that clients fail to copy.
   */
  clearIdentityFlags(mask: number): number | null;
  /** DispatchSpawn this created entity. With keyvalues, the entity's own Spawn() parses them (the
   *  SourceMod DispatchKeyValue / CSSharp DispatchSpawn(kv) mechanism). Returns false if stale,
   *  unresolved, or the keyvalue map is rejected (non-finite number, unsupported value type, empty
   *  key) — a rejection spawns NOTHING (never a partially-configured entity). */
  spawn(keyvalues?: EntityKeyValueMap): boolean;
  /** Teleport this entity. origin/angles/velocity are [x,y,z] triples; any may be null. False if stale. */
  teleport(origin: number[] | null, angles?: number[] | null, velocity?: number[] | null): boolean;
  /** Remove (UTIL_Remove) this entity from the world. Returns false if stale/unresolved. */
  remove(): boolean;
  /** Register this entity's collision bounds in the spatial partition (zones real-trigger backend).
   *  A runtime-created trigger_multiple needs this to fire touch; false if the op is unavailable. */
  activateCollision(): boolean;
  /** Give this entity a model (and its collision) via `CBaseEntity::SetModel`. A runtime
   *  `trigger_multiple` needs a model to build the physics volume that fires touch. Returns false
   *  if the op is unavailable or the ref is stale. */
  setModel(name: string): boolean;
  /** Set this entity's gravity multiplier (`CBaseEntity::SetGravityScale`). 1 is normal, 0 is
   *  weightless.
   *
   *  Use this rather than writing `m_flGravityScale`: the engine setter early-returns when the value
   *  is unchanged and maintains a second field (`m_flActualGravityScale`), so a raw field write
   *  appears to succeed and does nothing. Returns false if the op is unavailable, the ref is stale,
   *  or `scale` is not finite. */
  setGravityScale(scale: number): boolean;
  /** Add to this entity's velocity, physics-aware (`CBaseEntity::ApplyAbsVelocityImpulse`) — for
   *  knockback, boosts, explosions.
   *
   *  Additive, unlike `teleport(null, null, velocity)` which sets velocity absolutely; and unlike a
   *  direct `m_vecAbsVelocity` write, which skips the partition/physics update. `impulse` is an
   *  [x,y,z] triple. A zero impulse is a legal no-op. Returns false if the op is unavailable, the ref
   *  is stale, or any component is not finite. */
  applyAbsVelocityImpulse(impulse: number[]): boolean;
  /** Stop a sound playing on this entity (`CBaseEntity::StopSound`) — the counterpart to
   *  `Sound.emit`. Returns false if the op is unavailable or the ref is stale. */
  stopSound(name: string): boolean;
  /** Set a model body group by name (`CBaseModelEntity::SetBodyGroupByName`).
   *
   *  There is no schema equivalent — `m_bodyGroupChoices` is a `CUtlOrderedMap`, not a writable
   *  scalar. `group` is 32-bit engine-side. Returns false if the op is unavailable, the ref is stale,
   *  or `group` is out of 32-bit range. */
  setBodyGroupByName(name: string, group: number): boolean;
  /** Set this entity's model scale (`CBaseModelEntity::SetModelScale`).
   *
   *  The argument shape is confirmed against the pinned build; the function's NAME is a catalogue
   *  attribution its body does not itself prove, so verify the effect before relying on this in a
   *  shipped plugin — see the gamedata comment. Calling it is safe either way. Returns false if the
   *  op is unavailable, the ref is stale, or `scale` is not finite. */
  setModelScale(scale: number): boolean;
  /** Read a CUtlVector<CHandle> at (ptrOffs chain -> vectorOff) as live liveness-gated EntityRefs.
   *  Follows the pointer chain, reads count@+0 / elements@+8, caps at maxCount. [] if stale/unresolved. */
  readHandleVector(ptrOffs: number[], vectorOff: number, maxCount?: number): EntityRef[];
  /** Fire an entity input (e.g. "Kill"/"Ignite"/"SetHealth"/"Enable"/"Open"/"FireUser1"/"AddOutput")
   *  via `AddEntityIOEvent` — the game's own input-firing path (map I/O and `FireOutputInternal` route
   *  through it). `value` is the input's string argument (Source parses it per the input's field type;
   *  omit for a value-less input). `activator`/`caller` are optional entities threaded through to any
   *  output the input triggers. `delay` queues the event on the engine's same-tick I/O pump (0 = fires
   *  this same tick — NOT synchronous-within-the-call). Returns false with no op / a stale ref. */
  acceptInput(input: string, value?: string, activator?: EntityRef, caller?: EntityRef, delay?: number): boolean;
}

/** Keyvalues for a CEntityKeyValues-configured spawn. Inference: string -> SetString,
 *  boolean -> SetBool, integer (int32) -> SetInt, other finite number -> SetFloat.
 *  Keys are case-insensitive (hashed via MurmurHash2LowerCase, the engine's own keying). */
export type EntityKeyValueMap = { [key: string]: string | number | boolean };

/** Create a new entity by class name. WITHOUT keyvalues: create only — set fields, then call
 *  `.spawn()`. WITH keyvalues: create + DispatchSpawn(keyvalues) in one call — a non-null result is
 *  a LIVE, SPAWNED entity (on spawn failure the entity is removed and null returned). The created
 *  entity is game-world-owned (NOT auto-removed on plugin unload) — the plugin owns cleanup.
 *  @example
 *  import { createEntity } from "@s2script/sdk/entity";
 *  const wt = createEntity("point_worldtext", { message: "s2-ekv-proof", fullbright: true });
 *  if (wt) wt.remove();
 */
export declare function createEntity(className: string, keyvalues?: EntityKeyValueMap): EntityRef | null;

/** The payload delivered to an `Entity.onOutput` handler. */
export interface OutputEvent {
  /** The output's name (e.g. "OnTrigger", "OnPressed", "OnStartTouch"). */
  output: string;
  /** The entity that activated the chain leading to this output firing, or null. */
  activator: EntityRef | null;
  /** The entity that owns/fired this output (the `this` of `FireOutputInternal`), or null. */
  caller: EntityRef | null;
  /** The output's value, formatted as a string (MVP — typed `CVariant` marshalling is deferred). */
  value: string;
  /** The output's fire delay in seconds (0 = same-tick). */
  delay: number;
}

/**
 * Entry point for entity lookup by class name.
 * @example
 * import { Entity } from "@s2script/sdk/entity";
 * const triggers = Entity.findByClass("trigger_multiple");
 * for (const t of triggers) console.log(t.index, t.name);
 */
export declare const Entity: {
  /** Find every entity whose designer-name (class) exactly matches `className`. Returns liveness-gated refs. */
  findByClass(className: string): EntityRef[];
};
