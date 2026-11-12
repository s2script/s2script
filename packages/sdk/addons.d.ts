/**
 * @s2script/sdk/addons — workshop addons mounted on the server and pushed to every client, the
 * job MultiAddonManager does as a separate Metamod plugin. NO runtime code: the engine injects the
 * implementation at load (`__s2pkg_addons`).
 *
 * Engine-generic: every Source 2 server mounts addons into its "GAME" search path and tells each
 * client which addons to load. Which addons is your data; downloading them is
 * `@s2script/sdk/steam`'s {@link import("./steam").Workshop}.
 *
 * Three pieces, each switched on by its own engine facts (see {@link Addons.status}):
 * - **mount**: an installed addon's VPK goes into the server's "GAME" search path.
 * - **mapAddons**: every map load carries {@link Addons.setExtraAddons}'s list after the map's own
 *   workshop addon, so the server lists them to clients.
 * - **clientHandshake**: a client downloads one addon per connection. s2script names the first one
 *   it lacks when it connects, and the next one each time it comes back, until it has them all. A
 *   client that is still on its first addon after the connect timeout is dropped.
 *
 * One plugin manages addons at a time: the first one to change anything owns the state, a second
 * plugin's change throws `S2Error { code: "AddonsOwned" }`, and the owner's unload unmounts
 * everything it mounted and clears every list. The base plugin `addonmanager` is that plugin on a
 * stock install.
 *
 * Ids are decimal workshop ids. Anywhere an id is taken, a decimal string, a positive safe integer
 * or a `bigint` works; lists come back as decimal strings in load order.
 *
 * @example Mount one addon once it is installed, and send it to every client
 * import { Addons } from "@s2script/sdk/addons";
 * import { Workshop } from "@s2script/sdk/steam";
 * export async function OnPluginStart() {
 *   const id = "3090239773";
 *   if (!Workshop.isInstalled(id)) await Workshop.download(id);
 *   const info = Workshop.installInfo(id);
 *   if (info) Addons.mount(id, info.folder);
 *   Addons.setExtraAddons([id]);
 * }
 */

import type { WorkshopId } from "./steam";

/** What {@link Addons.mount} did. */
export type AddonMountResult =
  /** The addon's VPK is now in the "GAME" search path. */
  | "mounted"
  /** Already mounted by s2script, or it is the current map's own addon. */
  | "already"
  /** The filesystem interface is unavailable on this build (see {@link Addons.status}). */
  | "unavailable"
  /** Not a workshop id. */
  | "invalid"
  /** Neither `<id>_dir.vpk` nor `<id>.vpk` is in the folder. */
  | "missing";

/** Which parts of addon support this build's engine facts switched on. */
export interface AddonsStatus {
  /** Mounting into the server's "GAME" search path. */
  readonly mount: boolean;
  /** Map loads carrying the extra addons. */
  readonly mapAddons: boolean;
  /** The per-client download handshake. */
  readonly clientHandshake: boolean;
}

/** Handshake timing. Every field is optional; omitted fields take the default. */
export interface AddonsOptions {
  /**
   * Seconds a client has to come back after being sent away for an extra addon, for that addon to
   * count as downloaded. Default 10.
   */
  extraAddonTimeout?: number;
  /**
   * Seconds a client may stay on its first addon download before it is dropped; 0 never drops.
   * Default 30.
   */
  connectTimeout?: number;
  /** Remember which addons a client has across map changes and rejoins. Default false. */
  cacheClients?: boolean;
  /** How long a cached list lasts after the client was last seen, in seconds; 0 is forever. Default 0. */
  cacheSeconds?: number;
}

export declare const Addons: {
  /** Which parts are switched on. */
  status(): AddonsStatus;
  /**
   * Mount an installed addon. `folder` is its install folder, as
   * {@link import("./steam").Workshop.installInfo} reports it. `tail: true` searches it after
   * everything else instead of first.
   */
  mount(id: WorkshopId, folder: string, options?: { tail?: boolean }): AddonMountResult;
  /** Remove a mount made by {@link Addons.mount}. False when s2script had not mounted it. */
  unmount(id: WorkshopId): boolean;
  /** The addons s2script has mounted, in mount order. */
  mounted(): string[];
  /** The addons every map load carries after the map's own one. Replaces the previous list. */
  setExtraAddons(ids: readonly WorkshopId[]): void;
  /**
   * The client-only addons every client loads, or with `steamId` (a SteamID64), the ones only that
   * client loads. Replaces the previous list.
   */
  setClientAddons(ids: readonly WorkshopId[], steamId?: string | bigint): void;
  /** What a client is told to load, in order: the map's, the mounted ones, then the client lists. */
  clientAddons(steamId?: string | bigint): string[];
  /** Set the handshake timing. */
  configure(options: AddonsOptions): void;
  /** Forget which addons one client (or, with no argument, every client) has downloaded. */
  clearClientCache(steamId?: string | bigint): void;
  /** The running map's own workshop addon(s); empty on a stock map. */
  currentMap(): string[];
  /** The plugin that manages the addon state, or null when none does. */
  owner(): string | null;
};
