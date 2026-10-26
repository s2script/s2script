/**
 * @s2script/sdk/stringtables — the engine's network string tables. NO runtime code: the engine
 * injects the implementation at load (`__s2pkg_stringtables`).
 *
 * String tables are how the server networks indexed strings to clients: precache lists, effect
 * names, and per-entry user data. The engine rebuilds them on every map change, so a
 * {@link StringTable} is plain data (its id and name) and every call looks the table up again. A
 * handle kept past a map change answers -1 / null / false rather than reaching a freed table; get
 * a fresh one with {@link StringTables.find}.
 */

/** A string table handle. Safe to keep: it holds no pointer. */
export interface StringTable {
  /** The table's id, its position in the container. */
  readonly id: number;
  /** The table's name, for example `"EffectDispatch"`. */
  readonly name: string;
  /** How many strings the table holds, or -1 when the handle is stale. */
  count(): number;
  /** The index of `value` in the table, or -1 when it is absent or the handle is stale. */
  findString(value: string): number;
  /** The string at `index`, or null for an index out of range or a stale handle. */
  getString(index: number): string | null;
  /**
   * A copy of the user data stored with the string at `index`. An entry without user data gives an
   * empty array; null means an index out of range or a stale handle.
   */
  getData(index: number): Uint8Array | null;
  /**
   * Replace the user data of the string at `index`. A string is stored as its UTF-8 bytes. Returns
   * false for an index out of range, a stale handle, or a value that is neither bytes nor a string.
   */
  setData(index: number, data: Uint8Array | string): boolean;
  /**
   * Add `value` to the table, with optional user data, and return its index. Adding a string the
   * table already has returns its existing index. Returns -1 when the table is full or the handle
   * is stale. The engine networks the new entry to clients.
   *
   * @example
   * const t = StringTables.find("EffectDispatch");
   * const i = t?.addString("MyPack.Effect") ?? -1;
   */
  addString(value: string, data?: Uint8Array | string): number;
}

/**
 * Find and list the server's string tables.
 *
 * @example
 * import { StringTables } from "@s2script/sdk/stringtables";
 * const effects = StringTables.find("EffectDispatch");
 * const particle = effects?.findString("ParticleEffect") ?? -1;
 */
export declare const StringTables: {
  /** The table with that name, or null. */
  find(name: string): StringTable | null;
  /** The table with that id, or null. */
  get(id: number): StringTable | null;
  /** How many tables the server has. 0 before the first map or when unavailable. */
  count(): number;
  /** Every table, in id order. */
  all(): StringTable[];
};
