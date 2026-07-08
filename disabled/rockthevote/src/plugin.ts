// @s2script/rockthevote — SourceMod rockthevote: chat `rtv`/`sm_forcertv` starts a map vote once
// enough connected non-bot players have asked, at a configurable turnout threshold; the winner
// changes the map at the end of the round.
//
// Task 1: the plugin scaffold, the chat/command trigger, the turnout threshold, per-map state
// reset, and disconnect cleanup.
// Task 2 (this file, current state): the ballot (nominations + random pool-fill − cooldown +
// "Don't Change"), the @s2script/votes vote, and the round_end map-change apply (workshop/stock).

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Chat } from "@s2script/chat";
import { Events, HookResult } from "@s2script/events";
import type { VoteResult } from "@s2script/votes";
import { Vote } from "@s2script/votes";
import { Clients } from "@s2script/clients";
import { OnGameFrame } from "@s2script/frame";
import { Server } from "@s2script/server";
import { config } from "@s2script/config";
import { Database } from "@s2script/db";

/** A map option: its stock/BSP name, or a workshop id (mutually informative — see the ballot). */
interface MapEntry { name: string; workshopId: string | null; }

/** The non-map ballot sentinel (SM "Don't Change") — one literal, referenced by both build + finish. */
const DONT_CHANGE = "Don't Change";

// --- module state (persists across a changelevel — see pollMapChange below) ---
const rtvVoters: Set<number> = new Set();
let voteRunning = false;
let votedThisMap = false;
let pendingMap: MapEntry | null = null;
let currentMap = "";
let frameCounter = 0; // throttles the map-change poll to ~once/sec

// The shared "mapvote" SQLite DB (nominations owns the schema/tables — this plugin only reads).
let db: Database | null = null;

const logErr = (e: unknown) => console.log("[rockthevote] error: " + e);

/** Connected non-bot player count (bots are skipped everywhere in RTV). */
function playerCount(): number {
  return Clients.all().filter(c => !c.isBot).length;
}

// maplist.txt parsing — duplicated from nominations (colon-split "name:workshopId", `//`/`#`/blank
// skip, skip an empty-name entry). Read-only here: nominations owns auto-generation.
function parseMaplist(text: string): MapEntry[] {
  const out: MapEntry[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("//") || line.startsWith("#")) continue;
    const i = line.indexOf(":");
    const name = i >= 0 ? line.slice(0, i).trim() : line;
    if (!name) continue;   // skip a malformed ":123" (empty name) entry
    out.push({ name, workshopId: i >= 0 ? (line.slice(i + 1).trim() || null) : null });
  }
  return out;
}

function loadPool(): MapEntry[] {
  const text = config.readFile("maplist.txt");
  if (text === null) {
    console.log("[rockthevote] maplist.txt not found — treating the pool as empty (nominations owns generation)");
    return [];
  }
  return parseMaplist(text);
}

/** Maps that must NOT be offered yet — the most-recently-played `rtv_cooldown` distinct maps. */
async function cooldownSet(): Promise<Set<string>> {
  if (!db) return new Set();
  const rows = await db.query(
    "SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?",
    [Math.max(0, config.getInt("rtv_cooldown"))]
  );
  return new Set(rows.map(r => String(r.map)));
}

/** This map's live nominations, in nomination order. */
async function nominationList(): Promise<string[]> {
  if (!db) return [];
  const rows = await db.query("SELECT map FROM nominations ORDER BY rowid", []);
  return rows.map(r => String(r.map));
}

/**
 * Build the RTV ballot: nominations first (in order, truncated to the cap), then a random
 * pool-fill (excluding the cooldown set + already-listed nominations) up to the cap, then the
 * literal "Don't Change" option. Returns null if there are zero map options to offer (a
 * "Don't Change"-only vote is pointless — the caller aborts instead of starting one).
 */
async function buildBallot(): Promise<{ options: string[]; entries: Map<string, MapEntry> } | null> {
  const cap = Math.min(Math.max(1, config.getInt("rtv_map_count")), 8); // ballot is 2..9 (Don't Change takes a slot)
  const entries = new Map<string, MapEntry>();
  const options: string[] = [];

  for (const name of await nominationList()) {
    if (options.length >= cap) break;
    if (entries.has(name)) continue;
    options.push(name);
    entries.set(name, { name, workshopId: null });
  }

  if (options.length < cap) {
    const cooldown = await cooldownSet();
    const pool = loadPool().filter(m => !cooldown.has(m.name) && !entries.has(m.name));
    // Fisher-Yates shuffle.
    for (let i = pool.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      const tmp = pool[i]; pool[i] = pool[j]; pool[j] = tmp;
    }
    for (const m of pool) {
      if (options.length >= cap) break;
      if (entries.has(m.name)) continue;   // a duplicate maplist.txt entry must not split the vote
      options.push(m.name);
      entries.set(m.name, m);
    }
  }

  if (options.length === 0) return null;

  options.push(DONT_CHANGE);
  return { options, entries };
}

