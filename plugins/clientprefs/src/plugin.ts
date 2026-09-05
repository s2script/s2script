// @s2script/clientprefs (plugin) — cookie DB lifecycle. L1: OnPluginStart awaits the DB; a failure
// FAILS the load loudly (no zombie), so `db` is non-null by construction everywhere below.
import { Database } from "@s2script/sdk";
import type { Client } from "@s2script/sdk";

declare const __s2pkg_clients: { _token(client: Client): string };
declare function __s2_cookie_session(slot: number, token: string, op: string, data: string): string;
declare function __s2_cookie_take_retired(): string;
declare function __s2_cookie_take_offline_writes(): Array<[string, string, string, number]>;

let db!: Database;
// Serialize each account's departing writes before its replacement's SELECT. Queues disappear
// when drained; host-owned retired rows survive absent/dropped disconnect subscribers.
const accountWork = new Map<string, Promise<void>>();
function enqueue(steamId: string, work: () => Promise<void>): Promise<void> {
  const pending = (accountWork.get(steamId) ?? Promise.resolve()).then(work);
  accountWork.set(steamId, pending);
  void pending.finally(() => { if (accountWork.get(steamId) === pending) accountWork.delete(steamId); });
  return pending;
}
function drainRetired(): void {
  const rows = JSON.parse(__s2_cookie_take_retired()) as Array<[string, string, string, number]>;
  for (const [steamId, name, value, updated] of rows) {
    void enqueue(steamId, async () => {
      try {
        await db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)", [steamId, name, value, updated]);
      } catch (e) { console.log("[clientprefs] save ERROR for " + steamId + ": " + String(e)); }
    });
  }
}
async function loadCookies(client: Client): Promise<void> {
  if (!client.isValid() || client.steamId === "0") return;
  const steamId = client.steamId;
  const token = __s2pkg_clients._token(client);
  drainRetired();
  return enqueue(steamId, async () => {
    if (!client.isValid()) return;
    try {
      const rows = await db.query("SELECT name, value, updated FROM cookies WHERE steamid = ?", [steamId]);
      if (!client.isValid()) return;
      __s2_cookie_session(client.slot, token, "load", JSON.stringify({ steamId, rows: rows.map(row => ({ name: String(row.name), value: String(row.value), updated: Number(row.updated) })) }));
    } catch (e) { console.log("[clientprefs] load ERROR for " + steamId + ": " + String(e)); }
  });
}

function drainOfflineWrites(): void {
  const writes = __s2_cookie_take_offline_writes();
  if (writes.length === 0) return;
  for (const [steamid, name, value, updated] of writes) {
    db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
      [steamid, name, value, updated]
    ).catch((e) => console.log("[clientprefs] offline-write ERROR: " + String(e)));
  }
}

export async function OnPluginStart(): Promise<void> {
  db = await Database.open("clientprefs");
  await db.execute(
    "CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))"
  );
}

export function OnClientPutInServer(client: Client): void | Promise<void> {
  return loadCookies(client);
}

export function OnClientDisconnect(_client: Client): void | Promise<void> {
  drainRetired();
}

export function OnGameFrame(): void {
  drainRetired();
  drainOfflineWrites();
}
