/** @s2script/votes — chat-ballot voting with revote + an optional live center tally. NO runtime code (injected at load). */
export interface VoteResult { readonly winner: number | null; readonly counts: number[]; readonly total: number; }
export interface VoteConfig {
  question: string;
  /** 2..9 options (chat votes are single-digit). */
  options: string[];
  /** seconds. */
  duration: number;
  /** show a live center tally (default false — the SM chat-only way). */
  showLiveTally?: boolean;
  onEnd: (result: VoteResult) => void;
}
export interface VoteTally { question: string; options: { label: string; count: number }[]; total: number; secondsLeft: number; }
export interface VoteTallyRenderer { show(slot: number, tally: VoteTally): void; clear(slot: number): void; }
export declare const Vote: {
  /** Start a vote (chat ballot to all connected players). Returns false if one is already active. */
  start(config: VoteConfig): boolean;
  isActive(): boolean;
  /** Abort the active vote (no onEnd). */
  cancel(): void;
  /** Register the live-tally renderer (the CS2 center-HTML renderer). */
  registerTallyRenderer(renderer: VoteTallyRenderer): void;
};
