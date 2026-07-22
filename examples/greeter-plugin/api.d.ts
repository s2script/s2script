/**
 * @demo/greeter — the contract this example publishes.
 *
 * The impl in src/plugin.ts is declared `: Greeter`, so `s2s build` fails if a
 * method drifts from this file. Consumers typecheck against this same file, and
 * the build hashes it into manifest.compiledAgainst — a drifted contract is
 * refused at load rather than marshalled across.
 *
 * Note the interface name here matches the package name, so the manifest's
 * `publishes` block is derived automatically as "self". A contract named
 * differently from its package needs an authored publishes entry with a
 * concrete version.
 */
import type { EntityRef } from "@s2script/sdk/entity";

export interface Greeter {
  /** Greet the player in `slot`. */
  greet(slot: number): string;
  /**
   * The slot's pawn as a live, serial-gated EntityRef — or null if there is
   * no such pawn. The ref survives the crossing as a LIVE ref: the consumer
   * validates it against the SHARED entity system, and it flips to invalid
   * when the pawn dies. Never a raw pointer, never a dead copy.
   */
  pawnRef(slot: number): EntityRef | null;
  /** A producer-side schema read, so the consumer needs no offset of its own. */
  pawnHealth(slot: number): number | null;
}
