import type { Recipe } from "../recipe.ts";
import { Player } from "@s2script/cs2";

/**
 * Player.respawn() queues CCSPlayerController::Respawn for the next
 * GameFrame, so the resulting player_spawn reaches every plugin's handlers —
 * including this recipe's own listener below. The engine's Respawn honors
 * the game's respawn rules: it no-ops on a competitive mid-round server
 * (players stay dead) and fires in gamemodes that permit respawn (warmup,
 * and TTT-style rules).
 *
 *   sm_respawn <slot>   respawn one slot, called from a command handler
 *   sm_respawn_all      respawn every dead in-game player in one dispatch
 */
export const playerStateRecipe: Recipe = {
  name: "player-state",
  describe: "respawn a player and observe the resulting player_spawn (sm_respawn / sm_respawn_all)",
  register(ctx) {
    ctx.events.on("player_spawn", (e) => {
      const slot = e.getPlayerSlot("userid");
      const p = Player.fromSlot(slot);
      const pawn = p ? p.pawn : null;
      console.log(
        "[cookbook] player-state: player_spawn slot=" + slot +
        " alive=" + (p ? p.pawnIsAlive : null) +
        " health=" + (pawn ? pawn.health : null)
      );
    });

    ctx.events.on("player_death", (e) => {
      console.log("[cookbook] player-state: player_death slot=" + e.getPlayerSlot("userid"));
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

    // sm_respawn_all — respawn every dead in-game player in one dispatch (the multi-entry batch proof).
    ctx.commands.register("sm_respawn_all", (cmd) => {
      let queued = 0, skipped = 0;
      for (const p of Player.all()) {
        if (p.respawn()) queued++; else skipped++;
      }
      cmd.reply("sm_respawn_all: queued=" + queued + " skipped=" + skipped);
    });
  },
};
