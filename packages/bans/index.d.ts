/**
 * @s2script/bans — engine-generic SteamID64 ban store + bans.json persistence.
 *
 * Resolved at runtime via `globalThis.__s2pkg_bans`; no game-specific symbols.
 * Import: `import { Bans } from "@s2script/bans";`
 *
 * The store is host-global in core (its reader is the C++ ClientConnect hook, not a JS context),
 * populated from `addons/s2script/configs/bans.json` via the config bridge. A banned SteamID64 is
 * rejected at connect time by the shim.
 */

/** A ban entry's value: expiry + reason. `until === 0` = permanent, else unix-second expiry. */
export interface BanInfo {
  /** Unix-second expiry, or 0 for a permanent ban. */
  until: number;
  /** The ban reason (may be empty). */
  reason: string;
}

/** A ban entry as listed by `Bans.list()`: the SteamID64 plus its value. */
export interface BanEntry {
  /** The banned SteamID64 (decimal string). */
  steamid: string;
  /** Unix-second expiry, or 0 for a permanent ban. */
  until: number;
  /** The ban reason (may be empty). */
  reason: string;
}

/** The ban API: add/remove bans, look up by SteamID64, list, and reload from bans.json. */
export declare const Bans: {
  /** Add (or overwrite) a ban and persist it to bans.json. `minutes <= 0` = permanent. */
  add(steamId: string, minutes: number, reason?: string): void;
  /** Remove a ban and persist. Returns whether the SteamID64 was banned. */
  remove(steamId: string): boolean;
  /** Get a ban by SteamID64, or null if not banned. */
  get(steamId: string): BanInfo | null;
  /** List every ban currently in the cache. */
  list(): BanEntry[];
  /** Re-read bans.json into the cache (clears the old cache first). */
  reload(): void;
};
