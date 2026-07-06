/** @s2script/server — server control (run console commands, query map validity). NO runtime code (injected at load). */
export declare const Server: {
  /** Run `cmd` at the server console (queued; executes next frame). */
  command(cmd: string): void;
  /** Whether `map` is an installed, valid map file. */
  isMapValid(map: string): boolean;
  /** A cvar's current value as a string. `""` if the cvar doesn't exist (or an unsupported type → `"<type>"`). */
  getCvar(name: string): string;
  /**
   * Set a cvar via the server console (so any type is accepted). SECURITY: this builds and runs a console
   * command (`<name> <value>`), which the console splits on `;` — treat `value` like `command()` input and
   * sanitize/quote any untrusted value to avoid command injection. Queued: applies next frame.
   */
  setCvar(name: string, value: string): void;
  /** The server's configured max client count (`GetMaxClients()`). `0` if unavailable. */
  readonly maxPlayers: number;
  /** The current map name (`GetMapName()`, the BSP). `""` if unavailable. */
  readonly mapName: string;
  /** The current map time in seconds (`GetGlobals()->curtime`). `0` if unavailable. */
  readonly gameTime: number;
};
