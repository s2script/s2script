/** @s2script/server — server control (run console commands, query map validity). NO runtime code (injected at load). */
export declare const Server: {
  /** Run `cmd` at the server console (queued; executes next frame). */
  command(cmd: string): void;
  /** Whether `map` is an installed, valid map file. */
  isMapValid(map: string): boolean;
  /** A cvar's current value as a string. `""` if the cvar doesn't exist (or an unsupported type → `"<type>"`). */
  getCvar(name: string): string;
  /** Set a cvar (via the console, so any type is accepted). */
  setCvar(name: string, value: string): void;
};
