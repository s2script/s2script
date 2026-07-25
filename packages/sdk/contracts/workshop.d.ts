/**
 * `@s2script/sdk/contracts/workshop` — the standard contract for a **workshop / UGC** plugin.
 *
 * TYPE-ONLY. The framework ships no implementation: subscribing to, downloading and mounting UGC is
 * a Steam-services concern, not a Source 2 engine touchpoint. A community plugin implements this and
 * publishes it; consumers depend on the one agreed shape. See `contracts/README.md`.
 *
 * Engine-generic on purpose — every Source 2 title has workshop content, so nothing here is
 * CS2-specific. Game-specific economy lives in `@s2script/cs2/econ`.
 *
 * @example
 * import type { WorkshopService } from "@s2script/sdk/contracts/workshop";
 * const ws = ctx.tryUse<WorkshopService>("workshop");
 * if (ws) {
 *   const map = await ws.currentMap();
 *   console.log(map ? `${map.title} (${map.id})` : "not a workshop map");
 * }
 */

/**
 * A workshop item. `id` is a **decimal string**, never a number: published-file IDs are 64-bit and a
 * `BigInt` cannot cross the plugin boundary (it throws and drops the whole payload).
 */
export interface WorkshopItem {
  /** Published-file ID as a decimal string, e.g. `"3070288894"`. */
  readonly id: string;
  /** Item title, or `""` when metadata has not been fetched yet. */
  readonly title: string;
  /** Owning author's SteamID64 as a decimal string, or `""` if unknown. */
  readonly authorSteamId: string;
  /** Bytes on disk once installed; `0` when not installed or unknown. */
  readonly sizeBytes: number;
  /** Last-updated Unix seconds; `0` if unknown. */
  readonly updatedAt: number;
}

/** An installed workshop map, as reported by {@link WorkshopService.currentMap}. */
export interface WorkshopMap extends WorkshopItem {
  /** The BSP name the engine loads (what `Server.mapName` reports once live). */
  readonly mapName: string;
}

/** Progress for an in-flight download. */
export interface WorkshopDownloadProgress {
  readonly id: string;
  readonly bytesDownloaded: number;
  readonly bytesTotal: number;
}

/**
 * The contract a workshop plugin publishes.
 *
 * Everything that touches Steam is asynchronous — an implementation must not block the game frame.
 * Methods resolve to `null`/`false` rather than throwing when an item is unknown or Steam is
 * unreachable, so a consumer degrades instead of crashing (the framework's standing posture).
 */
export interface WorkshopService {
  /** The currently-running workshop map, or `null` when the server is on a stock map. */
  currentMap(): Promise<WorkshopMap | null>;

  /** Metadata for `id`, or `null` if unknown/unreachable. */
  info(id: string): Promise<WorkshopItem | null>;

  /**
   * Ensure `id` is installed and up to date, resolving `true` once usable.
   * Implementations should be idempotent — calling it for an already-current item is a cheap `true`.
   */
  ensureInstalled(id: string): Promise<boolean>;

  /** In-flight download progress, or `null` when nothing is downloading for `id`. */
  progress(id: string): Promise<WorkshopDownloadProgress | null>;

  /**
   * Change level to a workshop map. Resolves `false` when the item is not a map, is not installed
   * and could not be fetched, or the change was refused. On success the server changes level, so a
   * caller should not assume any code after this runs on the same map.
   */
  changeMap(id: string): Promise<boolean>;

  /** Every installed workshop item known to the implementation. */
  installed(): Promise<readonly WorkshopItem[]>;
}
