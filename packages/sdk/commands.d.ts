/** @s2script/commands — register server commands. NO runtime code (injected at load). */

/**
 * The parsed invocation handed to a command callback: who called it, its arguments (multiple typed
 * accessors), and a caller-appropriate `reply` channel. Valid only for the duration of the callback.
 */
export interface CommandInvocation {
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
  /** reply to the caller, translated for THEIR language (SM's `%t` on the reply path). Soft-deps
   * `@s2script/translations` — degrades to the raw `key` if it isn't loaded. */
  replyT(key: string, ...args: (string | number)[]): void;
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

/**
 * Command-registry utilities: dispatch by name, parse/route chat triggers, and enumerate the global
 * registry. Commands themselves are registered through the plugin context (`ctx.commands.register*`).
 * @example
 * import { Commands } from "@s2script/sdk/commands";
 * // sm_help backend: every registered command + its required admin flag mask.
 * const cmds = Commands.list().slice().sort((a, b) => (a.name < b.name ? -1 : 1));
 */
export declare const Commands: {
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
