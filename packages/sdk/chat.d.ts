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
   * to a color control byte — e.g. `Chat.color = ChatColors.Green` from `@s2script/cs2`. `""` = send raw
   * (CS2 needs a leading control byte for chat to render). A message may also embed its own colors.
   */
  color: string;
  /** Print to the chat of the client in `slot` (0-based). */
  toSlot(slot: number, message: string): void;
  /** Print to every live player's chat. */
  toAll(message: string): void;
};