/** Apply the vote's result: map the winning index back to its display string, then either keep
 *  the map (tie / no votes / "Don't Change") or stage the winner for the round_end apply. */
function finishVote(result: VoteResult, options: string[], entries: Map<string, MapEntry>): void {
  voteRunning = false;   // votedThisMap is set only when a map actually wins (below) — a tie /
                         // "Don't Change" / invalid winner leaves RTV open to re-accumulate (SM parity)

  const chosen = result.winner === null ? null : options[result.winner];
  if (chosen === null || chosen === DONT_CHANGE) {
    Chat.toAll(chosen === null ? "[RTV] Vote tied — map stays" : "[RTV] Don't Change won — map stays");
    return;
  }

  const entry = entries.get(chosen) ?? { name: chosen, workshopId: null };
  if (!/^[A-Za-z0-9_]+$/.test(entry.name) || (entry.workshopId !== null && !/^[0-9]+$/.test(entry.workshopId))) {
    console.log("[rockthevote] winner failed validation: " + JSON.stringify(entry));
    Chat.toAll("[RTV] winner invalid — map unchanged");
    return;
  }

  pendingMap = entry;
  votedThisMap = true;   // a change is queued for round end — block further RTV until the map changes
  Chat.toAll("[RTV] " + chosen + " won — changing at the end of the round");
}

/** Start (or force) the RTV map vote: build the ballot, then hand it to @s2script/votes.
 *  Returns true if the request was accepted (the lock claimed); false if a vote is already active. */
function startVote(force: boolean): boolean {
  if (voteRunning || Vote.isActive()) return false;
  voteRunning = true;   // claim synchronously — closes the guard window so a concurrent requestRtv (buildBallot awaits the DB) can't double-start
  buildBallot().then(ballot => {
    if (ballot === null) {
      Chat.toAll("[RTV] No maps available to vote on");
      voteRunning = false;      // release — nothing started
      votedThisMap = true;
      return;
    }
    rtvVoters.clear();
    const { options, entries } = ballot;
    Vote.start({
      question: "RockTheVote",
      options,
      duration: config.getInt("rtv_vote_duration"),
      showLiveTally: config.getBool("rtv_show_tally"),
      onEnd: (result) => finishVote(result, options, entries),
    });
    console.log("[rockthevote] startVote force=" + force + " options=" + JSON.stringify(options));
  }).catch(e => { voteRunning = false; logErr(e); });   // release the lock on a build error too
  return true;
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
// reset all per-map RTV state (mirrors the nominations pattern); also re-evaluate the RTV threshold
// (a disconnect lowers the denominator but the per-player path can't re-trigger — SM re-checks on
// disconnect; we do it on the settled ~1s tick to avoid racing the disconnect event).
function pollTick(): void {
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (m && m !== currentMap) { // map changed — reset all per-map RTV state
    currentMap = m;
    rtvVoters.clear();
    voteRunning = false;
    votedThisMap = false;
    pendingMap = null;
    return;
  }
  if (!voteRunning && !votedThisMap && rtvVoters.size > 0) {
    const pc = playerCount();
    if (pc > 0 && rtvVoters.size >= Math.ceil(config.getFloat("rtv_threshold") * pc)) startVote(false);
  }
}

export function onLoad(): void {
  OnGameFrame.subscribe(pollTick);

  Database.open("mapvote").then(d => { db = d; }).catch(logErr); // nominations owns CREATE TABLE

  Events.on("round_end", () => {
    if (!pendingMap) return;
    const m = pendingMap;
    Server.command(m.workshopId ? "host_workshop_map " + m.workshopId : "changelevel " + m.name);
    pendingMap = null;
  });

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
    ctx.reply(startVote(true) ? "RTV forced." : "A vote is already running.");
  });

  Clients.onDisconnect(c => rtvVoters.delete(c.slot));

  console.log("[rockthevote] onLoad — sm_forcertv + rtv registered");
}

export function onUnload(): void {
  console.log("[rockthevote] onUnload");
}
