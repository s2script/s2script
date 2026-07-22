/**
 * @s2script/admin — engine-generic admin flag model + cache API.
 *
 * Resolved at runtime via `globalThis.__s2pkg_admin`; no game-specific symbols.
 * Import: `import { ADMFLAG, Admin } from "./admin";`
 */

/** SourceMod-parity admin flag bitmask constants (SM bit values). */
export declare const ADMFLAG: {
  /** Reserved-slot access (`1<<0` = 1). */
  readonly RESERVATION: number;
  /** Generic admin — the baseline "is an admin" flag (`1<<1` = 2). */
  readonly GENERIC:     number;
  /** May kick players (`1<<2` = 4). */
  readonly KICK:        number;
  /** May ban players (`1<<3` = 8). */
  readonly BAN:         number;
  /** May remove bans (`1<<4` = 16). */
  readonly UNBAN:       number;
  /** May slay/slap players (`1<<5` = 32). */
  readonly SLAY:        number;
  /** May change the map (`1<<6` = 64). */
  readonly CHANGEMAP:   number;
  /** May change most convars (`1<<7` = 128). */
  readonly CONVARS:     number;
  /** May execute config files (`1<<8` = 256). */
  readonly CONFIG:      number;
  /** Admin-chat and other special chat privileges (`1<<9` = 512). */
  readonly CHAT:        number;
  /** May start votes (`1<<10` = 1024). */
  readonly VOTE:        number;
  /** May set the server password (`1<<11` = 2048). */
  readonly PASSWORD:    number;
  /** May use rcon commands (`1<<12` = 4096). */
  readonly RCON:        number;
  /** May change cheat convars and use cheat commands (`1<<13` = 8192). */
  readonly CHEATS:      number;
  /** Full access — implicitly satisfies every flag (`1<<14` = 16384). See {@link AdminInfo.hasFlags}. */
  readonly ROOT:        number;
};

/** An admin entry: their SteamID64, their combined flag mask, and a helper to check flags. */
export interface AdminInfo {
  /** This admin's SteamID64 (decimal string). */
  readonly steamId: string;
  /** Combined flag mask (union of file + runtime tiers and group flags) — test against {@link ADMFLAG} bits. */
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
  /** The group's name (its key in admin_groups.json). */
  readonly name: string;
  /** Flag mask granted to every member — {@link ADMFLAG} bits. */
  readonly flags: number;
  /** Immunity level conferred by membership (0 = none). */
  readonly immunity: number;
  /** Per-command access overrides, keyed by command name: `public` opens it to all, `mask` is the required flags. */
  readonly overrides: Readonly<Record<string, { public: boolean; mask: number }>>;
}

/**
 * The admin API: add/remove runtime admins, look up by SteamID or slot, reload from file.
 * @example
 * import { Admin, ADMFLAG } from "@s2script/sdk/admin";
 * const a = Admin.forSlot(p.slot);
 * const adminStr = a ? "yes(flags=0x" + a.flags.toString(16) + ")" : "no";
 */
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
