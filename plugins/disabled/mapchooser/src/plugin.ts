// @s2script/mapchooser — SourceMod mapchooser: the end-of-map vote. A configurable time before
// mp_timelimit runs out (or rounds before mp_maxrounds), the server votes on the next map from the
// nominations plus a random maplist.txt fill, with an Extend Map option.
//
// It CHOOSES the next map; it does not change it. With @s2script/nextmap loaded the winner becomes
// nextmap's override and nextmap changes the map at the limit, exactly like sm_setnextmap. Without
// it, mapchooser sets `nextlevel` and changes the map itself when the match ends.

import {
  command, hook, translations, publish, watchOptional, ADMFLAG, Chat, HookResult, Vote, Server,
  config, Database, Translations,
} from "@s2script/sdk";
import type { TypedPublishHandle, VoteResult } from "@s2script/sdk";
import type { Contract } from "../api";

/** A map option: its stock/BSP name, or a workshop id. */
interface MapEntry { name: string; workshopId: string | null; }

/** The ballot's extend sentinel. Never translated, so it can never collide with a map name. */
const EXTEND = "Extend Map";

// --- per-map state (plugins persist across a changelevel; pollTick resets it) ---
let currentMap = "";
let roundsPlayed = 0;
let voteRunning = false;   // claimed synchronously before the ballot's DB reads
let voteFinished = false;  // a map won this map's vote
let extendsUsed = 0;
let pendingMap: MapEntry | null = null; // applied by us only when nextmap is absent
let frameCounter = 0;

let db!: Database;
let iface: TypedPublishHandle<Contract>;
/** The live nextmap service, or null while nextmap is not loaded. */
let nextmap: { setNextMap(map: string): boolean } | null = null;

const logErr = (e: unknown) => console.log("[mapchooser] error: " + e);

// maplist.txt parsing — the same format nextmap, nominations and rockthevote read.
function parseMaplist(text: string): MapEntry[] {
  const out: MapEntry[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("//") || line.startsWith("#")) continue;
    const i = line.indexOf(":");
    const name = i >= 0 ? line.slice(0, i).trim() : line;
    if (!name) continue;
    out.push({ name, workshopId: i >= 0 ? (line.slice(i + 1).trim() || null) : null });
  }
  return out;
}

function loadPool(): MapEntry[] {
  const text = config.readFile("maplist.txt");
  return text === null ? [] : parseMaplist(text);
}

/** Injection guard: the name/id reaches Server.command and the console splits on `;`. */
function isValidEntry(e: MapEntry): boolean {
  return /^[A-Za-z0-9_]+$/.test(e.name) && (e.workshopId === null || /^[0-9]+$/.test(e.workshopId));
}

function isCurrentMap(name: string): boolean {
  return currentMap !== "" && name.toLowerCase() === currentMap.toLowerCase();
}

function timeLimitMinutes(): number {
  const tl = parseFloat(Server.getCvar("mp_timelimit"));
  return tl > 0 ? tl : 0;
}

function maxRounds(): number {
  const max = parseInt(Server.getCvar("mp_maxrounds"), 10);
  return max > 0 ? max : 0;
}

/** Whether Extend can go on this ballot: allowed by config and there is a limit to extend. */
function canExtend(): boolean {
  return extendsUsed < Math.max(0, config.getInt("mapvote_extend")) && (timeLimitMinutes() > 0 || maxRounds() > 0);
}

async function cooldownSet(): Promise<Set<string>> {
  const rows = await db.query(
    "SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?",
    [Math.max(0, config.getInt("mapvote_exclude"))]
  );
  return new Set(rows.map(r => String(r.map)));
}

async function nominationList(): Promise<string[]> {
  const rows = await db.query("SELECT map FROM nominations ORDER BY rowid", []);
  return rows.map(r => String(r.map));
}

/**
 * The ballot: nominations first (in nomination order), then random maplist.txt maps that are not
 * the current map or on cooldown, up to mapvote_include, then Extend Map when it is allowed.
 */
