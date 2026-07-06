/**
 * @s2script/entity — author-time type stubs for the injected entity API.
 * NO runtime code: the engine injects the implementation at load time.
 */

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
}
