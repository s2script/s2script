/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API. NO runtime code.
 * The typed field accessors are GENERATED (schema.generated.d.ts) from the schema catalog by
 * `s2script gen-schema`; this file adds the hand-written entry points on top.
 */
import type { EntityRef } from "@s2script/entity";
export * from "./schema.generated";
import type { CCSPlayerPawn, CCSPlayerController } from "./schema.generated";

/**
 * A CS2 player pawn (the in-world body): the generated CCSPlayerPawn schema fields + the serial-gated ref.
 * `controller` is the typed reverse hop (shadows the raw generated m_hController handle).
 */
export interface Pawn extends Omit<CCSPlayerPawn, "controller"> {
  readonly ref: EntityRef;
  /** The player controlling this pawn, or null if stale/absent. */
  readonly controller: Player | null;
}
export declare const Pawn: {
  /** The Pawn for a player slot, or null if unoccupied / invalidated. */
  forSlot(slot: number): Pawn | null;
};

/**
 * A CS2 player (the persistent controller entity): the generated CCSPlayerController schema fields
 * (team/score/ping/…) + the serial-gated controller ref. `pawn` is the typed body (shadows the raw
 * generated m_hPawn handle). Referenced by slot (0-based); a stored Player degrades to null on reuse.
 */
export interface Player extends Omit<CCSPlayerController, "pawn"> {
  readonly ref: EntityRef;
  /** The 0-based player slot (CPlayerSlot). */
  readonly slot: number;
  /** This player's in-world pawn (the body), or null if dead/absent. */
  readonly pawn: Pawn | null;
}
export declare const Player: {
  /** The Player for a 0-based slot, or null if the slot is unoccupied / the controller is stale. */
  fromSlot(slot: number): Player | null;
  /** Every connected player (slots with a valid controller). */
  all(): Player[];
};