async function buildBallot(): Promise<{ maps: MapEntry[]; extend: boolean }> {
  const extend = canExtend();
  const cap = Math.min(Math.max(1, config.getInt("mapvote_include")), extend ? 8 : 9);
  const pool = loadPool();
  const maps: MapEntry[] = [];
  const listed = (name: string) => maps.some(m => m.name === name);

  for (const name of await nominationList()) {
    if (maps.length >= cap) break;
    if (listed(name) || isCurrentMap(name)) continue;
    // Nominations store the name only; recover the workshop id from maplist.txt.
    maps.push(pool.find(m => m.name === name) ?? { name, workshopId: null });
  }

  if (maps.length < cap) {
    const cooldown = await cooldownSet();
    const fill = pool.filter(m => !cooldown.has(m.name) && !isCurrentMap(m.name) && !listed(m.name));
    for (let i = fill.length - 1; i > 0; i--) {
      const j = Math.floor(Math.random() * (i + 1));
      const tmp = fill[i]; fill[i] = fill[j]; fill[j] = tmp;
    }
    for (const m of fill) {
      if (maps.length >= cap) break;
      if (!listed(m.name)) maps.push(m);
    }
  }
  return { maps: maps.filter(isValidEntry), extend };
}

/** Make `entry` the next map: nextmap's override when it is loaded, otherwise our own change. */
function setNextMap(entry: MapEntry): void {
  voteFinished = true;
  Server.setCvar("nextlevel", entry.name);
  let handled = false;
  if (nextmap !== null) {
    try { handled = nextmap.setNextMap(entry.name); } catch (e) { logErr(e); }
  }
  pendingMap = handled ? null : entry;
  console.log("[mapchooser] next map: " + entry.name + (handled ? " (nextmap rotates)" : " (changing at match end)"));
  Chat.toAll(Translations.translate(-1, "Mapvote Next Map", entry.name));
}

function extendMap(): void {
  extendsUsed++;
  const tl = timeLimitMinutes();
  const max = maxRounds();
  const minutes = Math.max(0, config.getInt("extendmap_timestep"));
  const rounds = Math.max(0, config.getInt("extendmap_roundstep"));
  if (tl > 0) Server.setCvar("mp_timelimit", String(tl + minutes));
  if (max > 0) Server.setCvar("mp_maxrounds", String(max + rounds));
  console.log("[mapchooser] map extended (" + extendsUsed + " used): mp_timelimit " + tl + "+" + minutes + ", mp_maxrounds " + max + "+" + rounds);
  // The next window opens against the new limit, like SM.
  voteFinished = false;
  Chat.toAll(Translations.translate(-1, tl > 0 ? "Mapvote Extended Time" : "Mapvote Extended Rounds", tl > 0 ? minutes : rounds));
}

function finishVote(result: VoteResult, options: string[], maps: MapEntry[], map: string): void {
  voteRunning = false;
  if (currentMap !== map) return; // the map changed under the vote
  let winner = result.winner;
  if (winner === null && result.total > 0) {
    // A tie: choose among the tied leaders rather than throwing the vote away.
    const best = Math.max(...result.counts);
    const tied = result.counts.map((c, i) => (c === best ? i : -1)).filter(i => i >= 0);
    winner = tied[Math.floor(Math.random() * tied.length)]!;
    Chat.toAll(Translations.translate(-1, "Mapvote Tie Broken", options[winner]!));
  }
  if (winner === null) {
    // Nobody voted: the normal rotation stands.
    voteFinished = true;
    Chat.toAll(Translations.translate(-1, "Mapvote No Votes"));
    return;
  }
  const chosen = options[winner]!;
  if (chosen === EXTEND) { extendMap(); return; }
  const entry = maps.find(m => m.name === chosen);
  if (entry) setNextMap(entry);
}

/**
 * Start the vote. Returns false when it cannot start now (one is running or finished, another
 * plugin postponed it, or our own vote lock is held).
 */
