/** @s2script/plugins — runtime plugin management (the SM `sm plugins` backend). NO runtime code (injected). */
export interface PluginInfo {
  /** the plugin id (from its manifest). */
  readonly id: string;
  /** true = running; false = manually unloaded (on disk but not running, via `unload`). */
  readonly loaded: boolean;
}
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
