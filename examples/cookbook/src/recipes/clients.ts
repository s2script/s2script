import type { Recipe } from "../recipe.ts";
import { Clients } from "@s2script/sdk/clients";
import { Player } from "@s2script/cs2";

/**
 * @s2script/clients — the engine-generic client handle: six lifecycle events
 * plus live-read fields (steamId/name/userId/isBot/signonState). cb_clients
 * snapshots every currently-connected client; cb_voice demonstrates
 * ctx.clients.onVoice with a lazy dead-player mute.
 */
export const clientsRecipe: Recipe = {
  name: "clients",
  describe: "list connected clients + lifecycle state (cb_clients) and onVoice (cb_voice)",
  register(ctx) {
    // --- lifecycle listeners: fire for clients connecting AFTER Active. To
    // cover already-connected clients, seed explicitly with Clients.all() —
    // there is no framework replay of these events.
    ctx.clients.onConnect((c) => {
      console.log(`[cookbook] clients connect slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot} ip=${c.ip}`);
      c.print("s2script cookbook: connected");
      console.log(`[cookbook] kickWithReason surface: typeof=${typeof c.kickWithReason}`);
    });
    ctx.clients.onPutInServer((c) =>
      console.log(`[cookbook] clients putInServer slot=${c.slot} name=${c.name}`));
    ctx.clients.onActive((c) =>
      console.log(`[cookbook] clients active slot=${c.slot} name=${c.name}`));
    ctx.clients.onFullyConnect((c) =>
      console.log(`[cookbook] clients fullyConnect slot=${c.slot} name=${c.name}`));
    ctx.clients.onDisconnect((c) =>
      console.log(`[cookbook] clients disconnect slot=${c.slot} name=${c.name} steamId=${c.steamId}`));
    ctx.clients.onSettingsChanged((c) =>
      console.log(`[cookbook] clients settingsChanged slot=${c.slot} name=${c.name}`));

    // cb_clients — snapshot every currently-connected client (bots included).
    ctx.commands.register("cb_clients", (cmd) => {
      const all = Clients.all();
      cmd.reply(`${all.length} connected client(s):`);
      for (const c of all) {
        cmd.reply(`  slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot} signonState=${c.signonState}`);
      }
    });

    // --- voice: lazy mute-on-talk for DEAD players, unmute on spawn/round_end.
    ctx.clients.onVoice((c) => {
      console.log("[cookbook] clients onVoice slot=" + c.slot + " name=" + c.name + " muted=" + c.voiceMuted);
      const p = Player.fromSlot(c.slot);
      const pawn = p ? p.pawn : null;
      const dead = !pawn || (pawn.health ?? 0) <= 0;
      if (dead && !c.voiceMuted) { // lazily mute on the talk attempt, once
        c.voiceMuted = true;
        c.chat("[cookbook] Dead players are muted until you respawn.");
        console.log("[cookbook] voice lazy-muted dead talker slot=" + c.slot);
      }
    });

    ctx.events.on("player_spawn", (ev) => { // clear on respawn
      const slot = ev.getPlayerSlot("userid");
      const c = Clients.fromSlot(slot);
      if (c && c.voiceMuted) { c.voiceMuted = false; console.log("[cookbook] voice unmuted slot " + slot + " on spawn"); }
    });

    ctx.events.on("round_end", () => { // clear all at round end
      for (const c of Clients.all()) if (c.voiceMuted) c.voiceMuted = false;
      console.log("[cookbook] voice round_end — unmuted all");
    });

    // cb_voice <slot> <0|1> — set/read the mute flag directly, without needing voice traffic.
    ctx.commands.register("cb_voice", (cmd) => {
      const slot = parseInt(cmd.arg(0), 10);
      const on = cmd.arg(1) !== "0";
      const c = Clients.fromSlot(isNaN(slot) ? -1 : slot);
      if (!c) { cmd.reply("[cookbook] no client in slot '" + cmd.arg(0) + "'"); return; }
      c.voiceMuted = on;
      cmd.reply("[cookbook] slot " + slot + " (" + c.name + ") voiceMuted=" + c.voiceMuted);
    });
  },
};
