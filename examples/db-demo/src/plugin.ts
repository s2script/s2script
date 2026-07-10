// @s2script/db-demo — proves the SQLite primitive persists across a server restart.
import { Database } from "@s2script/db";

export async function onLoad(): Promise<void> {
  try {
    const db = await Database.open("demo");
    await db.execute("CREATE TABLE IF NOT EXISTS boots (id INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT)");
    const res = await db.execute("INSERT INTO boots (at) VALUES (?)", ["load"]);
    const rows = await db.query("SELECT COUNT(*) AS n FROM boots", []);
    const n = rows.length ? rows[0].n : 0;
    console.log("[db-demo] onLoad — inserted id=" + res.lastInsertId + " total boots=" + n);
    await db.close();
  } catch (e) {
    console.log("[db-demo] onLoad ERROR: " + String(e));
  }
}

export function onUnload(): void { console.log("[db-demo] onUnload"); }
