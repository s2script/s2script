// Pure token-decay flood model (SourceMod antiflood parity). No engine — the caller supplies `now`
// and the per-slot state, so this is fully unit-testable. A message arriving faster than `floodTime`
// accrues a token; a well-spaced message decays one; crossing `maxTokens` blocks. `floodTime <= 0`
// disables (never blocks). Returns the new state + the block decision (caller stores {tokens, lastTime}).

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
  const dt = now - state.lastTime;
  const tokens = dt < floodTime ? state.tokens + 1 : Math.max(0, state.tokens - 1);
  return { block: tokens >= maxTokens, tokens, lastTime: now };
}
