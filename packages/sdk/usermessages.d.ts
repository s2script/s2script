import type { HookResultValue } from "./events";

/** A general protobuf user-message builder. Build then send in one synchronous burst. */
export class UserMessage {
  constructor(name: string);
  setInt(field: string, value: number): this;
  setFloat(field: string, value: number): this;
  setString(field: string, value: string): this;
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
  hasField(path: string): boolean;
  /** Scalar int read (int32/uint32/fixed32/enum; bool as 0/1). Dotted nested paths supported
   *  ("origin.x" walks sub-messages). null = no such field / repeated / no current message. */
  readInt(path: string): number | null;
  readFloat(path: string): number | null;
  readBool(path: string): boolean | null;
  readString(path: string): string | null;
}

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
