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
/** A parsed chat trigger: which command + args, and whether it was the silent (`/`) trigger. */
export interface ChatTrigger {
  /** `true` = the silent trigger (`/`, hidden); `false` = the public trigger (`!`). */
  readonly silent: boolean;
  /** the command name (the first token after the trigger char; NOT `sm_`-prefixed). */
  readonly name: string;
  /** everything after the command name. */
  readonly argString: string;
}

export declare const Commands: {
  register(name: string, handler: (ctx: CommandContext) => void): void;
  registerServer(name: string, handler: (ctx: CommandContext) => void): void;
  registerAdmin(name: string, flags: number, handler: (ctx: CommandContext) => void): void;
  /** Invoke a registered command by name in THIS plugin (applying its gating). Returns true if it exists. */
  dispatch(name: string, slot: number, argString: string): boolean;
  /** Parse a chat message for a trigger (`!`/`/`). Returns the parsed trigger, or null if it's ordinary chat. */
  parseChatTrigger(message: string): ChatTrigger | null;
  /** If `message` is a trigger, dispatch the command (tries `name` then `sm_<name>`) as `slot`; returns
   * `{ silent, ran }` (the caller should suppress the chat message), or null if it was ordinary chat. */
  handleChatTrigger(slot: number, message: string): { silent: boolean; ran: boolean } | null;
  /** The trigger characters — SM PublicChatTrigger (`"!"`) / SilentChatTrigger (`"/"`). Mutate to reconfigure. */
  readonly triggers: { public: string; silent: string };
  /** Every globally-registered command with its required admin `flags`: `0` = anyone, `-1` = console/server-only,
   * else the `ADMFLAG` bit mask (map bits→names in your plugin). The `sm_help` backend. */
  list(): { name: string; flags: number }[];
};
