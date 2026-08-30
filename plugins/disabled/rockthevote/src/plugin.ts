// @s2script/rockthevote — SourceMod rockthevote: chat `rtv`/`sm_forcertv` starts a map vote once
// enough connected non-bot players have asked, at a configurable turnout threshold; the winner
// changes the map at the end of the round.
//
// Task 1: the plugin scaffold, the chat/command trigger, the turnout threshold, per-map state
// reset, and disconnect cleanup.
// Task 2 (this file, current state): the ballot (nominations + random pool-fill − cooldown +
// "Don't Change"), the @s2script/votes vote, and the round_end map-change apply (workshop/stock).

import {
  command, hook, translations, ADMFLAG, Chat, HookResult, UserMessages, Vote, Clients,
  Server, config, Database, Events, Translations,
} from "@s2script/sdk";
import type { HookResultValue, VoteResult, Client } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

/** A map option: its stock/BSP name, or a workshop id (mutually informative — see the ballot). */
interface MapEntry { name: string; workshopId: string | null; }

// Colour is now carried entirely by tags inside phrases.ts (own {green}[RTV]{grey}/{lime}/{yellow}/
// {lightred} per phrase) — see that file's header comment for the mapping from the original
// ChatColors constants that used to live here.

/** The non-map ballot sentinel (SM "Don't Change") — one literal, referenced by both build + finish.
 *  Deliberately NEVER translated — see phrases.ts's header note. */
const DONT_CHANGE = "Don't Change";

// --- module state (persists across a changelevel — see pollMapChange below) ---
const rtvVoters: Set<number> = new Set();
let voteRunning = false;
let votedThisMap = false;
let pendingMap: MapEntry | null = null;
let currentMap = "";
let frameCounter = 0; // throttles the map-change poll to ~once/sec
let mapStartMs = Date.now(); // when the current map began — the rtv_initialdelay window counts from here

const logErr = (e: unknown) => console.log("[rockthevote] error: " + e);

/** Connected non-bot player count (bots are skipped everywhere in RTV). */
function playerCount(): number {
  return Clients.all().filter(c => !c.isBot).length;
}

/** Display name for `slot`, falling back to a neutral noun so a line never reads "undefined". */
function playerName(slot: number): string {
  const p = Player.fromSlot(slot);
  const n = p === null ? null : p.playerName;
  return n !== null && n !== undefined && n !== "" ? n : "A player";
}

// --- the live tally HUD -------------------------------------------------------------------------
// @s2script/votes exposes a renderer SEAM but ships no renderer, so `showLiveTally` did nothing
// except log "no tally renderer registered — chat-only" and every vote ran blind: a player saw the
// ballot scroll past once and had no idea what the count was or how long was left.
//
// The centre-screen surface in CS2 is the `show_survival_respawn_status` game event, whose
// `loc_token` renders as HTML. It paints for exactly ONE FRAME, while the vote ticks the renderer
// at 1 Hz — so `show()` cannot fire the event itself or the panel would flash once a second and
// read as broken. It records what should be on screen and the per-frame handler re-fires it.
let hudHtml = "";

