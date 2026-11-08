/**
 * @s2script/sdk/steam — the server's own Steam session and Steam Workshop downloads. NO runtime
 * code: the engine injects the implementation at load (`__s2pkg_steam`).
 *
 * Every Source 2 dedicated server runs the Steamworks game-server API, so this is engine-generic.
 * The engine owns the Steam session (it logs on and runs the callbacks); s2script borrows it, the
 * way SourceMod's SteamWorks extension and ModSharp's `ISteamApi` do.
 *
 * Workshop ids are 64-bit. They come back as **decimal strings**; anywhere an id is taken, a decimal
 * string, a positive safe integer or a `bigint` works.
 *
 * Steam may be unavailable (a LAN server, `-insecure` with no Steam, a server still logging on).
 * Nothing here throws for that: queries answer `null` / `false`, and {@link Workshop.download}
 * resolves `{ ok: false }`. Wait for {@link Steam.onConnected} before downloading at boot.
 *
 * @example Download every workshop map in a list once Steam is up
 * import { Steam, Workshop } from "@s2script/sdk/steam";
 * const ids = ["3070284539", "3121217565"];
 * async function fetchAll() {
 *   for (const id of ids) {
 *     if (Workshop.isInstalled(id)) continue;
 *     const r = await Workshop.download(id);
 *     console.log(`workshop ${id}: ${r.ok ? "ready" : "failed (EResult " + r.result + ")"}`);
 *   }
 * }
 * export function OnPluginStart() {
 *   Steam.onConnected(fetchAll);
 *   if (Steam.isLoggedOn()) fetchAll();
 * }
 */

/** A workshop item id: a decimal string, a positive safe integer, or a `bigint`. */
export type WorkshopId = string | number | bigint;

/**
 * Steam's `EResult` values that a download or session event commonly reports. Any other EResult
 * can arrive too; compare against the number.
 */
export declare const SteamResult: {
  readonly OK: 1;
  readonly Fail: 2;
  readonly NoConnection: 3;
  readonly InvalidParam: 8;
  readonly FileNotFound: 9;
  readonly Busy: 10;
  readonly AccessDenied: 15;
  readonly Timeout: 16;
  readonly ServiceUnavailable: 20;
  readonly Expired: 27;
  readonly DiskFull: 54;
};

/** `ISteamUGC::GetItemState` bit flags, as {@link Workshop.state} returns them. */
export declare const WorkshopItemState: {
  /** Not tracked by Steam on this server. */
  readonly None: 0;
  readonly Subscribed: 1;
  readonly LegacyItem: 2;
  /** Installed and usable, possibly out of date. */
  readonly Installed: 4;
  /** Not installed yet, or the author updated it. */
  readonly NeedsUpdate: 8;
  readonly Downloading: 16;
  /** {@link Workshop.download} was called; the content is not available until its result arrives. */
  readonly DownloadPending: 32;
  readonly DisabledLocally: 64;
};

/** The server's Steam session. */
export declare const Steam: {
  /** True once the engine's Steam game-server session is up and its interfaces resolved. */
  isAvailable(): boolean;
  /** True while the server is logged on to Steam. Downloads need this. */
  isLoggedOn(): boolean;
  /** True when Steam reports the server as VAC secure. */
  isSecure(): boolean;
  /** The server's SteamID64 as a decimal string, or null before it has logged on. */
  steamId(): string | null;
  /** The server's public IPv4 address as Steam sees it (`"203.0.113.7"`), or null when unknown. */
  publicIp(): string | null;
  /** The server logged on to Steam (at boot, and again after any reconnect). Lives as long as the plugin. */
  onConnected(handler: () => void): void;
  /** The server lost its Steam connection. `result` is the EResult. */
  onDisconnected(handler: (result: number) => void): void;
  /** A log-on attempt failed. `stillRetrying` says whether Steam keeps trying. */
  onConnectFailure(handler: (result: number, stillRetrying: boolean) => void): void;
};

/** How a download finished. */
export interface WorkshopDownloadResult {
  /** The item id, as a decimal string. */
  readonly id: string;
  /** The EResult. `0` when the request never reached Steam (bad id, Steam unavailable or not logged on). */
  readonly result: number;
  /** `result === SteamResult.OK`: the item's files are installed and current. */
  readonly ok: boolean;
}

/** A download result for any item, including ones the engine or another plugin requested. */
export interface WorkshopDownloadEvent extends WorkshopDownloadResult {
  /** The app the item belongs to. */
  readonly appId: number;
}

/** Progress of an item that is downloading. */
export interface WorkshopDownloadProgress {
  readonly id: string;
  readonly bytesDownloaded: number;
  readonly bytesTotal: number;
}

/** Where an installed item lives on disk. */
export interface WorkshopInstallInfo {
  readonly id: string;
  /** The item's install folder. */
  readonly folder: string;
  readonly sizeOnDisk: number;
  /** When the installed version was last updated, in Unix seconds. */
  readonly timestamp: number;
}

/** Steam Workshop (`ISteamUGC`) for the server. */
export declare const Workshop: {
  /**
   * Download an item, or update it when the installed copy is out of date (`ISteamUGC::DownloadItem`).
   * Resolves once Steam reports the result, which for an item that is already current is right away.
   * Resolves `{ ok: false, result: 0 }` when the request could not be made.
   *
   * `highPriority` moves the item to the front of Steam's download queue.
   */
  download(id: WorkshopId, options?: { highPriority?: boolean }): Promise<WorkshopDownloadResult>;
  /**
   * Ask Steam to download an item without waiting for it. True when Steam accepted the request; the
   * result arrives on {@link Workshop.onDownloadResult}.
   */
  request(id: WorkshopId, options?: { highPriority?: boolean }): boolean;
  /** The item's {@link WorkshopItemState} flags, or null when Steam is unavailable or the id is bad. */
  state(id: WorkshopId): number | null;
  /** True when the item is installed and does not need an update. */
  isInstalled(id: WorkshopId): boolean;
  /** Download progress, or null when the item has no download in progress or Steam is unavailable. */
  downloadProgress(id: WorkshopId): WorkshopDownloadProgress | null;
  /** Where the item is installed, or null when it is not installed. */
  installInfo(id: WorkshopId): WorkshopInstallInfo | null;
  /** Pause (`true`, the default) or resume (`false`) all workshop downloads. False when Steam is unavailable. */
  suspendDownloads(suspend?: boolean): boolean;
  /** Every download result on this server, whoever asked for it. Lives as long as the plugin. */
  onDownloadResult(handler: (event: WorkshopDownloadEvent) => void): void;
  /** An item was installed or updated on disk. */
  onItemInstalled(handler: (event: { readonly id: string; readonly appId: number }) => void): void;
};
