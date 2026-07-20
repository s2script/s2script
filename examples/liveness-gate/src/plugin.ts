// E1 live gate — holds a pawn ref across a changelevel and pokes it every frame (the exact pre-E1
// crash shape: a player_spawn handler captures a pawn ref, then per-frame reads pawn.isValid
// (staging flags) + a schema field on it). Pre-E1 this SEGV'd after a changelevel freed the entity
// storage; with E1's host-books authority the ref must resolve dead (isValid=false) instead.
import { Events } from "@s2script/sdk/events";
import { OnGameFrame } from "@s2script/sdk/frame";
import { Server } from "@s2script/sdk/server";
import { Player } from "@s2script/cs2";

let heldPawn: any = null;
let lastState = "";

export function onLoad() {
  Events.on("player_spawn", (ev) => {
    const slot = ev.getPlayerSlot("userid");
    const p = Player.fromSlot(slot);
    if (p && p.pawn) {
      heldPawn = p.pawn;
      console.log(`[gate] holding pawn ref idx=${heldPawn.ref.index} id=${heldPawn.ref.id}`);
    }
  });

  Server.onMapStart((map) => console.log(`[gate] map start: ${map} — held ref should now be dead`));

  OnGameFrame.subscribe(() => {
    if (!heldPawn) return;
    // The pre-E1 crash site: isValid (staging flags) + a schema read on a stale ref.
    const state = `valid=${heldPawn.isValid} health=${heldPawn.health}`;
    if (state !== lastState) {
      lastState = state;
      console.log(`[gate] ${state}`);
    }
  });
}
