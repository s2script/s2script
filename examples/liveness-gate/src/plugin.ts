// E1 live gate — holds a pawn ref across a changelevel and pokes it every frame (the exact pre-E1
// crash shape: capture a pawn ref, then per-frame read pawn.isValid (staging flags) + a schema
// field on it). Pre-E1 this SEGV'd after a changelevel freed the entity storage; with E1's
// host-books authority the ref must resolve dead (isValid=false) instead of dereferencing freed memory.
import { Events } from "@s2script/sdk/events";
import { OnGameFrame } from "@s2script/sdk/frame";
import { Server } from "@s2script/sdk/server";
import { Player } from "@s2script/cs2";

let heldPawn: any = null;
let lastState = "";

function tryCapture(slot: number): boolean {
  const p = Player.fromSlot(slot);
  if (p && p.pawn && p.pawn.isValid) {
    heldPawn = p.pawn;
    console.log(`[gate] holding pawn ref slot=${slot} idx=${heldPawn.ref.index} id=${heldPawn.ref.id} valid=${heldPawn.isValid}`);
    return true;
  }
  return false;
}

export function onLoad() {
  console.log("[gate] onLoad — armed (watch player_spawn + per-frame poll for a live pawn to hold)");

  Events.on("player_spawn", (ev) => {
    if (heldPawn) return;
    tryCapture(ev.getPlayerSlot("userid"));
  });

  Server.onMapStart((map) => console.log(`[gate] map start: ${map} — held ref should now be dead`));

  OnGameFrame.subscribe(() => {
    if (!heldPawn) {
      // Fallback: grab any live pawn so the test doesn't depend on the player_spawn timing.
      for (let s = 0; s < 12; s++) { if (tryCapture(s)) break; }
      return;
    }
    // The pre-E1 crash site: isValid (staging flags) + a schema read on a (possibly stale) ref.
    const state = `valid=${heldPawn.isValid} health=${heldPawn.health}`;
    if (state !== lastState) {
      lastState = state;
      console.log(`[gate] ${state}`);
    }
  });
}
