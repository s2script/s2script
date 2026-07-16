/**
 * @s2script/admin — engine-generic admin flag model + cache API.
 *
 * Resolved at runtime via `globalThis.__s2pkg_admin`; no game-specific symbols.
 * Import: `import { ADMFLAG, Admin } from "./admin";`
 */

/** SourceMod-parity admin flag bitmask constants (SM bit values). */
export declare const ADMFLAG: {
  readonly RESERVATION: number;
  readonly GENERIC:     number;
  readonly KICK:        number;
  readonly BAN:         number;
  readonly UNBAN:       number;
  readonly SLAY:        number;
  readonly CHANGEMAP:   number;
  readonly CONVARS:     number;
  readonly CONFIG:      number;
  readonly CHAT:        number;
  readonly VOTE:        number;
  readonly PASSWORD:    number;
  readonly RCON:        number;
  readonly CHEATS:      number;
  readonly ROOT:        number;
};

/** An admin entry: their SteamID64, their combined flag mask, and a helper to check flags. */
export interface AdminInfo {
  readonly steamId: string;
  readonly flags:   number;
  /** Immunity level (0 = none). A lower-immunity admin cannot target a higher one. */
  readonly immunity: number;
  /** Names of the groups this admin belongs to. */
  readonly groups:  readonly string[];
  /** True if `required` flags are all set, OR if this admin has ROOT (root ⇒ all). */
  hasFlags(required: number): boolean;
}

/** A resolved admin group (from admin_groups.json): its flags, immunity, and per-command overrides. */
export interface AdminGroup {
  readonly name: string;
  readonly flags: number;
  readonly immunity: number;
  readonly overrides: Readonly<Record<string, { public: boolean; mask: number }>>;
}

/** The admin API: add/remove runtime admins, look up by SteamID or slot, reload from file. */
export declare const Admin: {
  /** Add (or overwrite) a runtime admin. Not persisted to admins.json. */
  add(steamId: string, flags: number, immunity?: number): void;
  /** Remove a runtime admin. */
  remove(steamId: string): void;
  /** Get an admin by SteamID64 (union of file + runtime tiers), or null if not an admin. */
  get(steamId: string): AdminInfo | null;
  /** Get an admin by player slot (resolves SteamID via engine), or null. */
  forSlot(slot: number): AdminInfo | null;
  /** True if `callerSlot` may act on `targetSlot` (console = infinite; blocked iff target immunity > caller). */
  canTarget(callerSlot: number, targetSlot: number): boolean;
  /** A resolved group by name (from admin_groups.json), or null. */
  getGroup(name: string): AdminGroup | null;
  /** Re-read admins.json into the file tier (clears old file-tier entries first). */
  reload(): void;
};