/** Escape text that will be interpolated into the HUD's HTML (map names are operator-supplied). */
function esc(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// --- the chat ballot ----------------------------------------------------------------------------
// @s2script/votes announces a vote as ONE line — `"[Vote] " + question + " — 1=a, 2=b, 3=c..."` —
// built in the core. With five map names on it that wraps across most of the chat box and is
// genuinely hard to read, which is the opposite of what a ballot needs to be.
//
// The core line cannot be changed from a plugin, but it CAN be suppressed: it goes out as a
// SayText2 user message like any other chat, and `UserMessages.onPre` can drop a send. The flag is
// held only across the synchronous `Vote.start` call that emits it, so nothing else on the server
// loses a chat line — and it is matched on content as well, so an unrelated SayText2 landing inside
// that window is passed through untouched.
let suppressCoreVoteLine = false;

UserMessages.onPre("SayText2", (m): HookResultValue | void => {
  if (!suppressCoreVoteLine) return;
  // `param` is a REPEATED field on this message, so a scalar read of the whole body is not
  // available; the marker is looked for in the TextFormat dump instead. Narrow: it only runs while
  // the flag is up, which is one synchronous call per vote.
  if (m.debugString.includes("[Vote] RockTheVote")) return HookResult.Handled;
});

/** Print the ballot the way a ballot should look: a header, then one option per line.
 *  Broadcast (Chat.toAll) — everyone reads the ballot, so it resolves at the server default
 *  language, not any one player's. */
function announceBallot(options: string[]): void {
  Chat.toAll(Translations.translate(-1, "Rtv Ballot Header"));
  for (let i = 0; i < options.length; i++) {
    const label = options[i]!;
    const key = label === DONT_CHANGE ? "Ballot Option Dont Change" : "Ballot Option Map";
    Chat.toAll(Translations.translate(-1, key, i + 1, label));
  }
}

function paintHud(): void {
  if (hudHtml === "") return;
  for (const c of Clients.all()) {
    if (c.isBot) continue;
    const p = Player.fromSlot(c.slot);
    if (p === null) continue;
    Events.fireToClient(c.slot, "show_survival_respawn_status", {
      loc_token: hudHtml,
      duration: 1,
      userid: p.userId,
    });
  }
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

// The shared "mapvote" SQLite DB. This plugin runs standalone: it CREATEs the schema itself
// (IF NOT EXISTS — harmless alongside nominations, which owns the same identical schema), then
// reads the map_history (cooldown) + nominations (ballot) tables it needs. L1: OnPluginStart awaits
// the DB, so a failure FAILS the load loudly (no zombie) and `db` is non-null everywhere below.
let db!: Database;

/** Maps that must NOT be offered yet — the most-recently-played `rtv_cooldown` distinct maps. */
async function cooldownSet(): Promise<Set<string>> {
  const rows = await db.query(
    "SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?",
    [Math.max(0, config.getInt("rtv_cooldown"))]
  );
  return new Set(rows.map(r => String(r.map)));
}

/** This map's live nominations, in nomination order. */
async function nominationList(): Promise<string[]> {
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

  // The nominations table stores the map NAME only, so a nominated entry has to be resolved back
  // against maplist.txt to recover its workshop id. Hardcoding `workshopId: null` here (what this
  // used to do) meant a nominated WORKSHOP map won the vote and then `finishVote` fell to the
  // `changelevel <name>` branch — which only works for maps installed on the server. The level
  // change silently did nothing and the map stayed put, while a pool-filled map on the same
  // ballot worked fine, because those keep the id `loadPool` parsed.
  const known = loadPool();
  for (const name of await nominationList()) {
    if (options.length >= cap) break;
    if (entries.has(name)) continue;
    options.push(name);
    const listed = known.find(m => m.name === name);
    entries.set(name, { name, workshopId: listed ? listed.workshopId : null });
  }

  if (options.length < cap) {
    const cooldown = await cooldownSet();
    const pool = known.filter(m => !cooldown.has(m.name) && !entries.has(m.name));
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
    // Broadcast — no single recipient, resolves at the server default language.
    Chat.toAll(Translations.translate(-1, chosen === null ? "Rtv Vote Tied" : "Rtv Dont Change Won"));
    return;
  }

  const entry = entries.get(chosen) ?? { name: chosen, workshopId: null };
  if (!/^[A-Za-z0-9_]+$/.test(entry.name) || (entry.workshopId !== null && !/^[0-9]+$/.test(entry.workshopId))) {
    console.log("[rockthevote] winner failed validation: " + JSON.stringify(entry));
    Chat.toAll(Translations.translate(-1, "Rtv Winner Invalid"));
    return;
  }

  pendingMap = entry;
  votedThisMap = true;   // a change is queued for round end — block further RTV until the map changes
  console.log("[rockthevote] vote won by " + chosen + (entry.workshopId ? " (workshop " + entry.workshopId + ")" : "") + " — queued for round end");
  Chat.toAll(Translations.translate(-1, "Rtv Winner", chosen));
}

/** Start (or force) the RTV map vote: build the ballot, then hand it to @s2script/votes.
 *  Returns true if the request was accepted (the lock claimed); false if a vote is already active. */
function startVote(force: boolean): boolean {
  if (voteRunning || Vote.isActive()) return false;
  voteRunning = true;   // claim synchronously — closes the guard window so a concurrent requestRtv (buildBallot awaits the DB) can't double-start
  buildBallot().then(ballot => {
    if (ballot === null) {
      Chat.toAll(Translations.translate(-1, "Rtv No Maps Available"));
      voteRunning = false;      // release — nothing started
      votedThisMap = true;
      return;
    }
    rtvVoters.clear();
    const { options, entries } = ballot;
    // Suppress the core's own announcement for the duration of this start (see `suppressVoteLine`)
    // and print the ballot properly below, one option per line.
    suppressCoreVoteLine = true;
    Vote.start({
      question: "RockTheVote",
      options,
      duration: config.getInt("rtv_vote_duration"),
      showLiveTally: config.getBool("rtv_show_tally"),
      onEnd: (result) => finishVote(result, options, entries),
    });
    suppressCoreVoteLine = false;
    announceBallot(options);
    console.log("[rockthevote] startVote force=" + force + " options=" + JSON.stringify(options));
  }).catch(e => { voteRunning = false; logErr(e); });   // release the lock on a build error too
  return true;
}

function requestRtv(slot: number): void {
  if (voteRunning || votedThisMap) {
    Chat.toSlot(slot, Translations.translate(slot, voteRunning ? "Rtv Vote Already Running" : "Rtv Vote Already Happened"));
    return;
  }
  // rtv_initialdelay — refuse player RTV during a map's opening window (SM parity; sm_forcertv bypasses it).
  const initialDelayMs = Math.max(0, config.getInt("rtv_initialdelay")) * 1000;
  const remainingMs = initialDelayMs - (Date.now() - mapStartMs);
  if (remainingMs > 0) {
    Chat.toSlot(slot, Translations.translate(slot, "Rtv Not Open Yet", Math.ceil(remainingMs / 1000)));
    return;
  }
  const pc = playerCount();
  const need = Math.ceil(config.getFloat("rtv_threshold") * pc);
  if (rtvVoters.has(slot)) {
    Chat.toSlot(slot, Translations.translate(slot, "Rtv Already Voted", need));
    return;
  }
  rtvVoters.add(slot);
  if (pc < config.getInt("rtv_min_players")) {
    Chat.toSlot(slot, Translations.translate(slot, "Rtv Not Enough Players"));
    return;
  }
  if (rtvVoters.size >= need) {
    startVote(false);
  } else {
    // Broadcast — everyone sees the running tally, resolves at the server default language.
    Chat.toAll(Translations.translate(-1, "Rtv Wants To Vote", playerName(slot), need - rtvVoters.size));
  }
}

// Plugins persist across a changelevel — the shim has no level-init reload hook, so OnPluginStart fires
// once per plugin-load, NOT per map. Poll Server.mapName (throttled) to catch map transitions and
// reset all per-map RTV state (mirrors the nominations pattern); also re-evaluate the RTV threshold
// (a disconnect lowers the denominator but the per-player path can't re-trigger — SM re-checks on
// disconnect; we do it on the settled ~1s tick to avoid racing the disconnect event).
function pollTick(): void {
  // BEFORE the 1 Hz throttle: the centre-screen event paints for a single frame, so the live tally
  // has to be re-fired on EVERY frame it should be visible. A no-op (one string compare) whenever
  // no vote is running, which is nearly always.
  paintHud();
  if (++frameCounter < 64) return; // ~once/sec at 64-tick
  frameCounter = 0;
  const m = Server.mapName;
  if (m && m !== currentMap) { // map changed — reset all per-map RTV state
    currentMap = m;
    mapStartMs = Date.now();   // restart the rtv_initialdelay window for the new map
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

export async function OnPluginStart(): Promise<void> {
  translations.load("rockthevote", "common");

  db = await Database.open("mapvote");
  // Standalone-safe: create our own schema idempotently. IF NOT EXISTS makes this harmless
  // alongside nominations (whichever plugin loads first wins; the two CREATE statements are
  // byte-identical). Without this, running rockthevote WITHOUT nominations left the tables
  // absent → every cooldown/ballot query threw → buildBallot could never produce options.
  await db.execute("CREATE TABLE IF NOT EXISTS map_history(id INTEGER PRIMARY KEY AUTOINCREMENT, map TEXT NOT NULL, played_at INTEGER NOT NULL)", []);
  await db.execute("CREATE TABLE IF NOT EXISTS nominations(map TEXT PRIMARY KEY, nominator INTEGER NOT NULL)", []);

  // Fill the renderer seam @s2script/votes leaves open. `show` is called on every cast and once a
  // second by the vote tick; it only updates the buffer that `paintHud` re-fires each frame.
  Vote.registerTallyRenderer({
    show(_slot, tally) {
      const rows = tally.options
        .map((o, i) => {
          // The winner-so-far is the one players most need to read at a glance; a zero-vote option
          // is dimmed rather than hidden so the ballot numbering stays stable to type against.
          const leading = o.count > 0 && o.count === Math.max(...tally.options.map(x => x.count));
          const colour = o.count === 0 ? "#8a8a8a" : leading ? "#a3e048" : "#e8e8e8";
          return `<font color='#f0c040'>${i + 1}</font>`
            + `<font color='#8a8a8a'>.</font> `
            + `<font color='${colour}'>${esc(o.label)}</font>`
            + (o.count > 0 ? ` <font color='#8a8a8a'>(</font><font color='#f0c040'>${o.count}</font><font color='#8a8a8a'>)</font>` : "");
        })
        .join("<br>");
      // Wording only (Translations.translate at the server default language -1 — this HUD is the
      // same shared buffer for every viewer, see paintHud). The hex `color='#...'` values are raw
      // HTML for the show_survival_respawn_status panel, not the {tag} chat colour system, so they
      // stay literal here — see phrases.ts's header note. esc() covers the translated text too,
      // same as it already does for map labels: an operator-provided translation is as much
      // untrusted-for-HTML content as a player-supplied map name.
      hudHtml =
        `<font class='fontSize-m' color='#a3e048'>${esc(Translations.translate(-1, "Rtv Hud Title"))}</font><br>`
        + `<font class='fontSize-s' color='#8a8a8a'>${esc(Translations.translate(-1, "Rtv Hud Hint"))}</font>`
        + `<font class='fontSize-s' color='#f0c040'>${tally.secondsLeft}s</font>`
        + `<font class='fontSize-s' color='#8a8a8a'>${esc(Translations.translate(-1, "Rtv Hud Left"))}</font><br>`
        + `<font class='fontSize-sm'>${rows}</font>`;
    },
    clear() {
      hudHtml = "";
    },
  });

  hook.on("round_end", () => {
    if (!pendingMap) return;
    const m = pendingMap;
    // The workshop/stock split is load-bearing — see buildBallot's nomination resolution.
    Server.command(m.workshopId ? "host_workshop_map " + m.workshopId : "changelevel " + m.name);
    pendingMap = null;
  });

  command.admin("sm_forcertv", ADMFLAG.CHANGEMAP, (cmd) => {
    cmd.replyT(startVote(true) ? "Rtv Forced" : "Rtv Forced Already Running");
    return HookResult.Handled;
  });
}

export function OnGameFrame(): void {
  pollTick();
}

export function OnClientSayCommand(slot: number, text: string, _teamonly: boolean): HookResultValue {
  const t = text.trim().toLowerCase();
  const bang = t === "!rtv" || t === "!rockthevote";
  const bare = t === "rtv" || t === "rockthevote";
  if (bang || bare) {
    const c = Clients.fromSlot(slot);
    if (c && !c.isBot) requestRtv(slot);
  }
  return bang ? HookResult.Handled : HookResult.Continue;
}

export function OnClientDisconnect(c: Client): void {
  rtvVoters.delete(c.slot);
}
