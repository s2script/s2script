/**
 * @s2script/entity — author-time type stubs for the injected entity API.
 * NO runtime code: the engine injects the implementation at load time.
 */
import type { HookResultValue } from "./events";

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
  readonly index: number;
  /** The host-minted liveness id for this ref (books key). Not the raw engine serial. */
  readonly id: number;
  /** This entity's targetname (`CEntityIdentity::m_name`) — e.g. a map trigger's `"map_start"`. `""` if
   *  the entity has no targetname; `null` if the ref is stale/invalid. */
  readonly name: string | null;
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
  readInt8Via(pathOffs: number[], finalOff: number): number | null;
  readInt16Via(pathOffs: number[], finalOff: number): number | null;
  readUInt8Via(pathOffs: number[], finalOff: number): number | null;
  readUInt16Via(pathOffs: number[], finalOff: number): number | null;
  readUInt32Via(pathOffs: number[], finalOff: number): number | null;
  readFloat32Via(pathOffs: number[], finalOff: number): number | null;
  readBoolVia(pathOffs: number[], finalOff: number): boolean | null;
  readUInt64Via(pathOffs: number[], finalOff: number): bigint | null;
  readInt64Via(pathOffs: number[], finalOff: number): bigint | null;
  readHandleVia(pathOffs: number[], finalOff: number): EntityRef | null;
  /** Write a scalar through a pointer chain (write mirror of `read*Via`). Liveness-gated at the root;
   *  returns false on a stale ref, an unresolved hop, or a bad offset/kind. Does NOT notifyStateChanged —
   *  the caller decides (many sub-object fields, e.g. the fire gate, are server-authoritative).
   *  (`writeInt32Via` is declared above alongside `readInt32Via`.) */
  writeFloat32Via(pathOffs: number[], finalOff: number, value: number): boolean;
  writeBoolVia(pathOffs: number[], finalOff: number, value: boolean): boolean;
  /** Read a `CEntityHandle` at `offset`, decode it, and return a live `EntityRef` — or null if stale/invalid. */
  readHandle(offset: number): EntityRef | null;
  /** Notify the engine that the field at `offset` changed (triggers network replication). No-op if stale. */
  notifyStateChanged(offset: number): void;
  /** Raw identity-slot flags (engine m_flags), or null when stale/unavailable. Bit meanings are game-specific. */
  identityFlags(): number | null;
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
 *  entity is game-world-owned (NOT auto-removed on plugin unload) — the plugin owns cleanup. */
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
 * Hook Source 2 entity outputs (`func_button`→`OnPressed`, `trigger_multiple`→`OnStartTouch`,
 * `logic_relay`→`OnTrigger`, …) via a `FireOutputInternal` detour. `classname`/`output` accept `"*"`
 * wildcards. The handler runs SYNCHRONOUSLY (may block): returning a `HookResultValue >= Handled`
 * (2/3) SUPPRESSES the output — the original `FireOutputInternal` call is skipped.
 */
export declare const Entity: {
  /** @deprecated moved to ctx.entities.onOutput (L1 lifecycle v2) — removed after the port fan-out */
  onOutput(classname: string, output: string, handler: (ev: OutputEvent) => HookResultValue | void): void;
  /**
   * Fire when the engine CREATES an entity of `className` (`"*"` = all) — earliest hook; the entity is
   * barely constructed, schema fields may be zero/default. The handler receives the liveness-gated
   * `entity` (may be `null`) plus its `className`. Prefer `onSpawn` to read fields.
   *
   * @deprecated moved to ctx.entities.onCreate (L1 lifecycle v2) — removed after the port fan-out
   */
  onCreate(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /**
   * Fire after the engine SPAWNS an entity of `className` (`"*"` = all) — `Spawn()` has run, so schema
   * fields/keyvalues are populated. The useful hook for reading state.
   *
   * @deprecated moved to ctx.entities.onSpawn (L1 lifecycle v2) — removed after the port fan-out
   */
  onSpawn(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /**
   * Fire as the engine DELETES an entity of `className` (`"*"` = all). The entity is still readable
   * during the synchronous handler; a stashed ref reads `null` once the slot is freed (liveness gate),
   * never garbage.
   *
   * @deprecated moved to ctx.entities.onDelete (L1 lifecycle v2) — removed after the port fan-out
   */
  onDelete(className: string, handler: (entity: EntityRef | null, className: string) => void): void;
  /** Find every entity whose designer-name (class) exactly matches `className`. Returns liveness-gated refs. */
  findByClass(className: string): EntityRef[];
};
