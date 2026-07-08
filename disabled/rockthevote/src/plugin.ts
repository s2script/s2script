// @s2script/rockthevote — SourceMod rockthevote: chat `rtv`/`sm_forcertv` starts a map vote once
// enough connected non-bot players have asked, at a configurable turnout threshold; the winner
// changes the map at the end of the round.
//
// Task 1 (this file, current state): the plugin scaffold, the chat/command trigger, the turnout
// threshold, per-map state reset, and disconnect cleanup. `startVote` is an intentional STUB here
// (logs + flips `voteRunning`) — Task 2 fills in the ballot/vote/round_end-apply body.

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Chat } from "@s2script/chat";
import { HookResult } from "@s2script/events";
import { Clients } from "@s2script/clients";
import { OnGameFrame } from "@s2script/frame";
import { Server } from "@s2script/server";
import { config } from "@s2script/config";

/** A map option: its stock/BSP name, or a workshop id (mutually informative — see Task 2's ballot). */
interface MapEntry { name: string; workshopId: string | null; }

// --- module state (persists across a changelevel — see pollMapChange below) ---
const rtvVoters: Set<number> = new Set();
let voteRunning = false;
let votedThisMap = false;
let pendingMap: MapEntry | null = null;
let currentMap = "";
let frameCounter = 0; // throttles the map-change poll to ~once/sec

/** Connected non-bot player count (bots are skipped everywhere in RTV). */
function playerCount(): number {
  return Clients.all().filter(c => !c.isBot).length;
}

/**
 * Start (or force) the RTV map vote. Task 1 STUB: logs + marks a vote running so the trigger
 * threshold logic + sm_forcertv have something real to flip. Task 2 replaces this body with the
 * ballot build + @s2script/votes Vote.start(...) + the round_end apply.
 */
function startVote(force: boolean): void {
  console.log("[rockthevote] startVote force=" + force);
  voteRunning = true;
}

function requestRtv(slot: number): void {
  if (voteRunning || votedThisMap) {
    Chat.toSlot(slot, voteRunning ? "[RTV] A vote is already running." : "[RTV] A vote already happened this map.");
    return;
  }
  const pc = playerCount();
  const need = Math.ceil(config.getFloat("rtv_threshold") * pc);
  if (rtvVoters.has(slot)) {
    Chat.toSlot(slot, "[RTV] You already RTV'd (need " + need + ").");
    return;
  }
  rtvVoters.add(slot);
  if (pc < config.getInt("rtv_min_players")) {
    Chat.toSlot(slot, "[RTV] Not enough players.");
    return;
  }
  if (rtvVoters.size >= need) {
    startVote(false);
  } else {
    Chat.toAll("[RTV] Player wants RTV (" + (need - rtvVoters.size) + " more needed)");
  }
}

// Plugins persist across a changelevel — the shim has no level-init reload hook, so onLoad fires
// once per plugin-load, NOT per map. Poll Server.mapName (throttled) to catch map transitions and
// reset all per-map RTV state (mirrors the nominations pattern).
function pollMapChange(): void {
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (!m || m === currentMap) return; // no change
  currentMap = m;
  rtvVoters.clear();
  voteRunning = false;
  votedThisMap = false;
  pendingMap = null;
}

export function onLoad(): void {
  OnGameFrame.subscribe(pollMapChange);

  Chat.onMessage((slot, text) => {
    const t = text.trim().toLowerCase();
    const bang = t === "!rtv" || t === "!rockthevote";
    const bare = t === "rtv" || t === "rockthevote";
    if (bang || bare) {
      const c = Clients.fromSlot(slot);
      if (c && !c.isBot) requestRtv(slot);
    }
    return bang ? HookResult.Handled : HookResult.Continue;
  });

  Commands.registerAdmin("sm_forcertv", ADMFLAG.CHANGEMAP, ctx => {
    startVote(true);
    ctx.reply("RTV forced.");
  });

  Clients.onDisconnect(c => rtvVoters.delete(c.slot));

  console.log("[rockthevote] onLoad — sm_forcertv + rtv registered");
}

export function onUnload(): void {
  console.log("[rockthevote] onUnload");
}
