/**
 * Chat visibility — who can read whose chat. Hand-written: the view is not the generic
 * number-params shape hookgen emits.
 */
import type { HookResultValue } from "@s2script/sdk/events";
import type { Player } from "./index.d.ts";

/**
 * Block-scoped view of one recipient check. The engine asks once per recipient while it builds a
 * chat message's recipient list; the sender always sees their own message. Valid only during the
 * handler: do not stash it or read it after an `await`.
 */
export interface CanHearChatView {
  /** The player who would see the message, or null when the controller could not be resolved. */
  readonly receiver: Player | null;
  /** The player who sent it, or null when the controller could not be resolved. */
  readonly sender: Player | null;
  /** True for team chat (`say_team`). */
  readonly teamOnly: boolean;
  /** Write it to vote. Reads back what this handler wrote, `true` before any write. */
  canHear: boolean;
}

/**
 * Chat visibility. Register during the load window (module top level or `OnPluginStart`); a call
 * after the plugin settles throws. Subscriptions are owned by the calling plugin and torn down
 * with it.
 */
export interface ChatApi {
  /**
   * Decide whether `receiver` sees a chat message from `sender`. Return `Continue` to leave it to
   * the engine (and any other plugin). Return `Handled` or `Stop` to vote the `canHear` this handler
   * wrote, or `false` when it wrote none; `Changed` votes only when it wrote `canHear`. When anyone
   * voted, the engine's own rule is skipped and any `false` vote wins. `Stop` also keeps later
   * handlers from running.
   * @example
   * import { chat, HookResult } from "@s2script/cs2";
   * // Living players never read the dead.
   * chat.onCanHear(v => {
   *   if (v.sender?.pawnIsAlive === false && v.receiver?.pawnIsAlive) {
   *     v.canHear = false;
   *     return HookResult.Handled;
   *   }
   * });
   */
  onCanHear(handler: (view: CanHearChatView) => HookResultValue | void): void;
}

/** Load-window chat visibility: `import { chat } from "@s2script/cs2"`. Throws after settle. */
export declare const chat: ChatApi;
