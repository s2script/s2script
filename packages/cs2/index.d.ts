/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API.
 * NO runtime code: the s2script engine injects the real implementation at load time.
 * Plugins consume this package for TypeScript type checking only.
 *
 * CS2-specific identifiers live here (not in @s2script/std) per the
 * "core is engine-generic; games are packages" convention.
 */

/** A CS2 player pawn. */
export declare interface Pawn {
  /** Current health value of the pawn, or null if the entity ref is stale. */
  health: number | null;
}

export declare const Pawn: {
  /**
   * Return the Pawn for the given player slot, or null if the slot is
   * unoccupied or the pawn has been invalidated.
   */
  forSlot(slot: number): Pawn | null;
};
