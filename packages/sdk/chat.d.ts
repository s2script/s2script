/** @s2script/chat — print messages to player chat. NO runtime code (injected at load). */
import type { HookResultValue } from "./events";

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
  /**
   * Subscribe to raw player chat. The handler receives the speaker's `slot`, the raw `text`, and
   * `teamonly`. Returning `>= HookResult.Handled` (2) suppresses the broadcast (SM-parity). Non-command
   * chat lines are delivered here; the `@`-trigger layer subscribes through this.
   */
  onMessage(handler: (slot: number, text: string, teamonly: boolean) => HookResultValue | void): void;
};
