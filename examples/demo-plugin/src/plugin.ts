import type { EntityRef } from "@s2script/entity";
import { Player } from "@s2script/cs2";

// Slice 5E.3 live gate — reload state-handoff. onUnload returns state (a reload counter + a tracked
// pawn EntityRef); the host carries it across the reload gap; onLoad(prev) restores it. A file edit
// (touch → Reload) increments the counter WITHOUT losing it; the pawn ref survives as a live,
// serial-gated ref (isValid() → false, not a crash, if the entity died during the gap). First load →
// prev === undefined. A final removal (Vanished) discards the pending state (a re-add starts fresh).
interface State { reloads: number; pawn: EntityRef | null; }

let reloads = 0;
let pawn: EntityRef | null = null;

export function onLoad(prev?: State): void {
  if (prev) { reloads = prev.reloads; pawn = prev.pawn; }
  const pawnAlive = pawn ? pawn.isValid() : null;   // serial-gated: true while alive, false if it died in the gap
  console.log("[demo] onLoad — reloads=" + reloads + " hadPrev=" + (prev !== undefined)
    + " pawnAlive=" + String(pawnAlive)
    + (pawn ? " pawnRef=" + pawn.index + "/" + pawn.serial : ""));
  // Track the first live player's pawn ref so the NEXT reload proves EntityRef survival across the gap.
  const p = Player.all()[0];
  if (p && p.pawn) { pawn = p.pawn.ref; }
}

export function onUnload(): State {
  reloads += 1;
  console.log("[demo] onUnload — handing off reloads=" + reloads);
  return { reloads, pawn };
}
