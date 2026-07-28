/**
 * A DELIBERATELY STALE pre-migration verified copy: it says `greet` takes a number, which the
 * sibling's real api.d.ts never did. If this file were still authoritative the consumer's
 * `g.greet("world")` would fail to typecheck and compiledAgainst would carry this file's hash —
 * so it is the proof that the sibling wins (spec §5.3, decision #9).
 */
export interface Greeter {
  greet(name: number): string;
}
