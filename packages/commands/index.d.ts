/** @s2script/commands — register server commands. NO runtime code (injected at load). */
export interface CommandContext {
  /** 0-based caller slot, or -1 for the server console. */
  readonly callerSlot: number;
  /** argString split on whitespace (0-based; the command name is NOT included). Kept for compat. */
  readonly args: string[];
  /** everything after the command name (raw) — SM `GetCmdArgString`. */
  readonly argString: string;
  /** number of whitespace-split arguments — SM `GetCmdArgs`. */
  readonly argCount: number;
  /** the nth argument (0-based), or `""` if absent — SM `GetCmdArg`. */
  arg(n: number): string;
  /** the nth argument parsed as an integer, or `fallback` (default 0) if absent/non-numeric. */
  argInt(n: number, fallback?: number): number;
  /** the nth argument parsed as a float, or `fallback` (default 0) if absent/non-numeric. */
  argFloat(n: number, fallback?: number): number;
  /** every argument from index `n` onward, re-joined with a single space (a reason/message/value that spans spaces). */
  argsFrom(n: number): string;
  /** reply to the caller: server console → server print; a player → their chat. */
  reply(message: string): void;
}
export declare const Commands: {
  register(name: string, handler: (ctx: CommandContext) => void): void;
  registerServer(name: string, handler: (ctx: CommandContext) => void): void;
  registerAdmin(name: string, flags: number, handler: (ctx: CommandContext) => void): void;
};
