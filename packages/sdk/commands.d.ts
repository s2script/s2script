/** @s2script/commands — register server commands. NO runtime code (injected at load). */

/**
 * Where a command was invoked from — SourceMod's *reply source*. Set by the dispatch path and
 * exposed as {@link CommandInvocation.replySource}; it is what {@link CommandInvocation.reply}
 * routes on.
 *
 * - `"server"` — the server console or rcon (`callerSlot` is `-1`)
 * - `"console"` — a player's own developer console
 * - `"chat"` — a `!` or `/` chat trigger
 */
export type ReplySource = "server" | "console" | "chat";

/**
 * The parsed invocation handed to a command callback: who called it, its arguments (multiple typed
 * accessors), and a caller-appropriate `reply` channel. It is a plain object that captures no native
 * handle, so it MAY be retained and used after an `await`/`.then` — a deferred `reply` (e.g. from
 * inside `delay(...).then(...)` or an async DB/HTTP call) is safe to call once the awaited work
 * completes.
 *
 * What it captures, though, is the caller's **slot** — not a stable identity. If the original caller
 * disconnects before the deferred reply runs and a different player has since taken that slot, the
 * reply routes to (or targets the console/chat channel of) whoever now occupies it, not the original
 * caller. Prefer replying synchronously where the timing matters, or re-check the caller (e.g. via
 * their user id) before trusting a slot held across a long-running await.
 */
export interface CommandInvocation {
  /** 0-based caller slot, or -1 for the server console. */
  readonly callerSlot: number;
  /** Where this invocation came from — what {@link CommandInvocation.reply} routes on. */
  readonly replySource: ReplySource;
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
  /**
   * Reply to the caller in the channel they used — SourceMod's `ReplyToCommand`. Routed by
   * {@link CommandInvocation.replySource}: `"server"` → the server console, `"console"` → the
   * caller's own developer console (both control-bytes-stripped), `"chat"` → their chat, one frame later.
   *
   * `Chat.color`'s global prefix applies to the chat path only and never decorates a console reply.
   * To pin a channel regardless of how the command was invoked, use {@link
   * CommandInvocation.replyToChat} or {@link CommandInvocation.replyToConsole}.
   *
   * @example
   * // `!help` answers in chat; `sm_help` typed at a console answers in that console.
   * ctx.commands.register("sm_help", (cmd) => cmd.reply("[SM] Commands: …"));
   */
  reply(message: string): void;
  /**
   * Force the reply into the caller's chat, whichever channel they actually used — SM `PrintToChat`.
   * Sent raw (colour is content you own) and deferred one frame, so a `!cmd` answer lands *after*
   * the player's own chat line rather than above it. The server console (`callerSlot` `-1`) has no
   * chat channel and degrades to the server console.
   */
  replyToChat(message: string): void;
  /**
   * Force the reply into the caller's developer console — SM `PrintToConsole`. Control bytes are
   * stripped (a chat colour *is* a control byte, and renders as garbage in a console), and the line
   * is printed immediately. The server console (`callerSlot` `-1`) prints to the server console.
   */
  replyToConsole(message: string): void;
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
  /**
   * Invoke a registered command by name in THIS plugin (applying its gating). Returns true if it
   * exists.
   *
   * `replySource` sets where the command's {@link CommandInvocation.reply} lands. Omit it and it
   * falls back to the slot — the server console at `-1`, else that player's own console, matching
   * SourceMod's `FakeClientCommand`. Pass `"chat"` when re-dispatching on a player's behalf from a
   * chat context.
   */
  dispatch(name: string, slot: number, argString: string, replySource?: ReplySource): boolean;
  /** Parse a chat message for a trigger (`!`/`/`). Returns the parsed trigger, or null if it's ordinary chat. */
  parseChatTrigger(message: string): ChatTrigger | null;
  /** If `message` is a trigger, dispatch the command (tries `name` then `sm_<name>`) as `slot`; returns
   * `{ silent, ran }` (the caller should suppress the chat message), or null if it was ordinary chat.
   * Always dispatches with `replySource` `"chat"` — including the silent `/` trigger, where `silent`
   * suppresses the player's own line but the answer still belongs in chat. */
  handleChatTrigger(slot: number, message: string): { silent: boolean; ran: boolean } | null;
  /** The trigger characters — SM PublicChatTrigger (`"!"`) / SilentChatTrigger (`"/"`). Mutate to reconfigure. */
  readonly triggers: { public: string; silent: string };
  /** Every globally-registered command with its required admin `flags`: `0` = anyone, `-1` = console/server-only,
   * else the `ADMFLAG` bit mask (map bits→names in your plugin). The `sm_help` backend. */
  list(): { name: string; flags: number }[];
};
