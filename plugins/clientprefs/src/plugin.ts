// Payload ownership and retry deadlines live in core. This pump holds only active DB attempts
// and one load waiter per live slot, with a shared operation budget for writes and reads.
import { Database, Clients } from "@s2script/sdk";
import type { Client } from "@s2script/sdk";
declare const __s2pkg_clients: { _token(client: Client): string };
declare function __s2_cookie_session(slot: number, token: string, op: string, data: string): string;
declare function __s2_cookie_lease(maxItems: number, maxBytes: number): string;
declare function __s2_cookie_ack(leaseId: string, revision: string, success: boolean): boolean;
declare function __s2_cookie_account_fence(steamId: string): string;
declare function __s2_cookie_fence_done(steamId: string, revision: string): boolean;
interface Write { leaseId: string; revision: string; writerEpoch: string; steamId: string; name: string; value: string; updated: number }
interface Load { client: Client; token: string; steamId: string; fence: string; loading: boolean; done(): void }
let db!: Database;
let active = 0;
let started = false;
const loads = new Map<number, Load>();
// Provisional Task 5 policy; core also clamps leases. Task 6 will supply measured limits.
const MAX_OPERATIONS = 4;
const MAX_BATCH_BYTES = 256 * 1024;
const UPSERT = "INSERT INTO cookies (steamid, name, value, updated, writer_epoch, revision) VALUES (?, ?, ?, ?, ?, CAST(? AS INTEGER)) ON CONFLICT(steamid, name) DO UPDATE SET value=excluded.value, updated=excluded.updated, writer_epoch=excluded.writer_epoch, revision=excluded.revision WHERE cookies.writer_epoch IS NULL OR cookies.writer_epoch <> excluded.writer_epoch OR cookies.revision < excluded.revision";

async function persist(write: Write): Promise<void> {
  active++;
  try {
    await db.execute(UPSERT, [write.steamId, write.name, write.value, write.updated, write.writerEpoch, write.revision]);
    __s2_cookie_ack(write.leaseId, write.revision, true);
  } catch (e) {
    __s2_cookie_ack(write.leaseId, write.revision, false);
    console.log("[clientprefs] save retry: " + String(e));
  } finally { active--; }
}
function finish(load: Load): void {
  if (loads.get(load.client.slot) === load) loads.delete(load.client.slot);
  load.done();
}
async function select(load: Load): Promise<void> {
  load.loading = true; active++;
  try {
    const rows = await db.query("SELECT name, value, updated FROM cookies WHERE steamid = ?", [load.steamId]);
    if (!load.client.isValid() || loads.get(load.client.slot) !== load) return;
    __s2_cookie_session(load.client.slot, load.token, "load", JSON.stringify({ steamId: load.steamId,
      rows: rows.map(row => ({ name: String(row.name), value: String(row.value), updated: Number(row.updated) })) }));
  } catch (e) { console.log("[clientprefs] load ERROR for " + load.steamId + ": " + String(e)); }
  finally { active--; finish(load); }
}
function pump(): void {
  if (!started) return;
  // Serve eligible reads before new writes to prevent starvation under sustained write traffic.
  for (const load of loads.values()) {
    if (!load.client.isValid()) { finish(load); continue; }
    if (!load.loading && active < MAX_OPERATIONS && __s2_cookie_fence_done(load.steamId, load.fence)) void select(load);
  }
  if (active < MAX_OPERATIONS) {
    const writes = JSON.parse(__s2_cookie_lease(MAX_OPERATIONS - active, MAX_BATCH_BYTES)) as Write[];
    for (const write of writes) void persist(write);
  }
}
function loadCookies(client: Client): Promise<void> {
  if (!client.isValid() || client.steamId === "0") return Promise.resolve();
  const previous = loads.get(client.slot);
  if (previous) finish(previous);
  const pending = new Promise<void>(done => loads.set(client.slot, { client, token: __s2pkg_clients._token(client),
    steamId: client.steamId, fence: __s2_cookie_account_fence(client.steamId), loading: false, done }));
  pump(); return pending;
}
export async function OnPluginStart(): Promise<void> {
  db = await Database.open("clientprefs");
  await db.execute("CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, writer_epoch TEXT, revision INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (steamid, name))");
  const columns = await db.query("PRAGMA table_info(cookies)");
  if (!columns.some(c => c.name === "writer_epoch")) await db.execute("ALTER TABLE cookies ADD COLUMN writer_epoch TEXT");
  if (!columns.some(c => c.name === "revision")) await db.execute("ALTER TABLE cookies ADD COLUMN revision INTEGER NOT NULL DEFAULT 0");
  started = true;
  for (const client of Clients.all()) void loadCookies(client);
  pump();
}
export function OnClientPutInServer(client: Client): Promise<void> { return loadCookies(client); }
export function OnClientDisconnect(_client: Client): void { pump(); }
export function OnGameFrame(): void { pump(); }
