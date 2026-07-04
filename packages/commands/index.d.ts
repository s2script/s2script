/** @s2script/commands — register server commands. NO runtime code (injected at load). */
export interface CommandContext {
  /** 0-based caller slot, or -1 for the server console. */
  readonly callerSlot: number;
  /** argString split on whitespace. */
  readonly args: string[];
  /** everything after the command name (raw). */
  readonly argString: string;
  /** reply to the caller: server console → server print; a player → their chat. */
  reply(message: string): void;
}
export declare const Commands: {
  register(name: string, handler: (ctx: CommandContext) => void): void;
  registerServer(name: string, handler: (ctx: CommandContext) => void): void;
  registerAdmin(name: string, flags: number, handler: (ctx: CommandContext) => void): void;
};
