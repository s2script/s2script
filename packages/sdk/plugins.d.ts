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
  /** The folder under `plugins/` (e.g. `"disabled"`, `"fun"`) this plugin was loaded from by
   * {@link Plugins.load} for this run only — it stays there on disk, so a restart leaves it off.
   * null for a plugin loaded from `plugins/` itself. */
  readonly folder: string | null;
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
  /**
   * Load a plugin that isn't running (deferred): one unloaded earlier this run, by id — or, as
   * `"<folder>/<plugin id>"` (e.g. `"fun/@edgegamers/jailbreak"`, `"disabled/@edge/rtv"`), one kept
   * in any folder under `plugins/`, for THIS RUN ONLY. Only a folder prefix reaches into a folder;
   * the id is mapped to its file for you; nothing outside `plugins/` is reachable. The file stays
   * where it is (a restart leaves it off; for disabled/ plugins, `enable` makes it permanent).
   * false if there is no such plugin. `unload` accepts the same `<folder>/<plugin id>` form.
   */
  load(id: string): boolean;
  /**
   * Add registry plugins to the server's `addons/s2script/plugins.json` and install them with their
   * dependencies (`sm plugins install`). A bare name records `^<latest>`; `name@range` records the
   * range. `with` names optional members of the ONE bundle given. The files land in `plugins/`,
   * where the loader picks them up — or, with `disabled`, in `plugins/disabled/`, where it never
   * looks (kept current at startup, loaded once enabled). Resolves with report lines; rejects with
   * the reason. Requires the `plugins:manage` permission (default: `@s2script/basecommands` only).
   */
  install(names: string[], opts?: { with?: string[]; disabled?: boolean }): Promise<string[]>;
  /** Move the named plugins (all when omitted) to the newest version their plugins.json range allows. */
  update(names?: string[]): Promise<string[]>;
  /** Drop plugins from plugins.json; their files are removed (unless edited by hand) and unloaded. */
  remove(names: string[]): Promise<string[]>;
  /** One line per plugins.json dependency: current, wanted (newest in range) and latest version. */
  outdated(): Promise<string[]>;
  /** Move installed plugins out of plugins/disabled/ (they load) and record it in plugins.json. */
  enable(names: string[]): Promise<string[]>;
  /** Move installed plugins into plugins/disabled/ (they unload, and stay off across restarts). */
  disable(names: string[]): Promise<string[]>;
};
