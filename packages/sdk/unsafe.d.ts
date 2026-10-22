/**
 * @s2script/sdk/unsafe — plugin-declared engine calls and inbound hooks. NO runtime code: the
 * engine injects the implementation at load (`__s2pkg_unsafe`).
 *
 * UNSAFE by design. A declared call reaches a real engine function; a declared hook patches
 * that function's prologue. The framework validates every descriptor at load and degrades a
 * failure to `null`, but it cannot prove your signature or vtable index names the function you
 * meant. `s2s build` records `engine:calls` / `engine:hooks` in the manifest from your gamedata so
 * `s2s inspect` and the registry can show what the plugin reaches into; nothing to declare by hand
 * (see docs/superpowers/specs/2026-07-24-plugin-gamedata-design.md and ARCHITECTURE.md §2.0.7).
 */

import type { EntityRef } from "./entity";

declare const ptrBrand: unique symbol;

/**
 * A raw address in the server process: what an engine call with a `ptr` return gives you, what
 * {@link Ptr.from} makes from an entity, and what a `ptr` argument takes.
 *
 * UNSAFE. Nothing tracks what a `Ptr` points at. It answers typed reads and writes at a byte
 * offset, follows pointer fields with {@link Ptr.deref}, and can be handed to any generated
 * struct view, but a stale or wrong address reads garbage or crashes the server exactly as it
 * would in a Metamod plugin. Only addresses that can never be valid (the null page, anything
 * outside the user half of the address space) are refused, with an `S2Error` whose `code` is
 * `"bad_address"`. Prefer entity handles and their schema fields, which are checked on every
 * access; reach for `Ptr` when an engine structure has no other door.
 *
 * Do not keep a `Ptr` past the call or event that produced it unless you know the lifetime of the
 * memory behind it.
 *
 * @example
 * import { Ptr } from "@s2script/sdk/unsafe";
 * const p = Ptr.from(pawn);              // the entity's address, or null when it is stale
 * if (p) console.log(p.readInt32(healthOffset));
 */
export declare class Ptr {
  private constructor();
  private readonly [ptrBrand]: true;
  /** The null pointer. Reads through it throw. */
  static readonly NULL: Ptr;
  /** A `Ptr` for an address, or an entity's current address (null once the entity is gone). */
  static from(address: number | Ptr): Ptr;
  static from(entity: EntityRef): Ptr | null;
  /** The address as a number. Exact: every user-space address is below 2^47. */
  readonly address: number;
  isNull(): boolean;
  /** This address plus `bytes`. */
  add(bytes: number): Ptr;
  equals(other: Ptr): boolean;
  /** `0x…` */
  toString(): string;
  /** Hand this address to a view factory, such as a game package's struct view. */
  as<T>(view: (ptr: Ptr) => T): T;
  /** The pointer stored at `offset` (a pointer field), as a `Ptr`. `Ptr.NULL` when it holds null. */
  deref(offset?: number): Ptr;
  readPtr(offset: number): Ptr;
  writePtr(offset: number, value: Ptr): true;
  readInt8(offset: number): number;
  readUInt8(offset: number): number;
  readInt16(offset: number): number;
  readUInt16(offset: number): number;
  readInt32(offset: number): number;
  readUInt32(offset: number): number;
  readFloat32(offset: number): number;
  readFloat64(offset: number): number;
  readBool(offset: number): boolean;
  /** A 64-bit integer, as a decimal string (a JS number cannot hold every value). */
  readInt64(offset: number): string;
  readUInt64(offset: number): string;
  writeInt8(offset: number, value: number): true;
  writeUInt8(offset: number, value: number): true;
  writeInt16(offset: number, value: number): true;
  writeUInt16(offset: number, value: number): true;
  writeInt32(offset: number, value: number): true;
  writeUInt32(offset: number, value: number): true;
  writeFloat32(offset: number, value: number): true;
  writeFloat64(offset: number, value: number): true;
  writeBool(offset: number, value: boolean): true;
  /** The NUL-terminated string at `offset`, reading at most `maxLen` bytes. */
  readString(offset: number, maxLen: number): string;
  /** Write `value` into a `maxLen`-byte buffer, truncated so it always ends in a NUL. */
  writeString(offset: number, maxLen: number, value: string): boolean;
  /** `count` consecutive 32-bit floats (a vector, an angle). */
  readFloats(offset: number, count: number): number[];
  /** A `CEntityHandle` field, resolved through the entity books: null when it names no live entity. */
  readHandle(offset: number): EntityRef | null;
}

/**
 * The calls this plugin declares. The base interface is EMPTY — `s2s build` generates
 * `.s2script/gamedata.d.ts`, which augments this from your gamedata's `calls` section.
 */
export interface EngineCalls {}

/**
 * The inbound hooks this plugin declares. The base interface is EMPTY — `s2s build` generates
 * `.s2script/hooks.d.ts`, which augments this from your gamedata's `hooks` section.
 */
export interface EngineHooks {}

/** Generated bindings augment this map from gamedata/functions.jsonc. */
export interface EngineFunctions {}

export interface FunctionStatus {
  readonly canonicalId: string;
  readonly availability: 'available' | 'unavailable';
  readonly reason: string | null;
  readonly hookObservation: 'not-requested' | 'pending' | 'active' | 'failed';
  readonly provenance: {
    readonly archiveHash: string;
    readonly baseContractHash: string;
    readonly appliedOverrides: readonly { readonly path: string; readonly sha256: string }[];
    readonly finalTargetHash: string;
    readonly resolverReceipt: string;
    readonly required: boolean;
  };
}

export interface Disposable { dispose(): boolean; }

export interface FunctionSubscription extends Disposable {
  readonly status: 'pending' | 'active' | 'failed' | 'disposed';
  readonly reason: string | null;
  dispose(): boolean;
}

/**
 * Plugin-declared engine calls and inbound hooks, resolved from this plugin's own gamedata.
 *
 * @example
 * import { Engine } from "@s2script/sdk/unsafe";
 * // Resolve ONCE at load — null means the descriptor failed a load-time gate.
 * const ignite = Engine.call("ignite");
 * if (!ignite) console.log(`unavailable: ${Engine.status("ignite")}`);
 * const onFoo = Engine.hook("onFoo");
 * if (onFoo) onFoo((view) => { view.reason = 0; });
 */
export declare const Engine: {
  function<K extends keyof EngineFunctions>(name: K): EngineFunctions[K];
  /**
   * The declared call, or `null` when its descriptor failed a load-time gate (signature miss,
   * validator rejection, slot outside `.text`, missing platform entry, or the plugin is not
   * operator-allow-listed). Guard once at load; the returned function is a plain callable.
   */
  call<K extends keyof EngineCalls>(name: K): EngineCalls[K] | null;
  /** Why a call descriptor is unavailable; `"available"` when it resolved. For diagnostics/operator reports. */
  status(name: string): string;
  /**
   * The declared hook's subscribe function, or `null` when its descriptor failed a load-time
   * gate (same class of reasons as {@link Engine.call}, plus a missing `validate` or an unknown
   * thunk shape). Guard once at load; the returned function records the handler and lazily
   * installs the detour. The owner is always the calling plugin — you cannot subscribe to
   * another plugin's hook through this factory (game-package hooks are the `@s2script/cs2` exports such as `players` and `gameRules`).
   */
  hook<K extends keyof EngineHooks>(name: K): EngineHooks[K] | null;
  /** Why a hook descriptor is unavailable; `"available"` when it resolved. */
  hookStatus(name: string): string;
};
