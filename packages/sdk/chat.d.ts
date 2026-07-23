/** @s2script/chat — print messages to player chat. NO runtime code (injected at load). */

/**
 * Print messages to player chat (SayText2), with a caller-owned {@link Chat.color} prefix.
 * @example
 * import { Chat } from "@s2script/sdk/chat";
 * Chat.toAll("[Vote] Result: " + winner);
 * Chat.toSlot(slot, "Usage: @@<target> <message>");
 */
export declare const Chat: {
  /**
   * An opaque prefix prepended to every chat message (NOT rcon/console replies). Color is content the
   * caller owns (SourceMod-parity), so the engine-generic layer never picks one. A game/plugin sets this
   * to a color control byte — e.g. `Chat.color = ChatColors.Green` from `@s2script/cs2`. `""` = no prefix.
   * A message may also embed its own colors mid-string.
   *
   * You do NOT need a leading space for color to render: every chat line is auto-prefixed with an invisible
   * zero-width space (U+200B) so a Source 2 chat box doesn't swallow the message's first color byte. Just
   * write `Chat.toSlot(slot, ChatColors.Green + "hi")` — the green lands. (Idempotent: a line you already
   * lead with a space or a ZWSP is passed through unchanged.)
   */
  color: string;
  /** Print to the chat of the client in `slot` (0-based). */
  toSlot(slot: number, message: string): void;
  /** Print to every live player's chat. */
  toAll(message: string): void;
};
