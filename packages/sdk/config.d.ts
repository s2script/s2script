/** @s2script/config — typed access to the plugin's materialized config. NO runtime code. */
export type Config = Record<string, string | number | boolean>;
export declare const config: {
  getString(key: string): string;
  getInt(key: string): number;
  getFloat(key: string): number;
  getBool(key: string): boolean;
  /** Opt into live-reload: the handler fires with the re-materialized config when the file changes. */
  onChange(handler: (cfg: Config) => void): void;
  /** Read a raw file from the configs dir (name includes its extension, e.g. "maplist.txt"). null if absent. */
  readFile(name: string): string | null;
  /** Write a raw file to the configs dir (creates/overwrites). */
  writeFile(name: string, content: string): void;
};
