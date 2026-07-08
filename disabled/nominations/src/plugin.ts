import { Commands } from "@s2script/commands";
import { config } from "@s2script/config";
import { Database } from "@s2script/db";
import { Menu, MenuStyle } from "@s2script/menu";
import { Server } from "@s2script/server";
import { Player } from "@s2script/cs2";
import { Chat } from "@s2script/chat";

interface MapEntry { name: string; workshopId: string | null; }

const MAPLIST_TEMPLATE =
  "// nominations maplist — one map per line.\n" +
  "// Workshop maps: name:workshopId  (e.g. awp_lego_2:3070284539)\n" +
  "// Lines starting with // or # are ignored.\n" +
  "de_dust2\nde_inferno\nde_mirage\nde_nuke\nde_ancient\nde_anubis\n";

function parseMaplist(text: string): MapEntry[] {
  const out: MapEntry[] = [];
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("//") || line.startsWith("#")) continue;
    const i = line.indexOf(":");
    if (i >= 0) out.push({ name: line.slice(0, i).trim(), workshopId: line.slice(i + 1).trim() || null });
    else out.push({ name: line, workshopId: null });
  }
  return out;
}

function loadPool(): MapEntry[] {
  let text = config.readFile("maplist.txt");
  if (text === null) { config.writeFile("maplist.txt", MAPLIST_TEMPLATE); text = MAPLIST_TEMPLATE; }
  return parseMaplist(text);
}

// exact-name match wins, else case-insensitive substring (mirrors Player.target).
function resolveMap(input: string, pool: MapEntry[]): MapEntry[] {
  const needle = input.toLowerCase();
  const exact = pool.filter(m => m.name.toLowerCase() === needle);
  if (exact.length) return exact;
  return pool.filter(m => m.name.toLowerCase().includes(needle));
}

let db: Database | null = null;

async function cooldownSet(): Promise<Set<string>> {
  if (!db) return new Set();
  const rows = await db.query("SELECT map FROM map_history GROUP BY map ORDER BY MAX(id) DESC LIMIT ?", [config.getInt("map_cooldown")]);
  return new Set(rows.map(r => String(r.map)));
}
async function nominatedSet(): Promise<Set<string>> {
  if (!db) return new Set();
  const rows = await db.query("SELECT map FROM nominations", []);
  return new Set(rows.map(r => String(r.map)));
}

async function nominate(slot: number, name: string): Promise<void> {
  if (!db) { Chat.toSlot(slot, "[nominations] not ready."); return; }
  if ((await cooldownSet()).has(name)) { Chat.toSlot(slot, "[nominations] " + name + " was played too recently."); return; }
  if ((await nominatedSet()).has(name)) { Chat.toSlot(slot, "[nominations] " + name + " is already nominated."); return; }
  await db.execute("DELETE FROM nominations WHERE nominator = ?", [slot]);
  await db.execute("INSERT INTO nominations(map, nominator) VALUES(?, ?)", [name, slot]);
  const p = Player.fromSlot(slot);
  Chat.toAll("[nominations] " + (p ? p.playerName : "A player") + " nominated " + name + ".");
}

function mapMenu(slot: number, entries: MapEntry[], title: string): void {
  const m = new Menu(title);
  m.style = MenuStyle.Chat;   // non-freezing (players are mid-game)
  for (const e of entries) m.addItem(e.name, e.name);
  m.onSelect(e => { void nominate(e.slot, e.info); });   // nominate re-validates
  m.display(slot, 30);
}

async function nominateMenu(slot: number): Promise<void> {
  const pool = loadPool();
  const cd = await cooldownSet(), nom = await nominatedSet();
  const options = pool.filter(m => !cd.has(m.name) && !nom.has(m.name));
  if (options.length === 0) { Chat.toSlot(slot, "[nominations] No maps available to nominate right now."); return; }
  mapMenu(slot, options, "Nominate a map");
}

async function recordMapStart(): Promise<void> {
  if (!db) return;
  const cur = Server.mapName;
  const last = await db.query("SELECT map FROM map_history ORDER BY id DESC LIMIT 1", []);
  if (last.length && String(last[0].map) === cur) return;         // same map (a reload) -> keep nominations
  await db.execute("INSERT INTO map_history(map, played_at) VALUES(?, ?)", [cur, Math.floor(Date.now() / 1000)]);
  await db.execute("DELETE FROM nominations", []);                // new map -> fresh nominations
}

export function onLoad(): void {
  Database.open("mapvote").then(async (d) => {
    db = d;
    await db.execute("CREATE TABLE IF NOT EXISTS map_history(id INTEGER PRIMARY KEY AUTOINCREMENT, map TEXT NOT NULL, played_at INTEGER NOT NULL)", []);
    await db.execute("CREATE TABLE IF NOT EXISTS nominations(map TEXT PRIMARY KEY, nominator INTEGER NOT NULL)", []);
    await recordMapStart();
  }).catch((e) => console.log("[nominations] db init failed: " + e));

  Commands.register("sm_nominate", (ctx) => {
    const slot = ctx.callerSlot;
    if (slot < 0) { ctx.reply("Nominate in-game."); return; }
    const arg = ctx.arg(0);
    if (!arg) { void nominateMenu(slot); return; }
    const matches = resolveMap(arg, loadPool());
    if (matches.length === 0) ctx.reply("No map matching '" + arg + "'.");
    else if (matches.length === 1) void nominate(slot, matches[0].name);
    else mapMenu(slot, matches, "Did you mean...");   // disambiguate
  });

  console.log("[nominations] onLoad — sm_nominate registered");
}

export function onUnload(): void { console.log("[nominations] onUnload"); }
