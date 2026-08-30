import type { Recipe } from "../recipe.ts";
import { Clients } from "@s2script/sdk/clients";
import { Player } from "@s2script/cs2";
import { hook } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";

/**
 * @s2script/clients — the engine-generic client handle: six lifecycle events
 * plus live-read fields (steamId/name/userId/isBot/signonState). sm_clients
 * snapshots every currently-connected client; sm_voice demonstrates
 * ctx.clients.onVoice with a lazy dead-player mute.
 *
 * onVoice fires per-frame while a client is talking, so its per-packet log line is opt-in via
 * `sm_voice verbose` (off by default) — same reasoning as recipes/damage.ts defaulting its effect
 * off: loading the cookbook must not spam the console on its own.
 */
export const clientsRecipe: Recipe = {
  name: "clients",
  describe: "list connected clients + lifecycle state (sm_clients) and onVoice (sm_voice / sm_voice verbose)",
  register() {
    let voiceVerbose = false;
    // --- lifecycle listeners: fire for clients connecting AFTER Active. To
    // cover already-connected clients, seed explicitly with Clients.all() —
    // there is no framework replay of these events.
    hook.connect((c) => {
      console.log(`[cookbook] clients connect slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot} ip=${c.ip}`);
      c.print("s2script cookbook: connected");
      console.log(`[cookbook] kickWithReason surface: typeof=${typeof c.kickWithReason}`);
    });
    hook.putInServer((c) =>
      console.log(`[cookbook] clients putInServer slot=${c.slot} name=${c.name}`));
    hook.active((c) =>
      console.log(`[cookbook] clients active slot=${c.slot} name=${c.name}`));
    hook.fullyConnect((c) =>
      console.log(`[cookbook] clients fullyConnect slot=${c.slot} name=${c.name}`));
    hook.disconnect((c) =>
      console.log(`[cookbook] clients disconnect slot=${c.slot} name=${c.name} steamId=${c.steamId}`));
    hook.settingsChanged((c) =>
      console.log(`[cookbook] clients settingsChanged slot=${c.slot} name=${c.name}`));

    // sm_clients — snapshot every currently-connected client (bots included).
    command("sm_clients", (cmd) => {
      const all = Clients.all();
      cmd.reply(`${all.length} connected client(s):`);
      for (const c of all) {
        cmd.reply(`  slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot} signonState=${c.signonState}`);
      }
    });

    // --- voice: lazy mute-on-talk for DEAD players, unmute on spawn/round_end.
    hook.voice((c) => {
      if (voiceVerbose) {
        console.log("[cookbook] clients onVoice slot=" + c.slot + " name=" + c.name + " muted=" + c.voiceMuted);
      }
      const p = Player.fromSlot(c.slot);
      const pawn = p ? p.pawn : null;
      const dead = !pawn || (pawn.health ?? 0) <= 0;
      if (dead && !c.voiceMuted) { // lazily mute on the talk attempt, once
        c.voiceMuted = true;
        c.chat("[cookbook] Dead players are muted until you respawn.");
        console.log("[cookbook] voice lazy-muted dead talker slot=" + c.slot);
      }
    });

    hook.event("player_spawn", (ev) => { // clear on respawn
      const slot = ev.getPlayerSlot("userid");
      const c = Clients.fromSlot(slot);
      if (c && c.voiceMuted) { c.voiceMuted = false; console.log("[cookbook] voice unmuted slot " + slot + " on spawn"); }
    });

    hook.event("round_end", () => { // clear all at round end
      for (const c of Clients.all()) if (c.voiceMuted) c.voiceMuted = false;
      console.log("[cookbook] voice round_end — unmuted all");
    });

    // Client command execution. TWO different mechanisms:
    //   sm_clientcmd — ClientCommand: asks the CLIENT to run it, so it does nothing on a bot.
    //   sm_fakecmd   — FakeClientCommand: the SERVER processes it as if the client sent it, so it
    //                  DOES work on bots, and reaches command handlers including our own.
    command("sm_clientcmd", (cmd) => {
      const slot = cmd.argInt(0, -1);
      const rest = cmd.argsFrom(1);
      if (slot < 0 || slot > 63 || !rest) {
        cmd.reply("[cookbook] usage: sm_clientcmd <slot 0-63> <command...>");
        return;
      }
      const c = Clients.fromSlot(slot);
      if (!c) { cmd.reply(`[cookbook] clients: no client in slot ${slot}`); return; }
      const ok = c.command(rest);
      cmd.reply(`[cookbook] clients: command(slot ${slot}, ${JSON.stringify(rest)}) -> ${ok}` +
        (c.isBot ? " (bot — expect no visible effect: no console)" : ""));
    });

    command("sm_fakecmd", (cmd) => {
      const slot = cmd.argInt(0, -1);
      const rest = cmd.argsFrom(1);
      if (slot < 0 || slot > 63 || !rest) {
        cmd.reply("[cookbook] usage: sm_fakecmd <slot 0-63> <command...>");
        return;
      }
      const c = Clients.fromSlot(slot);
      if (!c) { cmd.reply(`[cookbook] clients: no client in slot ${slot}`); return; }
      const ok = c.fakeCommand(rest);
      cmd.reply(`[cookbook] clients: fakeCommand(slot ${slot}, ${JSON.stringify(rest)}) -> ${ok}` +
        (c.isBot ? " (bot — this one DOES work server-side)" : "") +
        "  [try: sm_fakecmd <slot> say hi — engine commands run; another PLUGIN's command is " +
        "dispatched but its handler is re-entrancy-skipped, use a cross-plugin interface instead]");
    });

    // sm_voice verbose — toggle the per-packet onVoice log (see the toggle note above).
    // sm_voice <slot> <0|1> — set/read the mute flag directly, without needing voice traffic.
    command("sm_voice", (cmd) => {
      if (cmd.arg(0) === "verbose") {
        voiceVerbose = !voiceVerbose;
        cmd.reply("[cookbook] voice verbose logging = " + (voiceVerbose ? "on" : "off"));
        return;
      }
      const slot = parseInt(cmd.arg(0), 10);
      const on = cmd.arg(1) !== "0";
      const c = Clients.fromSlot(isNaN(slot) ? -1 : slot);
      if (!c) { cmd.reply("[cookbook] no client in slot '" + cmd.arg(0) + "'"); return; }
      c.voiceMuted = on;
      cmd.reply("[cookbook] slot " + slot + " (" + c.name + ") voiceMuted=" + c.voiceMuted);
    });
  },
};
