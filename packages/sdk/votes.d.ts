/** @s2script/votes — chat-ballot voting with revote + an optional live center tally. NO runtime code (injected at load). */

/** The outcome delivered to {@link VoteConfig.onEnd} when a vote ends. */
export interface VoteResult {
  /** 0-based index of the winning option, or null on a tie or zero votes. */
  readonly winner: number | null;
  /** Per-option vote counts, parallel to {@link VoteConfig.options}. */
  readonly counts: number[];
  /** Total votes cast. */
  readonly total: number;
}
/** Configuration for {@link Vote.start}. */
export interface VoteConfig {
  /** The question shown to voters. */
  question: string;
  /** 2..9 options (chat votes are single-digit). */
  options: string[];
  /** seconds. */
  duration: number;
  /** show a live center tally (default false — the SM chat-only way). */
  showLiveTally?: boolean;
  /** Called once when the vote ends (or is exhausted), with the tallied {@link VoteResult}. It may start a new vote (the lock is released first). */
  onEnd: (result: VoteResult) => void;
}
/** A per-tick snapshot of an in-progress vote, handed to a {@link VoteTallyRenderer}. */
export interface VoteTally {
  /** The question being voted on. */
  question: string;
  /** Each option's label and current vote count. */
  options: { label: string; count: number }[];
  /** Total votes cast so far. */
  total: number;
  /** Seconds remaining before the vote ends. */
  secondsLeft: number;
}
/** A live-tally display backend for votes — registered via {@link Vote.registerTallyRenderer}. */
export interface VoteTallyRenderer {
  /** Show/update the running tally for `slot`. */
  show(slot: number, tally: VoteTally): void;
  /** Remove the tally display for `slot` (the vote ended or the player left). */
  clear(slot: number): void;
}
/**
 * The voting entry point — one active chat-ballot vote at a time, tallied on close.
 * @example
 * import { Vote } from "@s2script/sdk/votes";
 * Vote.start({
 *   question: `Kick ${name}?`,
 *   options: ["Yes", "No"],
 *   duration: 20,
 *   showLiveTally: true,
 *   onEnd: (r) => {
 *     if (r.winner === 0 && r.counts[0] > r.total / 2) kick(name);
 *   },
 * });
 */
export declare const Vote: {
  /** Start a vote (chat ballot to all connected players). Returns false if one is already active. */
  start(config: VoteConfig): boolean;
  /** True while a vote is running (the single-vote lock is held). */
  isActive(): boolean;
  /** Abort the active vote (no onEnd). */
  cancel(): void;
  /** Register the live-tally renderer (the CS2 center-HTML renderer). */
  registerTallyRenderer(renderer: VoteTallyRenderer): void;
};
