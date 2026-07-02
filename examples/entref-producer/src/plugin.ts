import { publishInterface } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

// Producer: publishes the typed inter-plugin interface @demo/ent@1.0.0. Its pawnRef(slot)
// native returns slot 0's pawn EntityRef across the wire — a LIVE ref the consumer holds and
// validates against the SHARED entity system (Task 1 wired the EntityRef replacer/reviver so it
// round-trips as a live ref, not a dead copy). pawnHealth(slot) is a producer-side schema read so
// the consumer can show a real number while the pawn lives WITHOUT needing a schema offset itself
// (typed cs2 accessors over a wired ref come in 5B).
export function onLoad(): void {
  console.log("[producer] onLoad — publishing @demo/ent@1.0.0");
  publishInterface("@demo/ent", "1.0.0", {
    // Return the pawn's EntityRef across the wire (null if no such pawn yet).
    pawnRef(slot: number) { const p = Pawn.forSlot(slot); return p ? p.ref : null; },
    // Producer-side health read — lets the consumer show a real number while alive without needing
    // a schema offset itself (typed cs2 accessors over a wired ref come in 5B).
    pawnHealth(slot: number) { const p = Pawn.forSlot(slot); return p ? p.health : null; },
  });
}

export function onUnload(): void { console.log("[producer] onUnload"); }
