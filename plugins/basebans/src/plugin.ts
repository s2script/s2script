// @s2script/basebans — SourceMod basebans: sm_ban / sm_unban / sm_addban.
//
//  - BAN (sm_ban): resolves a live target by SM target string, validates SteamID (rejects bots/unauth
//    whose steamId === "0"), writes the ban to the host-global store (persisted to bans.json), and kicks
//    the player from the server. NO_MULTI: banning is destructive — @all / name-ambiguous matches are
//    refused; the caller must use #<userid> or a unique name.
//  - UNBAN (sm_unban): removes a ban by SteamID64. No live player needed — offline bans supported.
//  - ADDBAN (sm_addban): offline ban by SteamID64 without a live player (e.g. from logs or a roster).
//
//  Connect enforcement (sub-project 3): a banned SteamID64 is NOT instant-rejected at connect anymore —
//  the shim admits every client, and this plugin enforces the ban in JS via Clients.onConnect, showing the
//  reason (chat + console) then kicking (Client.kickWithReason). sm_ban still kicks the ONLINE player
//  immediately; the onConnect handler is the RECONNECT enforcement + is where a 3rd party would query
//  their own ban store instead of ours.

import { plugin } from "@s2script/sdk/plugin";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Bans } from "@s2script/sdk/bans";
import { Clients } from "@s2script/sdk/clients";
import { Player, pickPlayer } from "@s2script/cs2";
import { Menu, MenuStyle } from "@s2script/sdk/menu";
import { Translations } from "@s2script/sdk/translations";
import { phrases } from "./phrases";

// The message a banned player sees (chat + console) — shared by the immediate sm_ban path and the
// reconnect enforcement so the wording is identical. `slot` is the BANNED PLAYER (the recipient of
// this message), translated in THEIR language — never the admin who issued the ban. No colour tags
// anywhere in here: kickWithReason delivers via Client.chat/Client.print, which never run through
// the colour-expanding chat/console funnels (see phrases.ts).
function banMessage(slot: number, reason: string, until: number): string {
  const now = Date.now() / 1000;
  const expiry = until === 0
    ? Translations.translate(slot, "Ban Expiry Permanent")
    : Translations.translate(slot, "Ban Expiry Minutes", Math.ceil((until - now) / 60));
  const reasonText = reason || Translations.translate(slot, "Ban Reason Default");
  return Translations.translate(slot, "Ban Message", reasonText, expiry);
}

