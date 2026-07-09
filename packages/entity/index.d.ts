/**
 * @s2script/entity — author-time type stubs for the injected entity API.
 * NO runtime code: the engine injects the implementation at load time.
 */
import type { HookResultValue } from "@s2script/events";

/**
 * A serial-gated handle to a live entity. Wraps the `__s2_ent_ref_*` natives; the raw
 * entity pointer never crosses to JS. All accessors degrade safely (return null/false)
 * when the entity slot has been reused or the ops table is absent.
 */
export declare class EntityRef {
  readonly index: number;
  readonly serial: number;
  constructor(index: number, serial: number);
  /** True iff the slot's current serial still matches the captured serial. */
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
   *  hop is null. `readHandleVia` decodes a handle field → a serial-gated EntityRef; vectors use readFloatsChain. */
  readInt32Via(pathOffs: number[], finalOff: number): number | null;
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
  /** Read a `CEntityHandle` at `offset`, decode it, and return a live `EntityRef` — or null if stale/invalid. */
  readHandle(offset: number): EntityRef | null;
  /** Notify the engine that the field at `offset` changed (triggers network replication). No-op if stale. */
  notifyStateChanged(offset: number): void;
  /** DispatchSpawn this created entity (register/activate it). Returns false if stale/unresolved. */
  spawn(): boolean;
  /** Teleport this entity. origin/angles/velocity are [x,y,z] triples; any may be null. False if stale. */
  teleport(origin: number[] | null, angles?: number[] | null, velocity?: number[] | null): boolean;
  /** Remove (UTIL_Remove) this entity from the world. Returns false if stale/unresolved. */
  remove(): boolean;
  /** Read a CUtlVector<CHandle> at (ptrOffs chain -> vectorOff) as live serial-gated EntityRefs.
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

/** Create a new entity by class name (e.g. "env_beam"). Returns a serial-gated EntityRef, or null on
 *  failure. Call `.spawn()` after setting fields to register it. The created entity is game-world-owned
 *  (NOT auto-removed on plugin unload) — the plugin owns cleanup via `.remove()`. */
export declare function createEntity(className: string): EntityRef | null;

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
  onOutput(classname: string, output: string, handler: (ev: OutputEvent) => HookResultValue | void): void;
};
