// @demo/respawn-demo — demo for the player-respawn slice (Player.respawn -> the sig-resolved,
// RTTI-vtable-membership-gated CCSPlayerController::Respawn, preceded by the sig-resolved SetPawn,
// queued to the next GameFrame OUTSIDE the JS isolate borrow so the resulting player_spawn reaches
// every plugin's handlers — including this one's).
//
//   sm_respawn <slot>   — Player.respawn on one slot, called FROM a command handler (a JS dispatch —
//                         the re-entrancy hazard path). The player_spawn logger below firing for our
//                         OWN respawn is the deferred-drain proof.
//   sm_respawnall       — the TTT round-start loop shape: respawn every dead player in ONE dispatch
//                         (multi-entry pending-set proof).
//
// NOTE: the engine's Respawn honors the game's respawn rules — it no-ops on a competitive mid-round
// server (players stay dead) and fires in gamemodes that permit respawn (warmup, and TTT's own rules).

import { plugin } from "@s2script/sdk/plugin";
import { Player } from "@s2script/cs2";

export default plugin((ctx) => {
  ctx.events.on("player_spawn", (e) => {
    const slot = e.getPlayerSlot("userid");
    const p = Player.fromSlot(slot);
    const pawn = p ? p.pawn : null;
    console.log(
      "[respawn-demo] player_spawn slot=" + slot +
      " alive=" + (p ? p.pawnIsAlive : null) +
      " health=" + (pawn ? pawn.health : null)
    );
  });

  ctx.events.on("player_death", (e) => {
    console.log("[respawn-demo] player_death slot=" + e.getPlayerSlot("userid"));
  });

  // sm_respawn <slot> — single-target respawn from a command handler.
  ctx.commands.register("sm_respawn", (cmd) => {
    const slot = cmd.argInt(0, -1);
    const p = slot >= 0 ? Player.fromSlot(slot) : null;
    if (!p) { cmd.reply("sm_respawn: no player in slot " + slot); return; }
    const ok = p.respawn();
    cmd.reply("sm_respawn slot=" + slot + " -> " +
      (ok ? "queued (executes next frame)" : "no-op (already alive / stale ref / degraded descriptor)"));
  });

  // sm_respawnall — respawn every dead in-game player in one dispatch (the multi-entry batch proof).
  ctx.commands.register("sm_respawnall", (cmd) => {
    let queued = 0, skipped = 0;
    for (const p of Player.all()) {
      if (p.respawn()) queued++; else skipped++;
    }
    cmd.reply("sm_respawnall: queued=" + queued + " skipped=" + skipped);
  });

  console.log("[respawn-demo] onLoad — sm_respawn / sm_respawnall registered");
});
