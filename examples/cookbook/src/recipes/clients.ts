import { Clients, hook, command, HookResult } from "@s2script/sdk";
import type { Client } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

/**
 * Clients — the engine-generic client handle: six lifecycle events
 * plus live-read fields (steamId/name/userId/isBot/signonState). sm_clients
 * snapshots every currently-connected client; sm_voice demonstrates
 * OnClientVoice with a lazy dead-player mute.
 *
 * onVoice fires per-frame while a client is talking, so its per-packet log line is opt-in via
 * `sm_voice verbose` (off by default) — same reasoning as recipes/damage.ts defaulting its effect
 * off: loading the cookbook must not spam the console on its own.
 */
let voiceVerbose = false;

export const name = "clients";
export const describe = "list connected clients + lifecycle state (sm_clients) and onVoice (sm_voice / sm_voice verbose)";

export function OnClientConnected(c: Client): void {
  console.log(`[cookbook] clients connect slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot} ip=${c.ip}`);
  c.print("s2script cookbook: connected");
  console.log(`[cookbook] kickWithReason surface: typeof=${typeof c.kickWithReason}`);
}

export function OnClientPutInServer(c: Client): void {
  console.log(`[cookbook] clients putInServer slot=${c.slot} name=${c.name}`);
}

export function OnClientActive(c: Client): void {
  console.log(`[cookbook] clients active slot=${c.slot} name=${c.name}`);
}

export function OnClientPostAdminCheck(c: Client): void {
  console.log(`[cookbook] clients fullyConnect slot=${c.slot} name=${c.name}`);
}

export function OnClientDisconnect(c: Client): void {
  console.log(`[cookbook] clients disconnect slot=${c.slot} name=${c.name} steamId=${c.steamId}`);
}

export function OnClientSettingsChanged(c: Client): void {
  console.log(`[cookbook] clients settingsChanged slot=${c.slot} name=${c.name}`);
}

export function OnClientVoice(c: Client): void {
  if (voiceVerbose) {
    console.log("[cookbook] clients onVoice slot=" + c.slot + " name=" + c.name + " muted=" + c.voiceMuted);
  }
  const p = Player.fromSlot(c.slot);
  const pawn = p ? p.pawn : null;
  const dead = !pawn || (pawn.health ?? 0) <= 0;
  if (dead && !c.voiceMuted) {
    c.voiceMuted = true;
    c.chat("[cookbook] Dead players are muted until you respawn.");
    console.log("[cookbook] voice lazy-muted dead talker slot=" + c.slot);
  }
}

export function OnPluginStart(): void {
  command("sm_clients", (cmd) => {
    const all = Clients.all();
    cmd.reply(`${all.length} connected client(s):`);
    for (const c of all) {
      cmd.reply(`  slot=${c.slot} name=${c.name} steamId=${c.steamId} userId=${c.userId} isBot=${c.isBot} signonState=${c.signonState}`);
    }
    return HookResult.Handled;
  });

  hook.on("player_spawn", (ev) => {
    const slot = ev.getPlayerSlot("userid");
    const c = Clients.fromSlot(slot);
    if (c && c.voiceMuted) { c.voiceMuted = false; console.log("[cookbook] voice unmuted slot " + slot + " on spawn"); }
  });

  hook.on("round_end", () => { // clear all at round end
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
      return HookResult.Handled;
    }
    const c = Clients.fromSlot(slot);
    if (!c) { cmd.reply(`[cookbook] clients: no client in slot ${slot}`); return HookResult.Handled; }
    const ok = c.command(rest);
    cmd.reply(`[cookbook] clients: command(slot ${slot}, ${JSON.stringify(rest)}) -> ${ok}` +
      (c.isBot ? " (bot — expect no visible effect: no console)" : ""));
    return HookResult.Handled;
  });

  command("sm_fakecmd", (cmd) => {
    const slot = cmd.argInt(0, -1);
    const rest = cmd.argsFrom(1);
    if (slot < 0 || slot > 63 || !rest) {
      cmd.reply("[cookbook] usage: sm_fakecmd <slot 0-63> <command...>");
      return HookResult.Handled;
    }
    const c = Clients.fromSlot(slot);
    if (!c) { cmd.reply(`[cookbook] clients: no client in slot ${slot}`); return HookResult.Handled; }
    const ok = c.fakeCommand(rest);
    cmd.reply(`[cookbook] clients: fakeCommand(slot ${slot}, ${JSON.stringify(rest)}) -> ${ok}` +
      (c.isBot ? " (bot — this one DOES work server-side)" : "") +
      "  [try: sm_fakecmd <slot> say hi — engine commands run; another PLUGIN's command is " +
      "dispatched but its handler is re-entrancy-skipped, use a cross-plugin interface instead]");
    return HookResult.Handled;
  });

  // sm_voice verbose — toggle the per-packet onVoice log (see the toggle note above).
  // sm_voice <slot> <0|1> — set/read the mute flag directly, without needing voice traffic.
  command("sm_voice", (cmd) => {
    if (cmd.arg(0) === "verbose") {
      voiceVerbose = !voiceVerbose;
      cmd.reply("[cookbook] voice verbose logging = " + (voiceVerbose ? "on" : "off"));
      return HookResult.Handled;
    }
    const slot = parseInt(cmd.arg(0), 10);
    const on = cmd.arg(1) !== "0";
    const c = Clients.fromSlot(isNaN(slot) ? -1 : slot);
    if (!c) { cmd.reply("[cookbook] no client in slot '" + cmd.arg(0) + "'"); return HookResult.Handled; }
    c.voiceMuted = on;
    cmd.reply("[cookbook] slot " + slot + " (" + c.name + ") voiceMuted=" + c.voiceMuted);
    return HookResult.Handled;
  });
}
