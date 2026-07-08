// @s2script/basecomm — SourceMod basecomm: communication control (gag/mute/silence + un-versions).
//
//  - GAG (chat): VERIFIED. A gagged speaker's say/say_team is suppressed server-side by returning
//    HookResult.Handled from Chat.onMessage (the live-proven Host_Say path). Keyed by SteamID so a gag
//    doesn't follow a slot to a reconnecting player.
//  - MUTE (voice): BEST-EFFORT. Sets the schema field m_bHasCommunicationAbuseMute (an existing
//    generated setter: writeBool + notifyStateChanged). The field-write is proven; whether CS2 actually
//    suppresses the muted client's voice from it is UNVERIFIED (it may be matchmaking/GC-driven). The
//    robust path is a CServerSideClient::CLCMsg_VoiceData detour (like the 6.6 damage hook) — a
//    deferred follow-up. sm_silence = gag + mute.

import { Commands } from "@s2script/commands";
import { Chat } from "@s2script/chat";
import { ADMFLAG } from "@s2script/admin";
import { Player, pickPlayer } from "@s2script/cs2";
import { HookResult } from "@s2script/events";
import { TopMenu } from "@s2script/topmenu";

const gagged = new Set<string>(); // SteamIDs — chat suppressed
const muted = new Set<string>();  // SteamIDs — voice mute requested (best-effort)

function forTargets(pat: string, callerSlot: number, reply: (m: string) => void, verb: string, usage: string, act: (p: Player) => void): void {
  if (!pat) { reply("Usage: " + usage); return; }
  const targets = Player.target(pat, callerSlot);
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
  p.hasCommunicationAbuseMute = on; // best-effort schema write (see header)
  const sid = p.steamId;
  if (!sid) return;
  if (on) muted.add(sid); else muted.delete(sid);
}

export function onLoad(): void {
  // Suppress chat from a gagged speaker (both say and say_team route through Host_Say).
  Chat.onMessage((slot, _text, _teamonly) => {
    if (gagged.size === 0) return HookResult.Continue;
    const p = Player.fromSlot(slot);
    const sid = p ? p.steamId : null;
    return sid && gagged.has(sid) ? HookResult.Handled : HookResult.Continue;
  });

  Commands.registerAdmin("sm_gag", ADMFLAG.CHAT, (ctx) =>
    forTargets(ctx.arg(0), ctx.callerSlot, (m) => ctx.reply(m), "Gagged", "sm_gag <target>", (p) => setGag(p, true)));
  Commands.registerAdmin("sm_ungag", ADMFLAG.CHAT, (ctx) =>
    forTargets(ctx.arg(0), ctx.callerSlot, (m) => ctx.reply(m), "Ungagged", "sm_ungag <target>", (p) => setGag(p, false)));
  Commands.registerAdmin("sm_mute", ADMFLAG.CHAT, (ctx) =>
    forTargets(ctx.arg(0), ctx.callerSlot, (m) => ctx.reply(m), "Muted", "sm_mute <target>", (p) => setMute(p, true)));
  Commands.registerAdmin("sm_unmute", ADMFLAG.CHAT, (ctx) =>
    forTargets(ctx.arg(0), ctx.callerSlot, (m) => ctx.reply(m), "Unmuted", "sm_unmute <target>", (p) => setMute(p, false)));
  Commands.registerAdmin("sm_silence", ADMFLAG.CHAT, (ctx) =>
    forTargets(ctx.arg(0), ctx.callerSlot, (m) => ctx.reply(m), "Silenced", "sm_silence <target>", (p) => { setGag(p, true); setMute(p, true); }));
  Commands.registerAdmin("sm_unsilence", ADMFLAG.CHAT, (ctx) =>
    forTargets(ctx.arg(0), ctx.callerSlot, (m) => ctx.reply(m), "Unsilenced", "sm_unsilence <target>", (p) => { setGag(p, false); setMute(p, false); }));

  // adminmenu — Gag proof item, same ADMFLAG as sm_gag, via pickPlayer + the shared setGag routine.
  TopMenu.addItem("Player Commands", { id: "basecomm:gag", name: "Gag", flags: ADMFLAG.CHAT,
    onSelect: adminSlot => pickPlayer(adminSlot, t => setGag(t, true)) });

  console.log("[basecomm] onLoad - gag/ungag/mute/unmute/silence/unsilence registered");
}

export function onUnload(): void { console.log("[basecomm] onUnload"); }
