/** @s2script/server — server control (run console commands, query map validity). NO runtime code (injected at load). */
export declare const Server: {
  /** Run `cmd` at the server console (queued; executes next frame). */
  command(cmd: string): void;
  /** Whether `map` is an installed, valid map file. */
  isMapValid(map: string): boolean;
};
