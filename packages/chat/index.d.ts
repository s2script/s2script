/** @s2script/chat — print messages to player chat. NO runtime code (injected at load). */
export declare const Chat: {
  /** Print to the chat of the client in `slot` (0-based). */
  toSlot(slot: number, message: string): void;
  /** Print to every live player's chat. */
  toAll(message: string): void;
};
