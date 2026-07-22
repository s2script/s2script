import type { HookResultValue } from "./events";

/** A general protobuf user-message builder. Build then send in one synchronous burst. */
export class UserMessage {
  constructor(name: string);
  /** Set an integer field. Returns `this` for chaining. */
  setInt(field: string, value: number): this;
  /** Set a float field. Returns `this` for chaining. */
  setFloat(field: string, value: number): this;
  /** Set a string field. Returns `this` for chaining. */
  setString(field: string, value: string): this;
  /** Set a boolean field. Returns `this` for chaining. */
  setBool(field: string, value: boolean): this;
  /** Infer the setter from the JS value type. */
  set(field: string, value: number | string | boolean): this;
  /** Send to one slot or a list of slots. Returns true if delivered to >=1 real client. */
  send(slots: number | number[]): boolean;
  /** Broadcast to all connected non-bot clients. */
  sendAll(): boolean;
}

// --- UserMessage interception (usermsg-hook slice) ---

/** A BLOCK-SCOPED view of an intercepted outbound user message — valid only during a
 *  UserMessages.onPre handler; across an await (or stashed) all reads return null/[]/"". */
export interface UserMessageView {
  /** Canonical unscoped message name (e.g. "CMsgTEFireBullets"). */
  readonly name: string;
  /** Numeric network-message id (e.g. 452). */
  readonly id: number;
  /** Recipient slots (0-based) this post targets; a broadcast decodes to all live slots. Read-only in v1. */
  readonly recipients: number[];
  /** protobuf TextFormat dump — the documented FALLBACK for unmapped messages only; prefer typed reads. */
  readonly debugString: string;
  /** Whether `path` is present on the current message. Dotted nested paths supported ("origin.x"). */
  hasField(path: string): boolean;
  /** Scalar int read (int32/uint32/fixed32/enum; bool as 0/1). Dotted nested paths supported
   *  ("origin.x" walks sub-messages). null = no such field / repeated / no current message. */
  readInt(path: string): number | null;
  /** Scalar float read (float/double). Dotted nested paths supported. null if absent/repeated/no message. */
  readFloat(path: string): number | null;
  /** Scalar bool read. Dotted nested paths supported. null if absent/repeated/no message. */
  readBool(path: string): boolean | null;
  /** Scalar string/bytes read. Dotted nested paths supported. null if absent/repeated/no message. */
  readString(path: string): string | null;
}

/**
 * Intercept outbound user messages before delivery (read typed fields, optionally suppress the send).
 * @example
 * import { UserMessages } from "@s2script/sdk/usermessages";
 * import { HookResult, type HookResultValue } from "@s2script/sdk/events";
 * // examples/usermsg-demo/src/plugin.ts:15 — blanket-block radio text
 * UserMessages.onPre("CCSUsrMsg_RadioText", (m): HookResultValue | void => {
 *   if (blockRadio) return HookResult.Handled;
 * });
 */
export declare const UserMessages: {
  /** Pre-hook an outbound user message by unscoped name (partial match, SayText2-style; the view
   *  carries the canonical name). Runs SYNCHRONOUSLY before delivery. Return >= HookResult.Handled
   *  to SUPPRESS the send for every recipient; Continue/undefined passes it through. THROWS at
   *  subscribe time on an unresolvable name or a degraded intercept descriptor. */
  onPre(name: string, handler: (msg: UserMessageView) => HookResultValue | void): void;
  /** Removes ALL of the calling plugin's handlers for this name (mux off semantics — handler
   *  identity not compared, matching Events.off). */
  off(name: string, handler?: (msg: UserMessageView) => HookResultValue | void): void;
};
