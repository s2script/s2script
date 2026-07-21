// @s2script/basecomm — SourceMod basecomm: communication control (gag/mute/silence + un-versions).
//
//  - GAG (chat): VERIFIED. A gagged speaker's say/say_team is suppressed server-side by returning
//    HookResult.Handled from Chat.onMessage (the live-proven Host_Say path). Keyed by SteamID so a gag
//    doesn't follow a slot to a reconnecting player.
//  - MUTE (voice): REAL. Flips Client.voiceMuted — the shim's SetClientListening rewrite silences the
//    sender's outgoing voice for every receiver (the CSSharp/Swiftly mechanism; supersedes the old
//    best-effort m_bHasCommunicationAbuseMute plan). The schema flag is still written as a cosmetic
//    scoreboard indicator only. Keyed by SteamID and re-asserted on putinserver so a mute survives a
//    reconnect. sm_silence = gag + mute.

import { plugin } from "@s2script/sdk/plugin";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Player, pickPlayer } from "@s2script/cs2";
import { HookResult } from "@s2script/sdk/events";
import { Clients } from "@s2script/sdk/clients";

const gagged = new Set<string>(); // SteamIDs — chat suppressed
const muted = new Set<string>();  // SteamIDs — voice mute requested (best-effort)

// Convention: filterImmunity=true for a punitive command (drops targets of higher immunity than the
// caller); filterImmunity=false for a reversal command (un-gag/un-mute/un-silence — no filter).
function forTargets(pat: string, callerSlot: number, reply: (m: string) => void, verb: string, usage: string, act: (p: Player) => void, filterImmunity: boolean): void {
  if (!pat) { reply("Usage: " + usage); return; }
  const targets = Player.target(pat, callerSlot, filterImmunity);
  if (targets.length === 0) { reply("[SM] No matching players."); return; }
  for (const p of targets) act(p);
  reply("[SM] " + verb + " " + targets.length + " player" + (targets.length === 1 ? "" : "s") + ".");
}

function setGag(p: Player, on: boolean): void {
  const sid = p.steamId;
  if (!sid) return;
  if (on) gagged.add(sid); else gagged.delete(sid);
}

function setMute(p: Player, on: boolean): void {
  const c = Clients.fromSlot(p.slot);
  if (c) c.voiceMuted = on;                 // REAL server-side voice mute (voice-control slice)
  p.hasCommunicationAbuseMute = on;         // cosmetic scoreboard indicator (best-effort, kept)
  const sid = p.steamId;
  if (!sid) return;
  if (on) muted.add(sid); else muted.delete(sid);
}

export default plugin((ctx) => {
  // Suppress chat from a gagged speaker (both say and say_team route through Host_Say).
  ctx.clients.onSay((slot, _text, _teamonly) => {
    if (gagged.size === 0) return HookResult.Continue;
    const p = Player.fromSlot(slot);
    const sid = p ? p.steamId : null;
    return sid && gagged.has(sid) ? HookResult.Handled : HookResult.Continue;
  });

  // A muted player who reconnects gets a fresh slot with a cleared flag (shim slot hygiene) — re-assert
  // the SteamID-keyed admin mute once their controller exists.
  ctx.clients.onPutInServer((c) => {
    if (muted.has(c.steamId)) c.voiceMuted = true;
  });

  ctx.commands.registerAdmin("sm_gag", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Gagged", "sm_gag <target>", (p) => setGag(p, true), true));
  ctx.commands.registerAdmin("sm_ungag", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Ungagged", "sm_ungag <target>", (p) => setGag(p, false), false));
  ctx.commands.registerAdmin("sm_mute", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Muted", "sm_mute <target>", (p) => setMute(p, true), true));
  ctx.commands.registerAdmin("sm_unmute", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Unmuted", "sm_unmute <target>", (p) => setMute(p, false), false));
  ctx.commands.registerAdmin("sm_silence", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Silenced", "sm_silence <target>", (p) => { setGag(p, true); setMute(p, true); }, true));
  ctx.commands.registerAdmin("sm_unsilence", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Unsilenced", "sm_unsilence <target>", (p) => { setGag(p, false); setMute(p, false); }, false));

  // adminmenu — Gag proof item, same ADMFLAG as sm_gag, via pickPlayer + the shared setGag routine.
  ctx.topmenu.addItem("Player Commands", { id: "basecomm:gag", name: "Gag", flags: ADMFLAG.CHAT,
    onSelect: adminSlot => pickPlayer(adminSlot, t => setGag(t, true)) });

  console.log("[basecomm] onLoad - gag/ungag/mute/unmute/silence/unsilence registered");
});
