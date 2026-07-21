// @s2script/clientprefs (plugin) — cookie DB lifecycle. L1: the factory awaits the DB; a failure
// FAILS the load loudly (no zombie), so `db` is non-null by construction everywhere below.
import { plugin } from "@s2script/sdk/plugin";
import { Database } from "@s2script/sdk/db";
import { Client } from "@s2script/sdk/clients";

declare function __s2_cookie_load(steamid: string, name: string, value: string, updated: number): void;
declare function __s2_cookie_mark_cached(steamid: string): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;
declare function __s2_cookie_clear(steamid: string): void;
declare function __s2_cookie_take_offline_writes(): Array<[string, string, string, number]>;
declare function __s2_cookie_dispatch_cached(slot: number): void;

export default plugin(async (ctx) => {
  const db = await Database.open("clientprefs");
  await db.execute(
    "CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))"
  );

  async function loadCookies(client: Client): Promise<void> {
    if (client.steamId === "0") return;   // skip bots
    const steamId = client.steamId;
    try {
      const rows = await db.query("SELECT name, value, updated FROM cookies WHERE steamid = ?", [steamId]);
      for (const row of rows) __s2_cookie_load(steamId, String(row.name), String(row.value), Number(row.updated));
      __s2_cookie_mark_cached(steamId);
      __s2_cookie_dispatch_cached(client.slot);
    } catch (e) {
      console.log("[clientprefs] load ERROR for " + steamId + ": " + String(e));
    }
  }

  async function saveCookies(client: Client): Promise<void> {
    if (client.steamId === "0") return;
    const steamId = client.steamId;
    const dirty = __s2_cookie_get_dirty(steamId);
    __s2_cookie_clear(steamId);
    const now = Math.floor(Date.now() / 1000);
    try {
      for (const name of Object.keys(dirty)) {
        await db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
          [steamId, name, dirty[name], now]);
      }
    } catch (e) {
      console.log("[clientprefs] save ERROR for " + steamId + ": " + String(e));
    }
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

  ctx.clients.onPutInServer(loadCookies);
  ctx.clients.onDisconnect(saveCookies);
  ctx.server.onGameFrame(drainOfflineWrites);
  console.log("[clientprefs] table ready, lifecycle hooked");
});
