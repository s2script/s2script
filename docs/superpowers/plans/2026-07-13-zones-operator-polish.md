# Zones operator polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the zone system's deferred operator-polish list: beam wireframe visualization (`sm_zone_show`/`sm_zone_hide`), an in-game E-to-mark editor (`sm_zone_edit`) with a live rubber-band preview, tags (`sm_zone_tag` + `getZonesByTag`/`setZoneTags` on the interface), `created`/`deleted` zone-lifecycle events, and the `1.0.x` → `0.1.0` versioning fix.

**Architecture:** Entirely game-layer — one plugin file (`plugins/zones/src/plugin.ts`, the existing single-file pattern), the `packages/zones` types package, the consumer demo, and a changeset. Composes the existing `Beam`/`pawn.origin`/`pawn.buttons` (`@s2script/cs2`), `Vector` (`@s2script/math`), `Chat.toSlot` (`@s2script/chat`), and the published `@s2script/zones` interface. No core/shim/op/ABI change → no sniper, hot-reloadable, cleanly parallel with any concurrent core slice.

**Tech Stack:** TypeScript (pure ESM), esbuild via `s2script build` (the 5E.1 full-strict typecheck gate), the `@s2script/*` first-party modules (types resolved from `packages/*/index.d.ts`), SQLite via `@s2script/db`.

**Spec:** `docs/superpowers/specs/2026-07-13-zones-operator-polish-design.md` (approved — source of truth for scope).

## Task → model routing

| Task | Implement | Review | Why |
|---|---|---|---|
| T1 Versioning fix | **haiku** | **sonnet** | Mechanical version/dep string edits across 3 files + 2 code strings; review needs only cross-file consistency judgment. |
| T2 Interface `.d.ts` + changeset | **sonnet** | **sonnet** | Standard `.d.ts` overload/interface work; the contract every later task must conform to, so a real (not haiku) review. |
| T3 Tags (migration + preservation + commands + interface impls) | **sonnet** | **opus** | Standard TS, but the data-migration idempotence and the tag-PRESERVATION semantics of `upsertZone` (a coords-only re-save must not wipe tags) are silent-data-loss risks — strongest reviewer. |
| T4 Beam viz | **sonnet** | **sonnet** | Straightforward composition of `Beam.draw` + a frame-expiry check + cleanup sites; failure modes are visible (orphan beams), not silent. |
| T5 E-mark editor | **opus** | **opus** | The highest-subtlety task: rising-edge button polling with a seeded `prevMask`, live `beam.update()` rubber-banding, session TTL, save/cancel ordering. Opus implements AND a separate opus instance reviews (per-risk pairing). |
| T6 `created`/`deleted` emits + consumer demo + offline verification | **sonnet** | **opus** | Emit-ordering is correctness-sensitive (deleted-before-clear on map change, single-emit through `upsertZone`, `iface`-null windows) — opus reviews the ordering invariants. |

