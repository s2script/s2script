// @s2script/db-demo — proves the SQLite primitive persists across a server restart.
import { plugin } from "@s2script/sdk/plugin";
import { Database } from "@s2script/sdk/db";

export default plugin(async (ctx) => {
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

  return {
    onUnload(): void { console.log("[db-demo] onUnload"); },
  };
});