function startVote(forced: boolean): boolean {
  if (voteRunning || Vote.isActive()) return false;
  if (!forced && voteFinished) return false;
  if (iface.dispatch("OnMapVoteStart", { currentMap }) >= HookResult.Handled) return false;
  voteRunning = true;
  const map = currentMap;
  buildBallot().then(({ maps, extend }) => {
    if (currentMap !== map) { voteRunning = false; return; }
    const options = maps.map(m => m.name);
    if (extend) options.push(EXTEND);
    if (options.length < 2) {
      voteRunning = false;
      // One map and no Extend: there is nothing to vote on, so it is simply next.
      if (maps.length === 1 && !extend) setNextMap(maps[0]!);
      else { voteFinished = true; console.log("[mapchooser] no maps available for the vote (check maplist.txt / mapvote_exclude)"); }
      return;
    }
    const started = Vote.start({
      question: Translations.translate(-1, "Mapvote Question"),
      options,
      duration: Math.max(5, config.getInt("mapvote_voteduration")),
      showLiveTally: true,
      onEnd: result => finishVote(result, options, maps, map),
    });
    if (!started) { voteRunning = false; return; }
    Chat.toAll(Translations.translate(-1, "Mapvote Ballot Header"));
    for (let i = 0; i < options.length; i++) {
      Chat.toAll(Translations.translate(-1, options[i] === EXTEND ? "Mapvote Option Extend" : "Mapvote Option Map", i + 1, options[i]!));
    }
    console.log("[mapchooser] vote started" + (forced ? " (forced)" : "") + ": " + JSON.stringify(options));
  }).catch(e => { voteRunning = false; logErr(e); });
  return true;
}

/** Seconds before the time limit at which the vote opens. Never so late that it outlasts the map. */
function startWindowSeconds(): number {
  const configured = Math.max(0, config.getFloat("mapvote_start")) * 60;
  return Math.max(configured, Math.max(5, config.getInt("mapvote_voteduration")) + 10);
}

function resetForMap(map: string): void {
  // A ballot left open by the old map would keep eating typed numbers on the new one.
  if (Vote.isActive()) Vote.cancel();
  currentMap = map;
  roundsPlayed = 0;
  voteRunning = false;
  voteFinished = false;
  extendsUsed = 0;
  pendingMap = null;
}

function pollTick(): void {
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (!m) return;
  if (m !== currentMap) { resetForMap(m); return; }
  if (!config.getBool("mapvote_endvote") || voteRunning || voteFinished) return;
  const tl = timeLimitMinutes();
  // Same clock nextmap uses to decide the time limit has run out.
  if (tl > 0 && tl * 60 - Server.gameTime <= startWindowSeconds()) startVote(false);
}

export async function OnPluginStart(): Promise<void> {
  translations.load("mapchooser", "common");

  db = await Database.open("mapvote");
  // The schema nominations and rockthevote share — idempotent, so any load order works.
  await db.execute("CREATE TABLE IF NOT EXISTS map_history(id INTEGER PRIMARY KEY AUTOINCREMENT, map TEXT NOT NULL, played_at INTEGER NOT NULL)", []);
  await db.execute("CREATE TABLE IF NOT EXISTS nominations(map TEXT PRIMARY KEY, nominator INTEGER NOT NULL)", []);

  iface = publish("@s2script/mapchooser", {
    isVoteInProgress: () => voteRunning,
    hasEndOfMapVoteFinished: () => voteFinished,
  });

  watchOptional("@s2script/nextmap", (service, scope) => {
    nextmap = service;
    scope.own({ dispose() { nextmap = null; } });
    // The limit was reached while our vote is still open (short rounds, or a late sm_mapvote):
    // hold the rotation. nextmap retries, and picks up our winner through setNextMap.
    scope.own(service.on("OnAutoMapChange", () => (voteRunning ? HookResult.Handled : HookResult.Continue)));
  });

  hook.on("round_end", () => {
    roundsPlayed++;
    const max = maxRounds();
    if (max === 0 || !config.getBool("mapvote_endvote") || voteRunning || voteFinished) return;
    // At least one round ahead, so the vote closes before nextmap rotates at the final round_end.
    if (max - roundsPlayed <= Math.max(1, config.getInt("mapvote_startround"))) startVote(false);
  });

  // Only reached without nextmap: nextmap otherwise owns the change.
  hook.on("cs_win_panel_match", () => {
    const m = pendingMap;
    pendingMap = null;
    if (m === null || Server.mapName !== currentMap) return;
    Server.command(m.workshopId ? "host_workshop_map " + m.workshopId : "changelevel " + m.name);
  });

  command.admin("sm_mapvote", ADMFLAG.CHANGEMAP, cmd => {
    cmd.replyT(startVote(true) ? "Mapvote Forced" : "Mapvote Cannot Start");
    return HookResult.Handled;
  });
}

export function OnGameFrame(): void {
  pollTick();
}
