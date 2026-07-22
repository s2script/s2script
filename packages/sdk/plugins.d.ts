/** @s2script/plugins — runtime plugin management (the SM `sm plugins` backend). NO runtime code (injected). */

/** A snapshot of one plugin's identity and lifecycle state, as returned by {@link Plugins.list}. */
export interface PluginInfo {
  /** the plugin id (from its manifest). */
  readonly id: string;
  /** true = running; false = not running (loading, waiting, failed, or manually unloaded). Exactly
   * `state === "running"`. */
  readonly loaded: boolean;
  /** the plugin's lifecycle state (L1 lifecycle v2):
   * - `"running"`  — Active (factory settled, armed).
   * - `"loading"`  — an in-flight factory load (async factory not yet settled).
   * - `"waiting"`  — parked until a hard-dependency producer is published (or a ~30s timeout).
   * - `"failed"`   — the load failed (bad artifact, throwing/rejecting factory, publishes mismatch,
   *                  or a load timeout); the plugin is NOT running.
   * - `"unloaded"` — on disk but manually unloaded via `unload` (not auto-reloaded until `load`/`reload`).
   */
  readonly state: "running" | "loading" | "waiting" | "failed" | "unloaded";
}
/**
 * Enumerate and drive the plugin set at runtime (list / load / unload / reload).
 * @example
 * import { Plugins } from "@s2script/sdk/plugins";
 * // plugins/basecommands/src/plugin.ts:126 — sm plugins list
 * for (const p of Plugins.list()) cmd.reply(p.id + " — " + p.state);
 */
export declare const Plugins: {
  /** Every loaded/unloaded plugin. */
  list(): PluginInfo[];
  /** Unload a running plugin (deferred to the next frame drain). false if it isn't loaded. It stays
   * unloaded (not auto-reloaded by the file-watcher) until `load`/`reload`. */
  unload(id: string): boolean;
  /** Reload a plugin (unload + load from disk, deferred). false if the id is unknown. */
  reload(id: string): boolean;
  /** Load a previously-unloaded plugin (deferred). false if it isn't currently unloaded. */
  load(id: string): boolean;
};
