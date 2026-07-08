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

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Bans } from "@s2script/bans";
import { Clients } from "@s2script/clients";
import { Player, pickPlayer } from "@s2script/cs2";
import { Menu, MenuStyle } from "@s2script/menu";
import { TopMenu } from "@s2script/topmenu";

// The message a banned player sees (chat + console) — shared by the immediate sm_ban path and the
// reconnect enforcement so the wording is identical.
function banMessage(reason: string, until: number): string {
  const now = Date.now() / 1000;
  const expiry = until === 0 ? "permanent" : "expires in " + Math.ceil((until - now) / 60) + " min";
  return "[SM] You are banned: " + (reason || "No reason") + " (" + expiry + ")";
}

export function onLoad(): void {
  // sm_ban <target> <minutes> [reason] — ADMFLAG.BAN
  // Resolves the target live, validates the SteamID, adds the ban, and kicks the player.
  // NO_MULTI: banning is destructive — a single target only.
  Commands.registerAdmin("sm_ban", ADMFLAG.BAN, (ctx) => {
    const target = ctx.arg(0);
    if (!target) {
      ctx.reply("[SM] Usage: sm_ban <#userid|name> <minutes> [reason]");
      return;
    }
    if (!/^\d+$/.test(ctx.arg(1))) {
      // A missing OR non-numeric minutes arg must NOT silently become a permanent ban
      // (argInt falls back to 0 = permanent for NaN). Require explicit digits; "0" = permanent.
      ctx.reply("[SM] Usage: sm_ban <#userid|name> <minutes> [reason]");
      return;
    }
    const minutes = ctx.argInt(1);
    const reason = ctx.argsFrom(2);

    const targets = Player.target(target, ctx.callerSlot);
    if (targets.length === 0) {
      ctx.reply("[SM] No matching players.");
      return;
    }
    // NO_MULTI: banning is destructive — single target only, do NOT allow @all or ambiguous names.
    if (targets.length > 1) {
      ctx.reply("[SM] '" + target + "' matches multiple players; be more specific.");
      return;
    }

    const p = targets[0];
    const sid = p.steamId;
    if (!sid || sid === "0") {
      ctx.reply("[SM] Cannot ban " + p.playerName + " (no SteamID — bot or unauthenticated).");
      return;
    }

    Bans.add(sid, minutes, reason);
    // Show the reason (chat + console, repeated) then kick — the player is online/in-game, so
    // kickWithReason delivers immediately. (A plain kick would disconnect them with no reason shown.)
    const b = Bans.get(sid);
    const c = Clients.fromSlot(p.slot);
    if (c) c.kickWithReason(banMessage(reason, b ? b.until : 0));
    else p.kick("Banned: " + (reason || "No reason"));   // fallback: no Client for the slot

    const durStr = minutes > 0
      ? " for " + minutes + " minute" + (minutes === 1 ? "" : "s")
      : " permanently";
    const reasonStr = reason ? " (" + reason + ")" : "";
    ctx.reply("[SM] Banned " + p.playerName + durStr + reasonStr + ".");
  });

  // sm_unban <steamid> — ADMFLAG.UNBAN
  // Removes a ban by SteamID64. No live player required — offline bans supported.
  Commands.registerAdmin("sm_unban", ADMFLAG.UNBAN, (ctx) => {
    const sid = ctx.arg(0);
    if (!/^\d+$/.test(sid)) {
      ctx.reply("[SM] Usage: sm_unban <steamid64>");
      return;
    }
    const was = Bans.remove(sid);
    ctx.reply(was ? "[SM] Unbanned " + sid + "." : "[SM] " + sid + " was not banned.");
  });

  // sm_addban <steamid> <minutes> [reason] — ADMFLAG.BAN
  // Adds an offline ban by SteamID64 without a live player (e.g. from logs or a server roster).
  Commands.registerAdmin("sm_addban", ADMFLAG.BAN, (ctx) => {
    const sid = ctx.arg(0);
    if (!/^\d+$/.test(sid)) {
      ctx.reply("[SM] Usage: sm_addban <steamid64> <minutes> [reason]");
      return;
    }
    if (!/^\d+$/.test(ctx.arg(1))) {
      // Missing or non-numeric minutes → usage, not a silent permanent ban (see sm_ban).
      ctx.reply("[SM] Usage: sm_addban <steamid64> <minutes> [reason]");
      return;
    }
    const minutes = ctx.argInt(1);
    const reason = ctx.argsFrom(2);

    Bans.add(sid, minutes, reason);

    const durStr = minutes > 0 ? " (" + minutes + " min)" : " (permanent)";
    const reasonStr = reason ? " " + reason : "";
    ctx.reply("[SM] Added ban for " + sid + durStr + reasonStr + ".");
  });

  // Connect-time enforcement: admit -> show reason (chat + console) -> kick. Runs for every connecting
  // client; a banned SteamID64 gets kickWithReason (delivered once they're in-game, then kicked ~5s later).
  // A 3rd-party ban system would register its OWN Clients.onConnect here, querying its store instead of Bans.
  Clients.onConnect((c) => {
    if (c.isBot) return;                                   // bots have steamId "0" — never banned
    const b = Bans.get(c.steamId);
    if (!b) return;
    const now = Date.now() / 1000;
    if (b.until !== 0 && b.until <= now) return;           // expired — let them in
    c.kickWithReason(banMessage(b.reason, b.until));
  });

  // adminmenu — Kick + Ban proof items, same ADMFLAG as their text commands, via pickPlayer.
  TopMenu.addItem("Player Commands", { id: "basebans:kick", name: "Kick", flags: ADMFLAG.KICK,
    onSelect: adminSlot => pickPlayer(adminSlot, t => t.kick("Kicked by admin")) });
  TopMenu.addItem("Player Commands", { id: "basebans:ban", name: "Ban", flags: ADMFLAG.BAN,
    onSelect: adminSlot => pickPlayer(adminSlot, t => {
      const sid = t.steamId, uid = t.userId, name = t.playerName || "player";
      if (!sid || sid === "0") {   // bot / unauthenticated — never ban (sm_ban parity: a "0" entry is shared)
        const admin = Clients.fromSlot(adminSlot);
        if (admin) admin.chat("Cannot ban " + name + " (bot / not authenticated)");
        return;
      }
      const dm = new Menu("Ban " + name + " for");
      dm.style = MenuStyle.Center;
      dm.freezePlayer = true;   // WASD nav — keep the admin frozen through the duration sub-menu
      const mins = [0, 5, 30, 60];   // 0 = permanent
      for (const m of mins) dm.addItem(String(m), m === 0 ? "Permanent" : (m + " min"));
      dm.onSelect(e => {
        const minutes = parseInt(e.info, 10);
        Bans.add(sid, minutes, "Banned by admin");   // ban record is keyed by SteamID — always correct
        const b = Bans.get(sid);
        // Re-resolve by userId at kick time: the target may have left (and the slot been reused) between
        // the player pick and the duration pick — only kick if the SAME player is still connected.
        const cur = Player.fromUserId(uid);
        if (cur && cur.steamId === sid) {
          const c = Clients.fromSlot(cur.slot);
          if (c) c.kickWithReason(banMessage("Banned by admin", b ? b.until : 0));
          else cur.kick("Banned by admin");
        }
        // else: they left / the slot was reused — the persisted ban + reconnect enforcement handles it.
      });
      dm.display(adminSlot, 30);
    }) });

  console.log("[basebans] onLoad - sm_ban/sm_unban/sm_addban + connect enforcement registered");
}

export function onUnload(): void { console.log("[basebans] onUnload"); }
