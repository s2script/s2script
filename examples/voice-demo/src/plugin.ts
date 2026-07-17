// Live-gate demo for the voice-control slice — shaped exactly like TTT's PlayerMuter:
// lazy mute-on-talk for DEAD players (+ one-time reminder), unmute on spawn, unmute-all on round end,
// plus sm_voicetest for the bot-provable tier (flag set/read without any human voice).
import { Clients } from "@s2script/sdk/clients";
import { Commands } from "@s2script/sdk/commands";
import { Events } from "@s2script/sdk/events";
import { Player } from "@s2script/cs2";

Clients.onVoice((c) => {
  console.log("[voice-demo] onVoice slot=" + c.slot + " name=" + c.name + " muted=" + c.voiceMuted);
  const p = Player.fromSlot(c.slot);
  const pawn = p ? p.pawn : null;
  const dead = !pawn || (pawn.health ?? 0) <= 0;
  if (dead && !c.voiceMuted) {                       // TTT PlayerMuter.cs:39-53, lazily on the talk attempt
    c.voiceMuted = true;
    c.chat("[voice-demo] Dead players are muted until you respawn.");
    console.log("[voice-demo] lazy-muted dead talker slot=" + c.slot);
  }
});

Events.on("player_spawn", (ev) => {                  // TTT :57-62 — clear on respawn
  const slot = ev.getPlayerSlot("userid");
  const c = Clients.fromSlot(slot);
  if (c && c.voiceMuted) { c.voiceMuted = false; console.log("[voice-demo] unmuted slot " + slot + " on spawn"); }
});

Events.on("round_end", () => {                       // TTT :66-70 — clear all at round end
  for (const c of Clients.all()) if (c.voiceMuted) c.voiceMuted = false;
  console.log("[voice-demo] round_end — unmuted all");
});

// Bot-provable gate hook: sm_voicetest <slot> <0|1> — set/read the flag without needing voice traffic.
Commands.register("sm_voicetest", (ctx) => {
  const slot = parseInt(ctx.arg(0), 10);
  const on = ctx.arg(1) !== "0";
  const c = Clients.fromSlot(isNaN(slot) ? -1 : slot);
  if (!c) { ctx.reply("[voice-demo] no client in slot '" + ctx.arg(0) + "'"); return; }
  c.voiceMuted = on;
  ctx.reply("[voice-demo] slot " + slot + " (" + c.name + ") voiceMuted=" + c.voiceMuted);
});

export function onLoad(): void {
  console.log("[voice-demo] onLoad — onVoice armed; sm_voicetest registered");
}
export function onUnload(): void {}
