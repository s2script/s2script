// @s2script/basecomm — SourceMod basecomm: communication control (gag/mute/silence + un-versions).
//
//  - GAG (chat): VERIFIED. A gagged speaker's say/say_team is suppressed server-side by returning
//    HookResult.Handled from OnClientSayCommand (the live-proven Host_Say path). Keyed by SteamID so a gag
//    doesn't follow a slot to a reconnecting player.
//  - MUTE (voice): REAL. Flips Client.voiceMuted — the shim's SetClientListening rewrite silences the
//    sender's outgoing voice for every receiver (the CSSharp/Swiftly mechanism; supersedes the old
//    best-effort m_bHasCommunicationAbuseMute plan). The schema flag is still written as a cosmetic
//    scoreboard indicator only. Keyed by SteamID and re-asserted on putinserver so a mute survives a
//    reconnect. sm_silence = gag + mute.

import { command, topmenu, translations, publish, ADMFLAG, HookResult, Clients, Translations } from "@s2script/sdk";
import type { Client, PhraseKey, HookResultValue, TypedPublishHandle } from "@s2script/sdk";
import { Player, pickPlayer } from "@s2script/cs2";
import type { Contract } from "../api";

const gagged = new Set<string>(); // SteamIDs — chat suppressed
const muted = new Set<string>();  // SteamIDs — BaseComm voice policy
let iface: TypedPublishHandle<Contract> | null = null;

const MAX_U64 = 18_446_744_073_709_551_615n;

function validSteamId(value: unknown): value is string {
  if (typeof value !== "string" || !/^[1-9][0-9]*$/.test(value)) return false;
  try { return BigInt(value) <= MAX_U64; } catch { return false; }
}

function livePlayer(steamId: string): Player | null {
  return Player.allConnected().find((player) => player.steamId === steamId) ?? null;
}

// Policy and live engine state are complete before notification. There is deliberately no
// post-notification engine write: if a listener re-enters setMuted, the nested request stays final.
function applyLiveMute(steamId: string, state: boolean): void {
  const player = livePlayer(steamId);
  if (!player) return;
  const client = Clients.fromSlot(player.slot);
  if (client && client.steamId === steamId) client.voiceMuted = state;
  if (player.steamId === steamId) player.hasCommunicationAbuseMute = state;
}

const basecomm = {
  isMuted(steamId: string): boolean {
    return validSteamId(steamId) && muted.has(steamId);
  },
  isGagged(steamId: string): boolean {
    return validSteamId(steamId) && gagged.has(steamId);
  },
  setMuted(steamId: string, state: boolean): boolean {
    if (!validSteamId(steamId) || typeof state !== "boolean") return false;
    const changed = muted.has(steamId) !== state;
    if (state) muted.add(steamId); else muted.delete(steamId);
    applyLiveMute(steamId, state);
    if (changed) iface?.emit("OnClientMuteChanged", { steamId, state });
    return muted.has(steamId) === state;
  },
  setGagged(steamId: string, state: boolean): boolean {
    if (!validSteamId(steamId) || typeof state !== "boolean") return false;
    const changed = gagged.has(steamId) !== state;
    if (state) gagged.add(steamId); else gagged.delete(steamId);
    if (changed) iface?.emit("OnClientGagChanged", { steamId, state });
    return gagged.has(steamId) === state;
  },
};

function setSilenced(steamId: string, state: boolean): boolean {
  // Both operations run independently. A mute notification may re-enter gag policy after the
  // first setter returned, so combined success is the final two-property state, not stale returns.
  basecomm.setGagged(steamId, state);
  basecomm.setMuted(steamId, state);
  return validSteamId(steamId)
    && basecomm.isGagged(steamId) === state
    && basecomm.isMuted(steamId) === state;
}

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
  act: (steamId: string) => boolean,
  filterImmunity: boolean,
): void {
  if (!pat) { reply(Translations.translate(callerSlot, usageKey)); return; }
  const targets = Player.target(pat, callerSlot, filterImmunity);
  if (targets.length === 0) { reply(Translations.translate(callerSlot, "No matching players")); return; }
  // Both command reply and translation use a slot. Snapshot its owner before notifications.
  // Player.fromSlot pawn-gates: dead/spectating admins are still connected actors.
  const actor = callerSlot < 0 ? null : Player.allConnected().find(player => player.slot === callerSlot);
  const actorIdentity = actor ? { userId: actor.userId, steamId: actor.steamId } : null;
  // Copy every target identity before the first policy notification invokes arbitrary consumers.
  const steamIds = targets.map((player) => player.steamId);
  let accepted = 0;
  for (const steamId of steamIds) if (act(steamId)) accepted++;
  if (callerSlot >= 0) {
    const current = Player.allConnected().find(player => player.slot === callerSlot);
    if (!actorIdentity || !current || current.userId !== actorIdentity.userId ||
        current.steamId !== actorIdentity.steamId) return;
  }
  reply(Translations.translate(callerSlot, accepted === 1 ? singularKey : pluralKey, accepted));
}

