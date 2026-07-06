// Pure leaky-bucket flood model (SourceMod antiflood parity). No engine — the caller supplies `now`
// and the per-slot state, so this is fully unit-testable. Each message adds 1 token; the bucket LEAKS
// continuously at 1 token per `floodTime` seconds of elapsed real time. When the post-leak level would
// exceed `maxTokens` the message is blocked and the level is CAPPED at `maxTokens` (so a frustrated
// re-spam can't push it unbounded — the leak always drains it once the client slows/stops). Time-based
// leak is what makes recovery reliable: waiting ~floodTime frees one token, so a client who pauses ~1s
// sends again. `floodTime <= 0` disables (never blocks). Returns the new state + the block decision.

export interface FloodState {
  tokens: number;
  lastTime: number; // seconds
}

export interface FloodResult {
  block: boolean;
  tokens: number;
  lastTime: number;
}

export function floodStep(state: FloodState, now: number, floodTime: number, maxTokens: number): FloodResult {
  if (floodTime <= 0) return { block: false, tokens: state.tokens, lastTime: state.lastTime };
  const leaked = Math.max(0, state.tokens - (now - state.lastTime) / floodTime);
  const level = leaked + 1; // this message adds one token
  if (level > maxTokens) return { block: true, tokens: maxTokens, lastTime: now };
  return { block: false, tokens: level, lastTime: now };
}
