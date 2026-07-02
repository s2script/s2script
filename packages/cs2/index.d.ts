/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API. NO runtime code.
 * The typed field accessors are GENERATED (schema.generated.d.ts) from the schema catalog by
 * `s2script gen-schema`; this file adds the hand-written entry points on top.
 */
import type { EntityRef } from "@s2script/entity";
export * from "./schema.generated";
import type { CCSPlayerPawn } from "./schema.generated";

/** A CS2 player pawn: the generated CCSPlayerPawn schema fields + the underlying serial-gated ref. */
export interface Pawn extends CCSPlayerPawn {
  readonly ref: EntityRef;
}
export declare const Pawn: {
  /** The Pawn for a player slot, or null if unoccupied / invalidated. */
  forSlot(slot: number): Pawn | null;
};