export default plugin((ctx) => {
  // Own set FIRST, common SECOND: translate takes the first hit across sets, so this order is what
  // lets a plugin override a shared phrase.
  Translations.load("basebans", phrases);
  Translations.load("common");

  // sm_ban <target> <minutes> [reason] — ADMFLAG.BAN
  // Resolves the target live, validates the SteamID, adds the ban, and kicks the player.
  // NO_MULTI: banning is destructive — a single target only.
  ctx.commands.registerAdmin("sm_ban", ADMFLAG.BAN, (cmd) => {
    const target = cmd.arg(0);
    if (!target) {
      cmd.replyT("Usage Ban");
      return;
    }
    if (!/^\d+$/.test(cmd.arg(1))) {
      // A missing OR non-numeric minutes arg must NOT silently become a permanent ban
      // (argInt falls back to 0 = permanent for NaN). Require explicit digits; "0" = permanent.
      cmd.replyT("Usage Ban");
      return;
    }
    const minutes = cmd.argInt(1);
    const reason = cmd.argsFrom(2);

    const targets = Player.target(target, cmd.callerSlot, true);
    if (targets.length === 0) {
      cmd.replyT("No matching players");
      return;
    }
    // NO_MULTI: banning is destructive — single target only, do NOT allow @all or ambiguous names.
    if (targets.length > 1) {
      cmd.replyT("Ban Ambiguous Target", target);
      return;
    }

    const p = targets[0];
    const sid = p.steamId;
    if (!sid || sid === "0") {
      cmd.replyT("Cannot Ban No Steamid", p.playerName ?? "");
      return;
    }

    Bans.add(sid, minutes, reason);
    // Show the reason (chat + console, repeated) then kick — the player is online/in-game, so
    // kickWithReason delivers immediately. (A plain kick would disconnect them with no reason shown.)
    const b = Bans.get(sid);
    const c = Clients.fromSlot(p.slot);
    if (c) c.kickWithReason(banMessage(p.slot, reason, b ? b.until : 0));
    else p.kick(Translations.translate(p.slot, "Kick Ban Reason Fallback", reason || Translations.translate(p.slot, "Ban Reason Default")));   // fallback: no Client for the slot

    const durText = minutes > 0
      ? Translations.translate(cmd.callerSlot, minutes === 1 ? "Ban Duration Minute" : "Ban Duration Minutes", minutes)
      : Translations.translate(cmd.callerSlot, "Ban Duration Permanently");
    const reasonText = reason ? Translations.translate(cmd.callerSlot, "Ban Reason Suffix", reason) : "";
    cmd.replyT("Ban Success", p.playerName ?? "", durText, reasonText);
  });

  // sm_unban <steamid> — ADMFLAG.UNBAN
  // Removes a ban by SteamID64. No live player required — offline bans supported.
  ctx.commands.registerAdmin("sm_unban", ADMFLAG.UNBAN, (cmd) => {
    const sid = cmd.arg(0);
    if (!/^\d+$/.test(sid)) {
      cmd.replyT("Usage Unban");
      return;
    }
    const was = Bans.remove(sid);
    cmd.replyT(was ? "Unban Success" : "Unban Not Banned", sid);
  });

  // sm_addban <steamid> <minutes> [reason] — ADMFLAG.BAN
  // Adds an offline ban by SteamID64 without a live player (e.g. from logs or a server roster).
  ctx.commands.registerAdmin("sm_addban", ADMFLAG.BAN, (cmd) => {
    const sid = cmd.arg(0);
    if (!/^\d+$/.test(sid)) {
      cmd.replyT("Usage Addban");
      return;
    }
    if (!/^\d+$/.test(cmd.arg(1))) {
      // Missing or non-numeric minutes → usage, not a silent permanent ban (see sm_ban).
      cmd.replyT("Usage Addban");
      return;
    }
    const minutes = cmd.argInt(1);
    const reason = cmd.argsFrom(2);

    Bans.add(sid, minutes, reason);

    const durText = minutes > 0
      ? Translations.translate(cmd.callerSlot, "Addban Duration Minutes", minutes)
      : Translations.translate(cmd.callerSlot, "Addban Duration Permanent");
    const reasonText = reason ? Translations.translate(cmd.callerSlot, "Addban Reason Suffix", reason) : "";
    cmd.replyT("Addban Success", sid, durText, reasonText);
  });

  // Connect-time enforcement: admit -> show reason (chat + console) -> kick. Runs for every connecting
  // client; a banned SteamID64 gets kickWithReason (delivered once they're in-game, then kicked ~5s later).
  // A 3rd-party ban system would register its OWN ctx.clients.onConnect here, querying its store instead of Bans.
  ctx.clients.onConnect((c) => {
    if (c.isBot) return;                                   // bots have steamId "0" — never banned
    const b = Bans.get(c.steamId);
    if (!b) return;
    const now = Date.now() / 1000;
    if (b.until !== 0 && b.until <= now) return;           // expired — let them in
    c.kickWithReason(banMessage(c.slot, b.reason, b.until));
  });

  // adminmenu — Kick + Ban proof items, same ADMFLAG as their text commands, via pickPlayer.
  ctx.topmenu.addItem("Player Commands", { id: "basebans:kick", name: "Kick", flags: ADMFLAG.KICK,
    onSelect: adminSlot => pickPlayer(adminSlot, t => t.kick(Translations.translate(t.slot, "Kick By Admin"))) });
  ctx.topmenu.addItem("Player Commands", { id: "basebans:ban", name: "Ban", flags: ADMFLAG.BAN,
    onSelect: adminSlot => pickPlayer(adminSlot, t => {
      const sid = t.steamId, uid = t.userId, name = t.playerName || "player";
      if (!sid || sid === "0") {   // bot / unauthenticated — never ban (sm_ban parity: a "0" entry is shared)
        const admin = Clients.fromSlot(adminSlot);
        // Client.chat is a raw pass-through (no colour funnel) — see "Cannot Ban Bot" in phrases.ts.
        if (admin) admin.chat(Translations.translate(adminSlot, "Cannot Ban Bot", name));
        return;
      }
      const dm = new Menu("Ban " + name + " for");
      dm.style = MenuStyle.Center;
      dm.freezePlayer = true;   // WASD nav — keep the admin frozen through the duration sub-menu
      const mins = [0, 5, 30, 60];   // 0 = permanent
      for (const m of mins) dm.addItem(String(m), m === 0 ? "Permanent" : (m + " min"));
      dm.onSelect(e => {
        const minutes = parseInt(e.info, 10);
        // The persisted ban record is always stored in canonical (untranslated) form — same as a
        // custom reason typed to sm_ban/sm_addban — and only translated at display time.
        Bans.add(sid, minutes, "Banned by admin");
        const b = Bans.get(sid);
        // Re-resolve by userId at kick time: the target may have left (and the slot been reused) between
        // the player pick and the duration pick — only kick if the SAME player is still connected.
        const cur = Player.fromUserId(uid);
        if (cur && cur.steamId === sid) {
          const c = Clients.fromSlot(cur.slot);
          const reason = Translations.translate(cur.slot, "Ban Reason By Admin");
          if (c) c.kickWithReason(banMessage(cur.slot, reason, b ? b.until : 0));
          else cur.kick(reason);
        }
        // else: they left / the slot was reused — the persisted ban + reconnect enforcement handles it.
      });
      dm.display(adminSlot, 30);
    }) });

  console.log("[basebans] onLoad - sm_ban/sm_unban/sm_addban + connect enforcement registered");
});
