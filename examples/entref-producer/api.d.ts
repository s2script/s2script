/** @demo/ent — the contract this example publishes. Note the package is @demo/entref-producer:
 *  the interface name is deliberately NOT the package name (design spec §4.2). */
import type { EntityRef } from "@s2script/entity";

export interface Ent {
  /** Slot's pawn as a live serial-gated ref, or null if there is no such pawn. */
  pawnRef(slot: number): EntityRef | null;
  /** Producer-side schema read so the consumer needs no offset of its own. */
  pawnHealth(slot: number): number | null;
}
