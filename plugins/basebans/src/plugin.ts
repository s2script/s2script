// @s2script/basebans — SourceMod basebans: sm_ban / sm_unban / sm_addban.
//
//  - BAN (sm_ban): resolves a live target by SM target string, validates SteamID (rejects bots/unauth
//    whose steamId === "0"), writes the ban to the host-global store (persisted to bans.json), and kicks
//    the player from the server. NO_MULTI: banning is destructive — @all / name-ambiguous matches are
//    refused; the caller must use #<userid> or a unique name.
//  - UNBAN (sm_unban): removes a ban by SteamID64. No live player needed — offline bans supported.
//  - ADDBAN (sm_addban): offline ban by SteamID64 without a live player (e.g. from logs or a roster).
//
//  Connect enforcement: a banned SteamID64 is rejected at connect time by the shim's ClientConnect hook
//  (engine-side, in core). This plugin manages the ban list and kicks the currently-online player; the
//  ClientConnect reject is handled automatically by the engine layer with NO per-plugin wiring.

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Bans } from "@s2script/bans";
import { Player } from "@s2script/cs2";

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
    p.kick("Banned: " + (reason || "No reason"));

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

  console.log("[basebans] onLoad - sm_ban/sm_unban/sm_addban registered");
}

export function onUnload(): void { console.log("[basebans] onUnload"); }