export function OnPluginStart(): void {
  translations.load("basecomm", "common");
  iface = publish("@s2script/basecomm", basecomm);

  // Own set FIRST, common SECOND: within each of translate's two passes (client language, then
  // English) the first hit wins, so this order makes a plugin's own phrase beat a shared one at
  // the same tier.

  command.admin("sm_gag", ADMFLAG.CHAT, (cmd) => {
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Gag", "Gagged Player", "Gagged Players", (steamId) => basecomm.setGagged(steamId, true), true);
    return HookResult.Handled;
  });
  command.admin("sm_ungag", ADMFLAG.CHAT, (cmd) => {
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Ungag", "Ungagged Player", "Ungagged Players", (steamId) => basecomm.setGagged(steamId, false), false);
    return HookResult.Handled;
  });
  command.admin("sm_mute", ADMFLAG.CHAT, (cmd) => {
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Mute", "Muted Player", "Muted Players", (steamId) => basecomm.setMuted(steamId, true), true);
    return HookResult.Handled;
  });
  command.admin("sm_unmute", ADMFLAG.CHAT, (cmd) => {
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Unmute", "Unmuted Player", "Unmuted Players", (steamId) => basecomm.setMuted(steamId, false), false);
    return HookResult.Handled;
  });
  command.admin("sm_silence", ADMFLAG.CHAT, (cmd) => {
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Silence", "Silenced Player", "Silenced Players", (steamId) => setSilenced(steamId, true), true);
    return HookResult.Handled;
  });
  command.admin("sm_unsilence", ADMFLAG.CHAT, (cmd) => {
    forTargets(cmd.arg(0), cmd.callerSlot, (m) => cmd.reply(m), "Usage Unsilence", "Unsilenced Player", "Unsilenced Players", (steamId) => setSilenced(steamId, false), false);
    return HookResult.Handled;
  });

  // adminmenu — Gag proof item, same ADMFLAG as sm_gag, via pickPlayer + the shared setGag routine.
  // `name` is a static field set once here, before any admin has opened the menu, so — same as
  // basecommands' "Change Map Item" — it can only resolve at the server default language (-1), not
  // per-viewer.
  topmenu.addTab({ id: "basecomm", title: "Comm" });
  topmenu.addItem("basecomm", { id: "basecomm:gag", name: Translations.translate(-1, "Gag Item"), flags: ADMFLAG.CHAT,
    onSelect: adminSlot => pickPlayer(adminSlot, target => { basecomm.setGagged(target.steamId, true); }) });
}

// Suppress chat from a gagged speaker (both say and say_team route through Host_Say).
export function OnClientSayCommand(slot: number, _text: string, _teamonly: boolean): HookResultValue {
  if (gagged.size === 0) return HookResult.Continue;
  const steamId = Clients.fromSlot(slot)?.steamId;
  return steamId && basecomm.isGagged(steamId) ? HookResult.Handled : HookResult.Continue;
}

// A muted player who reconnects gets a fresh slot with a cleared flag (shim slot hygiene) — re-assert
// the SteamID-keyed admin mute once their controller exists.
export function OnClientPutInServer(c: Client): void {
  const steamId = c.steamId;
  if (!basecomm.isMuted(steamId)) return;
  c.voiceMuted = true;
  const player = livePlayer(steamId);
  if (player && player.slot === c.slot && player.steamId === steamId)
    player.hasCommunicationAbuseMute = true;
}
