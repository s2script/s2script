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
import { Translations } from "@s2script/sdk/translations";
import type { PhraseKey } from "@s2script/sdk/phrases";

const gagged = new Set<string>(); // SteamIDs — chat suppressed
const muted = new Set<string>();  // SteamIDs — voice mute requested (best-effort)

// Convention: filterImmunity=true for a punitive command (drops targets of higher immunity than the
// caller); filterImmunity=false for a reversal command (un-gag/un-mute/un-silence — no filter).
//
// usageKey/singularKey/pluralKey are phrase KEYS, not raw text: this helper is shared by all six
// commands below, so the key is a variable here rather than a literal. `PhraseKey` covers this
// plugin's phrase file plus the shared one, so a key that exists in neither is a typecheck error at
// each of the six call sites — including the dynamic ones, which no scan of literals could reach.
function forTargets(
  pat: string,
  callerSlot: number,
  reply: (m: string) => void,
  usageKey: PhraseKey,
  singularKey: PhraseKey,
  pluralKey: PhraseKey,
  act: (p: Player) => void,
  filterImmunity: boolean,
): void {
  if (!pat) { reply(Translations.translate(callerSlot, usageKey)); return; }
  const targets = Player.target(pat, callerSlot, filterImmunity);
  if (targets.length === 0) { reply(Translations.translate(callerSlot, "No matching players")); return; }
  for (const p of targets) act(p);
  reply(Translations.translate(callerSlot, targets.length === 1 ? singularKey : pluralKey, targets.length));
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
  ctx.translations.load("basecomm", "common");

  // Own set FIRST, common SECOND: within each of translate's two passes (client language, then
  // English) the first hit wins, so this order makes a plugin's own phrase beat a shared one at
  // the same tier.

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
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Gag", "Gagged Player", "Gagged Players", (p) => setGag(p, true), true));
  ctx.commands.registerAdmin("sm_ungag", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Ungag", "Ungagged Player", "Ungagged Players", (p) => setGag(p, false), false));
  ctx.commands.registerAdmin("sm_mute", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Mute", "Muted Player", "Muted Players", (p) => setMute(p, true), true));
  ctx.commands.registerAdmin("sm_unmute", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Unmute", "Unmuted Player", "Unmuted Players", (p) => setMute(p, false), false));
  ctx.commands.registerAdmin("sm_silence", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Silence", "Silenced Player", "Silenced Players", (p) => { setGag(p, true); setMute(p, true); }, true));
  ctx.commands.registerAdmin("sm_unsilence", ADMFLAG.CHAT, (cmd) =>
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Unsilence", "Unsilenced Player", "Unsilenced Players", (p) => { setGag(p, false); setMute(p, false); }, false));

  // adminmenu — Gag proof item, same ADMFLAG as sm_gag, via pickPlayer + the shared setGag routine.
  // `name` is a static field set once here, before any admin has opened the menu, so — same as
  // basecommands' "Change Map Item" — it can only resolve at the server default language (-1), not
  // per-viewer.
  ctx.topmenu.addItem("Player Commands", { id: "basecomm:gag", name: Translations.translate(-1, "Gag Item"), flags: ADMFLAG.CHAT,
    onSelect: adminSlot => pickPlayer(adminSlot, t => setGag(t, true)) });

  console.log("[basecomm] onLoad - gag/ungag/mute/unmute/silence/unsilence registered");
});