Tasks run **sequentially** (T3 changes `upsertZone`'s signature that T5 calls and T6 instruments; T2 fixes the contract T3/T6 implement).

## Global Constraints

- **Worktree:** all work in `/home/gkh/projects/s2script-zones-polish` (branch `feat/zones-polish`). Absolute paths only. Do NOT touch `/home/gkh/projects/s2script`.
- **GAME-LAYER ONLY.** No changes to `core/src`, `shim/`, `gamedata/`, the `S2EngineOps` ABI, or `packages/entity`. **If a task believes it needs one, STOP** — it's out of scope (this slice must stay parallel-safe with a concurrent core slice). No sniper build exists in this plan.
- **Pure ESM** — named imports only (`import { X } from "@s2script/y"`); no `require`, no `import = require`, no `import * as` on interface proxies. The 5E.1 gate (`module: ESNext`, full `strict`) must pass.
- **Version-fix targets (exact, from the spec):** `plugins/zones/package.json` `publishes["@s2script/zones"]` → `"0.1.0"`; `pluginDependencies` → `@s2script/admin ^0.2.0`, `@s2script/cs2 ^0.3.0`, `commands`/`db`/`server`/`config`/`frame`/`interfaces` each `^0.1.0`, plus add `@s2script/math ^0.1.0` and `@s2script/entity ^0.2.0` (already imported by the plugin, previously undeclared). `examples/zones-consumer-demo/package.json` → `@s2script/cs2 ^0.3.0`, `@s2script/zones ^0.1.0`. Code: `publishInterface("@s2script/zones", "0.1.0", …)`. **No independent interface bump** — the interface version tracks the release. `build-base-plugins.sh` is NOT changed. T5 additionally adds `@s2script/chat ^0.1.0` (a new import).
- **The types package `packages/zones` stays independent** and gets the Changesets **minor** bump (`0.1.1` → `0.2.0` at release) — that axis is supposed to be independent.
- **Command names / gating (exact):** `sm_zone_show <name|all> [seconds]` (default 30; `0` = persistent), `sm_zone_hide [name|all]`, `sm_zone_edit <name>` / `sm_zone_edit` / `sm_zone_edit cancel`, `sm_zone_tag <name> [tag...]`, `sm_zone_list [tag]` — all `Commands.registerAdmin(…, ADMFLAG.GENERIC, …)`.
- **Event payloads (exact, wire-safe plain data):** `created` → `{ zone: string; min: Vec3; max: Vec3; tags: string[] }`; `deleted` → `{ zone: string }`. The `zone` key matches the existing enter/leave/stay events.
- **Tags representation:** stored comma-separated in a `tags TEXT` column; each tag lowercased + sanitized `[a-z0-9_-]` (≤32 chars); in-memory `Zone.tags: string[]`.
- **Build gate per task:** `node packages/cli/dist/cli.js build plugins/zones` (and `examples/zones-consumer-demo` where it changed). If `packages/cli/dist/cli.js` is absent, first run `( cd packages/cli && npm run build )`. T2/T6 also run `bash scripts/check-plugins-typecheck.sh` (all plugins + examples + disabled). There is no per-plugin unit-test harness — "tests" are build + typecheck + the scripted DB/helper sanity checks below. **The live rcon gate is a separate post-merge step on the shared server — no task deploys anything.**
- **Commit each task** with the trailer `Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy` via `git commit -F -` (heredoc; no backticks in the message). `git add` specific files only (never `dist/`).

---

### Task 1: Versioning fix (implement: haiku · review: sonnet)

**Files:**
- Modify: `plugins/zones/package.json`
- Modify: `plugins/zones/src/plugin.ts` (two strings only)
- Modify: `examples/zones-consumer-demo/package.json`

**Interfaces:**
- Consumes: the real `packages/*` versions (verified: admin `0.2.0`, cs2 `0.3.0`, entity `0.2.0`, math `0.1.1`, commands `0.1.2`, db/server/config/frame/interfaces `0.1.1`).
- Produces: a coherent `0.x` manifest for every later task to build on. No behavior change.

- [ ] **Step 1: `plugins/zones/package.json`** — replace the `s2script` block's dep/publish versions so the file reads exactly:
```json
{
  "name": "@s2script/zones",
  "version": "0.1.0",
  "private": true,
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x",
    "pluginDependencies": {
      "@s2script/commands": "^0.1.0",
      "@s2script/admin": "^0.2.0",
      "@s2script/db": "^0.1.0",
      "@s2script/server": "^0.1.0",
      "@s2script/config": "^0.1.0",
      "@s2script/frame": "^0.1.0",
      "@s2script/interfaces": "^0.1.0",
      "@s2script/cs2": "^0.3.0",
      "@s2script/entity": "^0.2.0",
      "@s2script/math": "^0.1.0"
    },
    "publishes": {
      "@s2script/zones": "0.1.0"
    }
  }
}
```
(`@s2script/entity` was already imported by `plugin.ts` but undeclared — a coherence fix in the same spirit as the spec's list; `@s2script/math` is spec-mandated, consumed from T4 on.)

- [ ] **Step 2: `plugins/zones/src/plugin.ts`** — two string edits, nothing else:
  - `publishInterface("@s2script/zones", "1.0.0", {` → `publishInterface("@s2script/zones", "0.1.0", {`
  - `console.log("[zones] publishing @s2script/zones@1.0.0");` → `…@0.1.0");`
  - The header comment's `` (`@s2script/zones@1.0.0`, emits enter/leave/stay) `` → `@s2script/zones@0.1.0`.

- [ ] **Step 3: `examples/zones-consumer-demo/package.json`** — `pluginDependencies` → `"@s2script/cs2": "^0.3.0"`, `"@s2script/zones": "^0.1.0"` (nothing else changes).

- [ ] **Step 4: Build + typecheck.** `( cd packages/cli && npm run build )` if `packages/cli/dist/cli.js` is absent, then `node packages/cli/dist/cli.js build plugins/zones` and `node packages/cli/dist/cli.js build examples/zones-consumer-demo` — both must print their `.s2sp` path.

- [ ] **Step 5: Commit.**
```bash
git add plugins/zones/package.json plugins/zones/src/plugin.ts examples/zones-consumer-demo/package.json
git commit -F - <<'EOF'
fix(zones): drop the 1.0.x anti-pattern - publishes/publishInterface -> 0.1.0, real 0.x dep carets (+ declare entity/math)

Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy
EOF
```

---

### Task 2: Interface `.d.ts` additions + changeset (implement: sonnet · review: sonnet)

**Files:**
- Modify: `packages/zones/index.d.ts`
- Create: `.changeset/zones-operator-polish.md`

**Interfaces:**
- Consumes: the existing declarations (verified: `Zone { name; min; max }`, `on(event: "enter" | "leave" | "stay", handler: (p: ZoneEvent) => void): number`).
- Produces: **the contract T3 and T6 implement** — `Zone.tags`, `getZonesByTag`, `setZoneTags`, `ZoneCreatedEvent`/`ZoneDeletedEvent`, `created`/`deleted` `on` overloads. Type names here are canonical; later tasks must match exactly.

- [ ] **Step 1: Extend `packages/zones/index.d.ts`.** Change `Zone` and add the new surface (keep every existing declaration; overloads of `on` must sit adjacent):
```ts
export interface Zone { name: string; min: Vec3; max: Vec3; tags: string[]; }

/** Payload of the `created` event (also fired per zone on a map's DB load; a re-save re-fires it). */
export interface ZoneCreatedEvent { zone: string; min: Vec3; max: Vec3; tags: string[]; }
/** Payload of the `deleted` event (also fired per zone cleared on a map change). */
export interface ZoneDeletedEvent { zone: string; }

/** The current map's zones carrying `tag` (lowercased match). */
export declare function getZonesByTag(tag: string): Zone[];
/** Set/replace a zone's tags (empty array clears). Returns true if the zone exists on the current map. */
export declare function setZoneTags(name: string, tags: string[]): boolean;
```
and replace the single `on` declaration with the overload set:
```ts
/** Subscribe to a zone event. `enter`/`leave` fire on boundary crossings; `stay` fires each tick while inside. */
export declare function on(event: "enter" | "leave" | "stay", handler: (p: ZoneEvent) => void): number;
/** `created` fires on createZone/sm_zone_add/the editor save, and per zone loaded on a map's DB load. */
export declare function on(event: "created", handler: (p: ZoneCreatedEvent) => void): number;
/** `deleted` fires on deleteZone/sm_zone_delete, and per zone cleared on a map change. */
export declare function on(event: "deleted", handler: (p: ZoneDeletedEvent) => void): number;
```
The header comment's `@s2script/zones@1.0.0` reference (if present) → `0.1.0`.

- [ ] **Step 2: The changeset** — create `.changeset/zones-operator-polish.md`:
```md
---
"@s2script/zones": minor
---

`@s2script/zones` interface additions: `Zone.tags`, `getZonesByTag(tag)`, `setZoneTags(name, tags)`, and the `created`/`deleted` zone-lifecycle events (`ZoneCreatedEvent`/`ZoneDeletedEvent`).
```
(This bumps the **types package** `packages/zones` `0.1.1` → `0.2.0` at release. The plugin/interface version stays release-bound at `0.1.0` — per the spec, no independent interface bump.)

- [ ] **Step 3: Typecheck the fleet.** `bash scripts/check-plugins-typecheck.sh` must pass (the consumer demo typechecks against the grown `.d.ts`; nothing consumes the new surface yet, so no breakage is expected — `Zone.tags` is only a widening of what `getZones()` returns, and the demo doesn't destructure `Zone`).

- [ ] **Step 4: Commit.**
```bash
git add packages/zones/index.d.ts .changeset/zones-operator-polish.md
git commit -F - <<'EOF'
feat(zones): interface types - Zone.tags, getZonesByTag, setZoneTags, created/deleted events (+minor changeset)

Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy
EOF
```

---

### Task 3: Tags — migration, preservation, commands, interface impls (implement: sonnet · review: opus)

**Files:**
- Modify: `plugins/zones/src/plugin.ts`

**Interfaces:**
- Consumes: `Database.execute(sql, params?): Promise<ExecuteResult>` / `query(sql, params?): Promise<Row[]>` (`packages/db/index.d.ts`); `ctx.args`/`ctx.reply` (`packages/commands/index.d.ts`); the T2 contract (`getZonesByTag`/`setZoneTags` shapes).
- Produces: `Zone.tags: string[]` on the in-memory registry; `sanitizeTag`/`parseTags`/`zonesByTag` helpers; a 3-arg **tag-preserving** `upsertZone(name, box, tags?)` (T5 calls the 2-arg form; T6 instruments it); the `sm_zone_tag` command + `sm_zone_list [tag]` filter; tags through export/import; `getZonesByTag`/`setZoneTags` on the published interface.

- [ ] **Step 1: The `Zone` shape + helpers.** Change the plugin-local interface and add the helpers next to `sanitizeName`:
```ts
interface Zone { name: string; min: Vec3; max: Vec3; tags: string[]; inside: Set<number>; trigger: TriggerZoneHandle | null; }

function sanitizeTag(t: string): string { return (t || "").toLowerCase().replace(/[^a-z0-9_-]/g, "").slice(0, 32); }
function parseTags(v: unknown): string[] {
  return String(v ?? "").split(",").map((t) => sanitizeTag(t)).filter((t) => t.length > 0);
}
function zonesByTag(tag: string): Zone[] {
  const t = sanitizeTag(tag);
  if (!t) return [];
  return Array.from(zones.values()).filter((z) => z.tags.includes(t));
}
```
**Every `zones.set` site must now carry `tags`** — the strict gate will enumerate them: `loadMap` (from the row), `upsertZone` (below), and `createZone`'s inline set (`tags: prev ? prev.tags : []`).

- [ ] **Step 2: Migration in `onLoad`.** The fresh `CREATE TABLE` gains the column AND a guarded `ALTER` covers pre-existing DBs (`CREATE TABLE IF NOT EXISTS` never alters an existing table). Immediately after the current `CREATE TABLE` execute:
```ts
await db.execute(
  "CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, tags TEXT, PRIMARY KEY (map, name))");
try { await db.execute("ALTER TABLE zones ADD COLUMN tags TEXT"); } catch { /* duplicate column name — already migrated */ }
```

- [ ] **Step 3: `loadMap` reads tags.** The SELECT becomes `"SELECT name, minX, minY, minZ, maxX, maxY, maxZ, tags FROM zones WHERE map = ?"` and the constructed zone gains `tags: parseTags(r.tags)` (a NULL column from a pre-migration row parses to `[]`).

- [ ] **Step 4: Tag-preserving `upsertZone`.** Replace the signature + body (the PRESERVATION rule is the load-bearing bit — a coords-only re-save from `sm_zone_add`/the editor/`createZone`/import must NOT wipe tags):
```ts
async function upsertZone(name: string, box: { min: Vec3; max: Vec3 }, tags?: string[]): Promise<void> {
  await dbReady;   // guarantee the DB is open (or failed) before we mutate
  const prev = zones.get(name);
  const t = tags !== undefined ? tags : (prev ? prev.tags : []);
  zones.set(name, { name, min: box.min, max: box.max, tags: t, inside: prev ? prev.inside : new Set<number>(), trigger: prev ? prev.trigger : null });
  pendingTriggers.add(name);   // (re)build the trigger on the next frame
  if (db) await db.execute(
    "INSERT OR REPLACE INTO zones (map, name, minX, minY, minZ, maxX, maxY, maxZ, tags) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    [currentMap, name, box.min.x, box.min.y, box.min.z, box.max.x, box.max.y, box.max.z, t.join(",")]);
}
```

- [ ] **Step 5: `sm_zone_tag` + the `sm_zone_list [tag]` filter.**
```ts
Commands.registerAdmin("sm_zone_tag", ADMFLAG.GENERIC, (ctx) => {
  const name = sanitizeName(ctx.args[0] || "");
  const z = zones.get(name);
  if (!name || !z) { ctx.reply(`No zone '${name}' on this map. Usage: sm_zone_tag <name> [tag...] (no tags = clear)`); return; }
  const tags = ctx.args.slice(1).map((t) => sanitizeTag(t)).filter((t) => t.length > 0);
  z.tags = tags;
  if (db) db.execute("UPDATE zones SET tags = ? WHERE map = ? AND name = ?", [tags.join(","), currentMap, name]).catch(() => {});
  ctx.reply(tags.length > 0 ? `Zone '${name}' tags: ${tags.join(", ")}` : `Zone '${name}' tags cleared.`);
});
```
`sm_zone_list` becomes tag-aware (filter + print tags; keep the coords/inside/trigger fields):
```ts
Commands.registerAdmin("sm_zone_list", ADMFLAG.GENERIC, (ctx) => {
  const filter = ctx.args.length > 0 ? sanitizeTag(ctx.args[0]) : "";
  const list = filter ? zonesByTag(filter) : Array.from(zones.values());
  ctx.reply(filter ? `Zones on ${currentMap} tagged '${filter}': ${list.length}` : `Zones on ${currentMap}: ${list.length}`);
  for (const z of list)
    ctx.reply(`  ${z.name} (${z.min.x.toFixed(0)},${z.min.y.toFixed(0)},${z.min.z.toFixed(0)})-(${z.max.x.toFixed(0)},${z.max.y.toFixed(0)},${z.max.z.toFixed(0)}) tags=[${z.tags.join(",")}] inside=${z.inside.size} trigger=${z.trigger ? "yes" : "pending"}`);
});
```

- [ ] **Step 6: Export/import round-trip.** Export shape becomes `Record<string, { min: number[]; max: number[]; tags: string[] }>`: `out[z.name] = { min: […], max: […], tags: z.tags };`. Import's parsed type gains `tags?: string[]`; per entry: `const tags = Array.isArray(e.tags) ? e.tags.map((t) => sanitizeTag(String(t))).filter((t) => t.length > 0) : undefined; pend.push(upsertZone(name, box, tags));` — an old (tagless) file passes `undefined` → preserves existing tags, backward-compatible.

- [ ] **Step 7: Interface impls** (added to the `publishInterface` object; the impl object isn't structurally typechecked against `packages/zones/index.d.ts` — keep the shapes matched to T2 by hand):
```ts
getZonesByTag(tag: string): { name: string; min: Vec3; max: Vec3; tags: string[] }[] {
  return zonesByTag(String(tag ?? "")).map((z) => ({ name: z.name, min: z.min, max: z.max, tags: z.tags }));
},
setZoneTags(name: string, tags: string[]): boolean {
  const nm = sanitizeName(name);
  const z = zones.get(nm);
  if (!z || !Array.isArray(tags)) return false;
  const t = tags.map((x) => sanitizeTag(String(x))).filter((x) => x.length > 0);
  z.tags = t;
  if (db) db.execute("UPDATE zones SET tags = ? WHERE map = ? AND name = ?", [t.join(","), currentMap, nm]).catch(() => {});
  return true;
},
```
and `getZones()` gains tags: `.map((z) => ({ name: z.name, min: z.min, max: z.max, tags: z.tags }))`.

- [ ] **Step 8: Offline DB round-trip check** (the migration + the exact SQL, via python3's sqlite3 — run from the scratchpad, do not commit the script):
```bash
python3 - <<'EOF'
import sqlite3
db = sqlite3.connect(":memory:")
# 1. the OLD (pre-tags) schema, as shipped before this slice
db.execute("CREATE TABLE IF NOT EXISTS zones (map TEXT, name TEXT, minX REAL, minY REAL, minZ REAL, maxX REAL, maxY REAL, maxZ REAL, PRIMARY KEY (map, name))")
db.execute("INSERT OR REPLACE INTO zones (map, name, minX, minY, minZ, maxX, maxY, maxZ) VALUES ('m','old',0,0,0,1,1,1)")
# 2. the guarded migration adds the column on an old DB
db.execute("ALTER TABLE zones ADD COLUMN tags TEXT")
# 3. idempotence: a second run throws (the plugin catches it)
try:
    db.execute("ALTER TABLE zones ADD COLUMN tags TEXT"); raise SystemExit("FAIL: second ALTER must throw")
except sqlite3.OperationalError: pass
# 4. the plugin's exact INSERT OR REPLACE + UPDATE + SELECT round-trip
db.execute("INSERT OR REPLACE INTO zones (map, name, minX, minY, minZ, maxX, maxY, maxZ, tags) VALUES (?,?,?,?,?,?,?,?,?)", ("m","t",0,0,0,1,1,1,"heal,vip"))
db.execute("UPDATE zones SET tags = ? WHERE map = ? AND name = ?", ("heal", "m", "t"))
rows = list(db.execute("SELECT name, tags FROM zones WHERE map='m' ORDER BY name"))
assert rows == [("old", None), ("t", "heal")], rows
# 5. parseTags on a NULL (pre-migration) column -> []
assert [x for x in str(rows[0][1] or "").split(",") if x] == []
print("DB tags round-trip: OK")
EOF
```

- [ ] **Step 9: Build + typecheck + commit.** `node packages/cli/dist/cli.js build plugins/zones` passes.
```bash
git add plugins/zones/src/plugin.ts
git commit -F - <<'EOF'
feat(zones): tags - guarded ALTER migration, tag-preserving upsert, sm_zone_tag + sm_zone_list filter, export/import round-trip, getZonesByTag/setZoneTags

Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy
EOF
```

---

### Task 4: Beam viz — `sm_zone_show` / `sm_zone_hide` (implement: sonnet · review: sonnet)

**Files:**
- Modify: `plugins/zones/src/plugin.ts`

**Interfaces:**
- Consumes (verified in `packages/cs2/index.d.ts:174-184` / `packages/math/index.d.ts`):
  - `Beam.draw(start: Vector, end: Vector, opts?: { color?: [number, number, number, number]; width?: number }): BeamHandle | null`
  - `BeamHandle { readonly ref: EntityRef; update(start: Vector, end: Vector): void; remove(): boolean; }`
  - `new Vector(x: number, y: number, z: number)` from `@s2script/math`
  - `ctx.argFloat(n: number, fallback?: number): number`
- Produces: `box12(min, max)` (pure — reused by T5's preview), `showZone(z, seconds)`, `hideZone(name)`, `clearAllBeams()`, the two commands, frame-loop expiry, cleanup on `loadMap`/`dropZone`/`onUnload`.

- [ ] **Step 1: Imports.** Extend the existing `@s2script/cs2` import with `Beam, BeamHandle` and add `import { Vector } from "@s2script/math";` (the manifest already declares `@s2script/math` from T1).

- [ ] **Step 2: State + the pure edge helper** (module scope, near `normBox`):
```ts
interface ShownEntry { beams: BeamHandle[]; expiresAt: number; }   // expiresAt: wall-clock ms; 0 = persistent
const shown = new Map<string, ShownEntry>();

// The 12 edges of the AABB [min,max]: 8 corners -> 4 bottom + 4 top + 4 vertical.
function box12(min: Vec3, max: Vec3): { a: Vec3; b: Vec3 }[] {
  const c: Vec3[] = [
    { x: min.x, y: min.y, z: min.z }, { x: max.x, y: min.y, z: min.z },
    { x: max.x, y: max.y, z: min.z }, { x: min.x, y: max.y, z: min.z },
    { x: min.x, y: min.y, z: max.z }, { x: max.x, y: min.y, z: max.z },
    { x: max.x, y: max.y, z: max.z }, { x: min.x, y: max.y, z: max.z },
  ];
  const e: [number, number][] = [[0,1],[1,2],[2,3],[3,0],[4,5],[5,6],[6,7],[7,4],[0,4],[1,5],[2,6],[3,7]];
  return e.map(([i, j]) => ({ a: c[i], b: c[j] }));
}
```

- [ ] **Step 3: show/hide/clear.**
```ts
function showZone(z: Zone, seconds: number): void {
  hideZone(z.name);   // never stack two beam sets for one zone
  const beams: BeamHandle[] = [];
  for (const e of box12(z.min, z.max)) {
    const b = Beam.draw(new Vector(e.a.x, e.a.y, e.a.z), new Vector(e.b.x, e.b.y, e.b.z), { color: [0, 255, 0, 255], width: 2 });
    if (b) beams.push(b);
  }
  shown.set(z.name, { beams, expiresAt: seconds > 0 ? Date.now() + seconds * 1000 : 0 });
}
function hideZone(name: string): void {
  const entry = shown.get(name);
  if (!entry) return;
  for (const b of entry.beams) { try { b.remove(); } catch { /* stale/already-gone */ } }
  shown.delete(name);
}
function clearAllBeams(): void { for (const name of Array.from(shown.keys())) hideZone(name); }
```

- [ ] **Step 4: Commands.**
```ts
Commands.registerAdmin("sm_zone_show", ADMFLAG.GENERIC, (ctx) => {
  const arg = ctx.args[0] || "";
  if (!arg) { ctx.reply("Usage: sm_zone_show <name|all> [seconds] (default 30; 0 = persistent)"); return; }
  const seconds = ctx.args.length > 1 ? Math.max(0, ctx.argFloat(1, 30)) : 30;
  if (arg === "all") {
    for (const z of zones.values()) showZone(z, seconds);
    ctx.reply(`Showing ${zones.size} zone(s)` + (seconds > 0 ? ` for ${seconds}s.` : " (persistent)."));
    return;
  }
  const z = zones.get(sanitizeName(arg));
  if (!z) { ctx.reply(`No zone '${sanitizeName(arg)}' on this map.`); return; }
  showZone(z, seconds);
  ctx.reply(`Showing '${z.name}'` + (seconds > 0 ? ` for ${seconds}s.` : " (persistent)."));
});
Commands.registerAdmin("sm_zone_hide", ADMFLAG.GENERIC, (ctx) => {
  const arg = ctx.args[0] || "all";
  if (arg === "all") { const n = shown.size; clearAllBeams(); ctx.reply(`Hid ${n} zone(s).`); return; }
  const name = sanitizeName(arg);
  if (!shown.has(name)) { ctx.reply(`Zone '${name}' is not shown.`); return; }
  hideZone(name);
  ctx.reply(`Hid '${name}'.`);
});
```

- [ ] **Step 5: Expiry + cleanup wiring.** At the TOP of the **existing** `OnGameFrame.subscribe` handler (before the `pendingTriggers` block — deleting from a `Map` while iterating it is safe in JS):
```ts
if (shown.size > 0) {
  const now = Date.now();
  for (const [name, entry] of shown) if (entry.expiresAt > 0 && now >= entry.expiresAt) hideZone(name);
}
```
Then: `clearAllBeams();` as the first line of `loadMap` (beams are world entities — gone on the new map, but the handles must not go stale-retained); `hideZone(name);` inside `dropZone` (deleting a shown zone must not orphan its wireframe); `clearAllBeams();` in `onUnload` (game-world-owned, not auto-ledgered).

- [ ] **Step 6: Pure-helper sanity** (scratchpad, not committed — `box12` copied verbatim into a node one-liner):
```bash
node -e '
const box12=(min,max)=>{const c=[{x:min.x,y:min.y,z:min.z},{x:max.x,y:min.y,z:min.z},{x:max.x,y:max.y,z:min.z},{x:min.x,y:max.y,z:min.z},{x:min.x,y:min.y,z:max.z},{x:max.x,y:min.y,z:max.z},{x:max.x,y:max.y,z:max.z},{x:min.x,y:max.y,z:max.z}];const e=[[0,1],[1,2],[2,3],[3,0],[4,5],[5,6],[6,7],[7,4],[0,4],[1,5],[2,6],[3,7]];return e.map(([i,j])=>({a:c[i],b:c[j]}))};
const E=box12({x:0,y:0,z:0},{x:1,y:2,z:3});
const key=(p)=>p.x+","+p.y+","+p.z; const set=new Set(E.map(e=>[key(e.a),key(e.b)].sort().join("|")));
if (E.length!==12||set.size!==12) throw new Error("box12 FAIL");
if (!E.every(e=>{const d=["x","y","z"].filter(k=>e.a[k]!==e.b[k]);return d.length===1;})) throw new Error("edges must be axis-aligned");
console.log("box12: 12 distinct axis-aligned edges OK");'
```

- [ ] **Step 7: Build + commit.** `node packages/cli/dist/cli.js build plugins/zones` passes.
```bash
git add plugins/zones/src/plugin.ts
git commit -F - <<'EOF'
feat(zones): beam wireframe viz - box12 helper, sm_zone_show/sm_zone_hide, frame expiry, loadMap/dropZone/onUnload cleanup

Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy
EOF
```

---

### Task 5: E-to-mark editor — `sm_zone_edit` (implement: opus · review: opus)

**Files:**
- Modify: `plugins/zones/src/plugin.ts`
- Modify: `plugins/zones/package.json` (add `"@s2script/chat": "^0.1.0"` to `pluginDependencies` — the in-frame notices have no `ctx.reply`)

**Interfaces:**
- Consumes (verified):
  - `pawn.buttons: number` (`packages/cs2/index.d.ts:46` — "currently-pressed button mask (low 32 bits; IN_USE/E = 32). 0 if the mask is unreadable"), `pawn.origin: Vector | null` (:29), `Pawn.forSlot(slot): Pawn | null` (:79)
  - `Chat.toSlot(slot: number, message: string): void` (`packages/chat/index.d.ts:13`)
  - T4's `box12`/`showZone`; T3's `upsertZone(name, box)` (2-arg → tags preserved); the existing `normBox`/`sanitizeName`
  - The rising-edge REFERENCE technique (`games/cs2/js/pawn.js:446-460`, the menu poll): per-slot `prevMask`, **seeded at session start** so a held E doesn't fire on entry; `pressed = mask & ~prevMask`; then `prevMask = mask`. Copy the technique — the plugin reads `pawn.buttons` (already exists); do NOT edit pawn.js.
- Produces: the `edits` session map, `cancelEdit`/`clearAllEdits`, the `sm_zone_edit` command, a dedicated `OnGameFrame` poll with live rubber-band preview. The save path calls `upsertZone` → T6's `created` emit comes for free.

- [ ] **Step 1: Import + state.** Add `import { Chat } from "@s2script/chat";` and the manifest dep. Module-scope state:
```ts
const IN_USE = 32;   // in_buttons.h (E)
interface EditSession { name: string; cornerA: Vec3 | null; prevMask: number; expiresAt: number; preview: BeamHandle[]; }
const edits = new Map<number, EditSession>();   // keyed by 0-based player slot

function clearPreview(s: EditSession): void {
  for (const b of s.preview) { try { b.remove(); } catch { /* stale/already-gone */ } }
  s.preview = [];
}
function cancelEdit(slot: number, notice?: string): void {
  const s = edits.get(slot);
  if (!s) return;
  clearPreview(s);
  edits.delete(slot);
  if (notice) Chat.toSlot(slot, notice);
}
function clearAllEdits(): void { for (const slot of Array.from(edits.keys())) cancelEdit(slot); }
```

- [ ] **Step 2: The command.**
```ts
Commands.registerAdmin("sm_zone_edit", ADMFLAG.GENERIC, (ctx) => {
  if (ctx.callerSlot < 0) { ctx.reply("sm_zone_edit is in-game only (it marks corners at your position)."); return; }
  const raw = ctx.args[0] || "";
  if (!raw || raw === "cancel") {
    if (edits.has(ctx.callerSlot)) { cancelEdit(ctx.callerSlot); ctx.reply("Zone edit cancelled."); }
    else ctx.reply("Usage: sm_zone_edit <name>  |  sm_zone_edit cancel");
    return;
  }
  const name = sanitizeName(raw);
  if (!name) { ctx.reply("Invalid zone name."); return; }
  const pw = Pawn.forSlot(ctx.callerSlot);
  if (!pw || !pw.origin) { ctx.reply("No position — spawn in first."); return; }
  cancelEdit(ctx.callerSlot);   // replace any prior session (and remove its preview)
  edits.set(ctx.callerSlot, { name, cornerA: null, prevMask: pw.buttons, expiresAt: Date.now() + 60_000, preview: [] });
  ctx.reply(`Editing zone '${name}': walk to a corner and press E; press E again at the opposite corner. (60s timeout; sm_zone_edit cancel to abort)`);
});
```
(`prevMask` is seeded with the CURRENT mask — the menu system's proven fix for a held E reading as a fresh rising edge. A literal zone named `cancel` can't be edited; documented, acceptable.)

- [ ] **Step 3: The dedicated poll** — a SECOND `OnGameFrame.subscribe` in `onLoad` (the `.d.ts` `subscribe(fn, opts?): void` returns nothing, so it cannot be disposed — it early-returns when idle, which is the "runs only while editing" semantics):
```ts
OnGameFrame.subscribe(() => {
  if (edits.size === 0) return;
  const now = Date.now();
  for (const [slot, s] of edits) {
    if (now >= s.expiresAt) { cancelEdit(slot, "[zones] Edit session timed out."); continue; }
    const pw = Pawn.forSlot(slot);
    if (!pw) { cancelEdit(slot, "[zones] Edit session cancelled (no pawn)."); continue; }
    const mask = pw.buttons;                     // 0 if unreadable — a momentary 0 can re-arm the edge; acceptable
    const pressed = mask & ~s.prevMask;
    s.prevMask = mask;
    const origin = pw.origin;
    // Live rubber-band: corner A set, corner B pending -> retarget the 12 preview beams to the walking box.
    if (s.cornerA && s.preview.length === 12 && origin) {
      const box = normBox(s.cornerA, origin);
      const edges = box12(box.min, box.max);
      for (let i = 0; i < 12; i++)
        s.preview[i].update(new Vector(edges[i].a.x, edges[i].a.y, edges[i].a.z), new Vector(edges[i].b.x, edges[i].b.y, edges[i].b.z));
    }
    if (!(pressed & IN_USE)) continue;
    if (!origin) { Chat.toSlot(slot, "[zones] No position — try again."); continue; }   // don't consume the press
    if (!s.cornerA) {
      // 1st press: pin corner A (a COPY — origin is a snapshot but never alias it) + create the preview collapsed at A.
      s.cornerA = { x: origin.x, y: origin.y, z: origin.z };
      for (const e of box12(s.cornerA, s.cornerA)) {
        const b = Beam.draw(new Vector(e.a.x, e.a.y, e.a.z), new Vector(e.b.x, e.b.y, e.b.z), { color: [255, 165, 0, 255], width: 2 });
        if (b) s.preview.push(b);
      }
      Chat.toSlot(slot, "[zones] Corner 1 set — walk to the opposite corner and press E.");
    } else {
      // 2nd press: normalize, reject zero-volume (keep the session), else save + swap preview for a timed showZone.
      const box = normBox(s.cornerA, { x: origin.x, y: origin.y, z: origin.z });
      if (box.min.x === box.max.x || box.min.y === box.max.y || box.min.z === box.max.z) {
        Chat.toSlot(slot, "[zones] Zero-volume box — move further from corner 1 and press E again.");
        continue;
      }
      const name = s.name;
      cancelEdit(slot);   // end the session + remove the preview BEFORE the async save
      upsertZone(name, box)
        .then(() => {
          const z = zones.get(name);
          if (z) showZone(z, 10);   // timed confirmation wireframe of the SAVED box
          Chat.toSlot(slot, `[zones] Zone '${name}' saved.`);
        })
        .catch((e) => Chat.toSlot(slot, `[zones] Save failed: ${e}`));
    }
  }
});
```
Notes the implementer must keep: (a) if any `Beam.draw` returned null the preview is partial — the `s.preview.length === 12` guard keeps index/edge alignment and simply skips rubber-banding (rare, degrade-not-crash); (b) an E press also fires the game's own `+use` (door/pickup) — inherent to no-detour button polling, documented caveat; (c) `edits` may be mutated by `cancelEdit` during the `for...of` — Map iteration tolerates deletes.

- [ ] **Step 4: Cleanup wiring.** `clearAllEdits();` beside `clearAllBeams()` in both `loadMap` (map change kills the preview entities; sessions must not survive into a new map's coordinates) and `onUnload`.

- [ ] **Step 5: Build + commit.** `node packages/cli/dist/cli.js build plugins/zones` passes.
```bash
git add plugins/zones/src/plugin.ts plugins/zones/package.json
git commit -F - <<'EOF'
feat(zones): in-game E-to-mark editor - sm_zone_edit sessions, seeded rising-edge poll, live rubber-band preview, 60s TTL + cancel

Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy
EOF
```

---

### Task 6: `created`/`deleted` emits + consumer demo + offline verification (implement: sonnet · review: opus)

**Files:**
- Modify: `plugins/zones/src/plugin.ts`
- Modify: `examples/zones-consumer-demo/src/plugin.ts`

**Interfaces:**
- Consumes: `PublishHandle.emit(event: string, payload: unknown): void` (`packages/interfaces/index.d.ts:12`); the T2 payload contracts (`ZoneCreatedEvent`/`ZoneDeletedEvent`); T3's `Zone.tags`.
- Produces: `emitCreated`/`emitDeleted` wired into every create/delete path via exactly TWO create sites (`upsertZone`, `loadMap`) and TWO delete sites (`dropZone`, `loadMap`'s clear) — the command/interface/editor paths all funnel through them; consumer-demo `created`/`deleted` logs (makes the events rcon-provable at the live gate); the slice-wide offline verification.

- [ ] **Step 1: Emit helpers** (module scope, after the `iface` declaration; `iface` is `PublishHandle | null` — null only before `publishInterface` runs in `onLoad`, and every emit site below runs after or async-after it):
```ts
function emitCreated(z: Zone): void { if (iface) iface.emit("created", { zone: z.name, min: z.min, max: z.max, tags: z.tags }); }
function emitDeleted(name: string): void { if (iface) iface.emit("deleted", { zone: name }); }
```

- [ ] **Step 2: Wire the create paths.** ONE emit site covers `sm_zone_add`, the editor save, `sm_zone_import`, and the interface `createZone` (whose inline `zones.set` is followed by its `upsertZone(...)` call — the emit lives in `upsertZone` ONLY, so no double-fire): in `upsertZone`, immediately after `pendingTriggers.add(name);` add:
```ts
emitCreated(zones.get(name)!);
```
And the per-zone load: in `loadMap`'s row loop, after `pendingTriggers.add(name);` add `emitCreated(zones.get(name)!);` (spec: `created` fires for each zone loaded on a map's DB load; on the initial load `iface` is already set — `publishInterface` runs synchronously in `onLoad` while the DB open is async).

- [ ] **Step 3: Wire the delete paths.** In `dropZone`, after `zones.delete(name);` add `emitDeleted(name);` (covers `deleteZone` + `sm_zone_delete`). In `loadMap`, BEFORE `zones.clear()` add:
```ts
for (const name of zones.keys()) emitDeleted(name);   // map change: the old map's zones are cleared
```
**Ordering invariant (the review target):** on a changelevel, consumers see `deleted` for every old-map zone BEFORE any `created` of the new map's rows — `loadMap` clears, then loads. The stream is a consistent delta feed over the `getZones()` snapshot (the standard list+subscribe pattern; the existing probe-then-subscribe load-order caveat still applies and is already documented in the `.d.ts` header).

- [ ] **Step 4: Consumer demo logs.** In `examples/zones-consumer-demo/src/plugin.ts`, extend the type import to `import type { ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "@s2script/zones";` and add to `subscribe()`:
```ts
on("created", (p: ZoneCreatedEvent) => {
  console.log(`[zones-consumer] CREATED ${p.zone} tags=[${p.tags.join(",")}] min=(${p.min.x.toFixed(0)},${p.min.y.toFixed(0)},${p.min.z.toFixed(0)}) max=(${p.max.x.toFixed(0)},${p.max.y.toFixed(0)},${p.max.z.toFixed(0)})`);
});
on("deleted", (p: ZoneDeletedEvent) => { console.log(`[zones-consumer] DELETED ${p.zone}`); });
```
(This is what makes the live gate rcon-provable: `sm_zone_add`/`sm_zone_delete` → the consumer logs the cross-plugin event.)

- [ ] **Step 5: Slice-wide offline verification** (all must pass before commit):
```bash
node packages/cli/dist/cli.js build plugins/zones
node packages/cli/dist/cli.js build examples/zones-consumer-demo
bash scripts/build-base-plugins.sh          # every base plugin still builds
bash scripts/check-plugins-typecheck.sh     # full-strict across plugins + examples + disabled
```
Re-run the T3 DB round-trip script (unchanged SQL — confirms no emit change touched persistence).

- [ ] **Step 6: Commit.**
```bash
git add plugins/zones/src/plugin.ts examples/zones-consumer-demo/src/plugin.ts
git commit -F - <<'EOF'
feat(zones): created/deleted zone-lifecycle events on the interface + consumer-demo logs; slice-wide offline verification

Claude-Session: https://claude.ai/code/session_013MaVbyGp1ZGd5WTnH2Egsy
EOF
```

---

## Post-merge live gate (NOT a task — the shared server; coordinate before deploying)

de_inferno/de_dust2, `bot_quota 4`, rcon, per the spec's Testing section: boot logs `@s2script/zones@0.1.0`; tags survive a restart (migration + round-trip); `sm_zone_show t 0` / `sm_zone_show all 5` / `sm_zone_hide all` / frame expiry run crash-free; `sm_zone_edit e` arms + `cancel` clears; `sm_zone_add`/`sm_zone_delete` produce the consumer demo's `CREATED`/`DELETED` lines. Human-client deferrals: seeing the wireframe; walking corners with the live rubber-band preview.

**T7 (post-live-gate follow-up):** `sm_zone_add <name>` bare in-game form starts the E-mark session via a shared `startMarking(slot, name)` helper (extracted from `sm_zone_edit`); "Creating"/"Editing" verb is name-driven.

## Self-Review

- **Spec coverage:** versioning fix (T1 — manifest, code strings, consumer carets; `build-base-plugins.sh` untouched) · interface types + changeset minor (T2) · tags: migration/preservation/`sm_zone_tag`/`sm_zone_list [tag]`/export-import/`getZonesByTag`/`setZoneTags` (T3) · viz: `box12`/`showZone`/`hideZone`/`clearAllBeams`/commands/expiry/cleanup (T4) · editor: sessions/seeded rising edge/rubber-band `update()`/TTL/cancel/save→`showZone` (T5) · `created`/`deleted` on all spec'd paths + consumer proof + verification (T6). Every spec section maps to a task.
- **Type consistency:** `Vec3`, `Zone` (gains `tags: string[]` in T3, matching T2's `.d.ts`), `ShownEntry`, `EditSession`, `box12`, `upsertZone(name, box, tags?)`, `showZone(z, seconds)`, `cancelEdit(slot, notice?)`, `emitCreated(z)`/`emitDeleted(name)` are used with identical shapes across tasks.
- **No placeholders:** every code step quotes real, verified signatures (`Beam.draw`/`BeamHandle.update`/`remove`, `pawn.buttons`/`origin`, `Chat.toSlot`, `ctx.argFloat`, `PublishHandle.emit`, `OnGameFrame.subscribe(fn): void` — hence the early-return poll, not a disposable).
- **Resolved ambiguities:** (1) the spec's dep list omitted `@s2script/entity` (already imported, undeclared) — declared in T1 as the same coherence fix; (2) in-frame editor notices need `Chat.toSlot` → `@s2script/chat` dep added in T5 (the spec's "Reply" on E-press implies a chat channel; `ctx.reply` doesn't exist in a frame poll); (3) `createZone`'s inline `zones.set` + its `upsertZone` call would double-emit `created` — the emit lives ONLY in `upsertZone`; (4) `dropZone` also hides a shown zone's beams (orphan-wireframe hygiene, implied by the cleanup rule).
