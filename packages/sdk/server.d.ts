/** @s2script/server — server control (run console commands, query map validity). NO runtime code (injected at load). */

/**
 * Server console + globals: run commands, register/read/set cvars, and query map/time state.
 * @example
 * import { Server } from "@s2script/sdk/server";
 * // plugins/basecommands/src/plugin.ts:46 — sm_map
 * Server.command("changelevel " + map);
 */
export declare const Server: {
  /** Run `cmd` at the server console (queued; executes next frame). */
  command(cmd: string): void;
  /** Whether `map` is an installed, valid map file. */
  isMapValid(map: string): boolean;
  /** A cvar's current value as a string. `""` if the cvar doesn't exist (or an unsupported type → `"<type>"`). */
  getCvar(name: string): string;
  /**
   * Set a cvar through `ICvar` now (SourceMod `SetConVarString`). Not a console command: `;` in
   * `value` is data, not a second command. `getCvar` and `onCvarChange` see the new value before
   * this returns. `false` if the cvar does not exist or the string cannot become that type.
   */
  setCvar(name: string, value: string): boolean;
  /**
   * Register a plugin-owned ConVar (CSSharp FakeConVar / SM CreateConVar parity). Idempotent —
   * re-registering an existing name is a no-op success, and the cvar + its value persist across
   * plugin reloads (SourceMod parity). The shim adds FCVAR_RELEASE (customer-visible); `flags`
   * are additive raw FCVAR bits. Read the value with `getCvar`; set it with `setCvar`.
   * `min`/`max` apply to numeric types only.
   */
  registerCvar(name: string, opts: {
    type: "bool" | "int" | "float" | "string";
    default: boolean | number | string;
    help?: string;
    flags?: number;
    min?: number;
    max?: number;
  }): boolean;
  /**
   * Watch a cvar for changes (SourceMod `HookConVarChange`, ModSharp's cvar change hook).
   *
   * `name` is a cvar name, or `"*"` for every cvar. The handler receives the cvar's name and its
   * new and old values as strings — for `"*"` the name tells you which one moved.
   *
   * NOTIFY-only: the engine's global change callback runs **after** the value has been applied, so
   * a handler cannot veto a change; returning anything is ignored. A handler that throws is logged
   * and contained, and the remaining handlers still run.
   *
   * Subscriptions are ledgered — unload removes them whether or not `dispose()` is called.
   *
   * @example
   * import { Server } from "@s2script/sdk/server";
   * Server.onCvarChange("mp_friendlyfire", (name, next, prev) => {
   *   console.log(`${name}: ${prev} -> ${next}`);
   * });
   */
  onCvarChange(
    name: string,
    handler: (name: string, newValue: string, oldValue: string) => void,
  ): { dispose(): void };
  /** The server's configured max client count (`GetMaxClients()`). `0` if unavailable. */
  readonly maxPlayers: number;
  /** The current map name (`GetMapName()`, the BSP). `""` if unavailable. */
  readonly mapName: string;
  /** The current map time in seconds (`GetGlobals()->curtime`). `0` if unavailable. */
  readonly gameTime: number;
};
