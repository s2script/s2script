/**
 * @s2script/sdk/voice — who can hear whom. NO runtime code: the engine injects the implementation
 * at load (`__s2pkg_voice`).
 *
 * Layered UNDER `Client.voiceMuted`: a muted sender is inaudible to everyone regardless of any rule
 * here, so admin moderation always beats a gameplay rule.
 *
 * Rules are declarative state, not a callback. The engine's listen matrix is re-asserted
 * continuously and the underlying hook fires per (receiver, sender) pair, so a per-pair JS callback
 * would run up to 64x64 times per refresh. Set a rule; the shim enforces it.
 */

export interface VoiceStats {
  /** Listen-matrix decisions the engine has asked about. */
  calls: number;
  /** Senders currently carrying a merged rule. */
  entries: number;
  /** Times a rule actually silenced a pair. THE effect counter — a rule that never rewrites is not working. */
  rewrites: number;
}

export declare const Voice: {
  /**
   * This sender is audible ONLY to `receivers` (0-based slots). An empty array means audible to
   * nobody — distinct from {@link Voice.reset}, which removes the rule and lets the engine decide.
   * Rules from multiple plugins AND-merge, so another plugin can only ever narrow yours, never widen it.
   * Returns false when voice control is degraded on this build.
   */
  setAudibleTo(sender: number, receivers: readonly number[]): boolean;
  /** Drop THIS plugin's rule for `sender`. False when no rule of yours was present. */
  reset(sender: number): boolean;
  /** Drop every rule this plugin owns. Unload does this automatically. */
  resetAll(): void;
  /** Hot-path counters, or null when the running shim predates this capability. */
  stats(): VoiceStats | null;
};
