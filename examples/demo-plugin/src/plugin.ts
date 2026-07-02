import { OnGameFrame } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

// A minimal @s2script/cs2-targeting plugin exercising the Slice 5A EntityRef guardrail.
// It STASHES a Pawn (an EntityRef-backed {index,serial}) once, then every ~256 frames reads
//   - the STASHED pawn's health: stays 100 while its entity lives; goes null the moment that
//     entity is destroyed (serial mismatch) — proving a held ref never dereferences a stale
//     pointer, closing the Slice-3 use-after-free.
//   - a FRESH Pawn.forSlot(0): recovers with the new serial once a pawn respawns / a bot rejoins.
// Built to a .s2sp by `npx s2script build`; dropped into addons/s2script/plugins/ to load.
let stashed: Pawn | null = null;
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 !== 0) return;
    if (!stashed) stashed = Pawn.forSlot(0);        // stash once
    const fresh = Pawn.forSlot(0);
    console.log("[demo] tick " + ticks
      + " stashed.health=" + (stashed ? stashed.health : "none")   // null once that pawn died
      + " fresh.health=" + (fresh ? fresh.health : "none"));       // works again after respawn
    if (stashed && stashed.health === null) { stashed = null; }     // re-stash next tick
  });
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
