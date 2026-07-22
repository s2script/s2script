/** @s2script/config — typed access to the plugin's materialized config. NO runtime code. */
/** A materialized config value: a scalar, or a nested section object of further values. */
export type ConfigValue = string | number | boolean | { [k: string]: ConfigValue };
/** The whole materialized config: top-level keys to {@link ConfigValue}s. Passed to `ctx.config.onChange`. */
export type Config = Record<string, ConfigValue>;
/**
 * Typed access to this plugin's materialized config values (declared under `s2script.config` in the manifest).
 * @example
 * import { config } from "@s2script/sdk/config";
 * // plugins/antiflood/src/plugin.ts:31 — re-read on live-reload
 * const maxTokens = config.getInt("max_tokens");
 */
export declare const config: {
  /** A config value as a string. `""` if the key is absent. */
  getString(key: string): string;
  /** A config value as an integer. `0` if the key is absent or non-numeric. */
  getInt(key: string): number;
  /** A config value as a float. `0` if the key is absent or non-numeric. */
  getFloat(key: string): number;
  /** A config value as a boolean. `false` if the key is absent. */
  getBool(key: string): boolean;
  /** Read a raw file from the configs dir (name includes its extension, e.g. "maplist.txt"). null if absent. */
  readFile(name: string): string | null;
  /** Write a raw file to the configs dir (creates/overwrites). */
  writeFile(name: string, content: string): void;
};
