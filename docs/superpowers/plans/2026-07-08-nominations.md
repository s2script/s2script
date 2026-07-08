# nominations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `nominations` plugin (map nomination with best-guess matching + a menu) + the shared map-vote SQLite foundation (`map_history`/`nominations` tables, `maplist.txt` pool, configurable cooldown), plus a small `config.readFile`/`writeFile` capability. Shipped opt-in in `disabled/`.

**Architecture:** A new engine-generic `config.readFile`/`writeFile` (raw configs-dir file access via a shim op pair) lets a plugin read a plain-text `maplist.txt`. The `nominations` plugin (in a top-level `disabled/` source dir, deployed to `plugins/disabled/`) opens a shared `mapvote` SQLite DB (`@s2script/db`), records the played map on `onLoad`, and adds `sm_nominate` (fuzzy-resolve against the maplist, one-per-player, cooldown-gated) + a nomination menu.

**Tech Stack:** C++ (shim ops), Rust (core natives + `@s2script/config` prelude method), TypeScript (`nominations` plugin via `s2script build`).

## Global Constraints

- **Charter boundary — core engine-generic; deps game→core.** `config.readFile`/`writeFile` (a sanitized file name → raw bytes) is engine-generic. CS2 facts (`Server.mapName`/`isMapValid`, `Player`) live in the `nominations` plugin. `scripts/check-core-boundary.sh` + `scripts/test-boundary-nameleak.sh` stay green.
- **ABI append-only.** The new ops APPEND after the current last `S2EngineOps` field (`db_data_dir`), same order in the C header (`shim/include/s2script_core.h`), the Rust mirror (`core/src/v8host.rs`), and BOTH in-isolate test op-structs.
- **Path safety.** `ConfigFilePath(name)` reuses `ConfigPath`'s sanitize (non-`[A-Za-z0-9._-]` → `_`, which neutralizes `/`) and additionally returns an empty path if `name` contains `..` or is empty → a read returns null / a write no-ops. No traversal.
- **`maplist.txt`**: one map/line; `//`/`#`/blank ignored; entry `<name>` or `<name>:<workshopId>` (colon; the ID is opaque text this slice).
- **Naming:** camelCase methods (`readFile`, `writeFile`); `sm_nominate` open to any player (`Commands.register`, guarded `callerSlot >= 0`).
- **Test running:** core tests serial (`cd core && cargo test`); in-isolate config tests use `eval_in_context_string`/`frame_tests`. Full spec: `docs/superpowers/specs/2026-07-08-nominations-design.md`.

---

### Task 1: `config.readFile` / `config.writeFile` capability

Raw configs-dir file read/write for plugins.

**Files:**
- Modify: `shim/include/s2script_core.h` — append `config_read_file`/`config_write_file` to `S2EngineOps`.
- Modify: `shim/src/s2script_mm.cpp` — `ConfigFilePath` + `s2_config_read_file`/`s2_config_write_file` + wire into the ops struct.
- Modify: `core/src/v8host.rs` — the fn typedefs + ops fields (after `db_data_dir`) + the ENGINE_OPS init + the `__s2_config_read_file`/`__s2_config_write_file` natives + registration + both test op-structs + `readFile`/`writeFile` on the `__s2_config` module object; in-isolate tests.
- Modify: `packages/config/index.d.ts` — `readFile`/`writeFile`.

**Interfaces:**
- Consumes: the existing `s2_config_read_raw` native (v8host.rs:4241) + `ConfigPath` (shim) as the mirror patterns.
- Produces: `config.readFile(name: string): string | null` + `config.writeFile(name: string, content: string): void` on `@s2script/config`'s `config` object; natives `__s2_config_read_file(name)`/`__s2_config_write_file(name, content)`.

- [ ] **Step 1: Write the failing in-isolate tests**

Add to `frame_tests` in `core/src/v8host.rs` (mirror the existing config-degrade tests):

```rust
#[test]
fn config_read_file_degrades_without_ops() {
    // No engine ops -> readFile returns null, writeFile is a no-op (never throws).
    assert_eq!(
        eval_in_context_string("p", r#"var {config}=__s2pkg_config; config.writeFile("x.txt","hi"); String(config.readFile("x.txt"))"#),
        "null"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd core && cargo test config_read_file_degrades`
Expected: FAIL — `config.readFile` is not a function (`TypeError`).

- [ ] **Step 3: Add the shim ops**

In `shim/src/s2script_mm.cpp`, after `ConfigPath` (near line 1116), add a no-`.json` variant + the ops:
```cpp
// ConfigFilePath: like ConfigPath but the name INCLUDES its extension (no .json append). Reuses the same
// sanitize (non-[A-Za-z0-9._-] -> '_', which neutralizes '/'); additionally refuses names containing ".."
// or empty (returns "" -> read/write fail) so there is no traversal.
static std::string ConfigFilePath(const char* name) {
    if (!name || !*name) return "";
    if (std::string(name).find("..") != std::string::npos) return "";
    std::string safe;
    for (const char* p = name; *p; ++p) {
        char c = *p;
        safe += ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9')
                 || c == '.' || c == '_' || c == '-') ? c : '_';
    }
    Dl_info info;
    if (dladdr(reinterpret_cast<void*>(&ConfigFilePath), &info) && info.dli_fname) {
        char buf[4096];
        snprintf(buf, sizeof buf, "%s", info.dli_fname);
        std::string dir = dirname(buf); snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);             snprintf(buf, sizeof buf, "%s", dir.c_str());
        dir = dirname(buf);
        return dir + "/configs/" + safe;
    }
    return "addons/s2script/configs/" + safe;
}
static std::string s_configFileReadBuf;
static const char* s2_config_read_file(const char* name) {
    std::string path = ConfigFilePath(name);
    if (path.empty()) return nullptr;
    std::ifstream f(path); if (!f) return nullptr;
    std::stringstream ss; ss << f.rdbuf(); s_configFileReadBuf = ss.str();
    return s_configFileReadBuf.c_str();
}
static int s2_config_write_file(const char* name, const char* content) {
    std::string path = ConfigFilePath(name); if (path.empty() || !content) return 0;
    std::error_code ec; std::filesystem::create_directories(std::filesystem::path(path).parent_path(), ec);
    std::ofstream f(path); if (!f) return 0; f << content; return f.good() ? 1 : 0;
}
```
Wire into the ops struct where the shim fills `S2EngineOps` (search `ops.db_data_dir`), appended after it:
```cpp
    ops.config_read_file  = s2_config_read_file;
    ops.config_write_file = s2_config_write_file;
```
Append to the C header `S2EngineOps` struct (`shim/include/s2script_core.h`), after the last op:
```c
    /* Slice nominations: raw configs-dir file read/write (name includes its extension; no .json append). */
    const char* (*config_read_file)(const char* name);
    int         (*config_write_file)(const char* name, const char* content);
```

- [ ] **Step 4: Add the core ops mirror + natives + module method**

In `core/src/v8host.rs`:
1. Fn typedefs (near `ConfigReadFn`):
```rust
type ConfigReadFileFn  = extern "C" fn(name: *const c_char) -> *const c_char;
type ConfigWriteFileFn = extern "C" fn(name: *const c_char, content: *const c_char) -> i32;
```
2. Ops fields — APPEND after `pub db_data_dir: Option<DbDataDirFn>,`:
```rust
    // --- Slice nominations: raw config-file read/write (APPENDED after db_data_dir; order is the ABI) ---
    pub config_read_file:  Option<ConfigReadFileFn>,
    pub config_write_file: Option<ConfigWriteFileFn>,
```
3. ENGINE_OPS init + both test op-structs: add `config_read_file: None, config_write_file: None,` in the appended position (search the init + the two test `S2EngineOps { ... }` literals for `db_data_dir`).
4. Natives (mirror `s2_config_read_raw` at v8host.rs:4241 — read a `*const c_char` arg, call the op, return the string or null; write takes two args, returns nothing):
```rust
fn s2_config_read_file(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rv.set_null();
        if args.length() < 1 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        ENGINE_OPS.with(|c| {
            let ops = c.get();
            if let Some(func) = ops.config_read_file {
                let cname = std::ffi::CString::new(name).unwrap_or_default();
                let p = func(cname.as_ptr());
                if !p.is_null() {
                    let s = unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned();
                    if let Some(v) = v8::String::new(scope, &s) { rv.set(v.into()); }
                }
            }
        });
    }));
}
fn s2_config_write_file(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if args.length() < 2 { return; }
        let name = args.get(0).to_rust_string_lossy(scope);
        let content = args.get(1).to_rust_string_lossy(scope);
        ENGINE_OPS.with(|c| {
            let ops = c.get();
            if let Some(func) = ops.config_write_file {
                let cn = std::ffi::CString::new(name).unwrap_or_default();
                let cc = std::ffi::CString::new(content).unwrap_or_default();
                func(cn.as_ptr(), cc.as_ptr());
            }
        });
    }));
}
```
(Match the exact `ENGINE_OPS.with`/CString access pattern `s2_config_read_raw` uses.)
5. Register (beside `__s2_config_read_raw` at v8host.rs:4241):
```rust
    set_native(scope, global_obj, "__s2_config_read_file", s2_config_read_file);
    set_native(scope, global_obj, "__s2_config_write_file", s2_config_write_file);
```
6. Add to the `__s2_config` module object (v8host.rs:679, the object with `getString`/`onChange`):
```javascript
    readFile:  function (name) { return __s2_config_read_file(String(name)); },
    writeFile: function (name, content) { __s2_config_write_file(String(name), String(content)); },
```

- [ ] **Step 5: Run the test + gates**

Run: `cd core && cargo test config_read_file_degrades` → PASS. Then `cd core && cargo test` (full green) + `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.

- [ ] **Step 6: `.d.ts` + commit**

Add to `packages/config/index.d.ts` on the `config` object:
```typescript
  /** Read a raw file from the configs dir (name includes its extension, e.g. "maplist.txt"). null if absent. */
  readFile(name: string): string | null;
  /** Write a raw file to the configs dir (creates/overwrites). */
  writeFile(name: string, content: string): void;
```

```bash
git add shim/include/s2script_core.h shim/src/s2script_mm.cpp core/src/v8host.rs packages/config/index.d.ts
git commit -m "feat(config): config.readFile/writeFile — raw configs-dir file access (for maplist.txt)"
```

---

### Task 2: `nominations` plugin (in `disabled/`)

**Files:**
- Create: `disabled/nominations/package.json` (with `s2script.config` for `map_cooldown`), `disabled/nominations/tsconfig.json`, `disabled/nominations/src/plugin.ts`.
- Modify: `scripts/check-plugins-typecheck.sh` — add `disabled` to the scanned base dirs (so `disabled/*` typechecks) with an empty-glob guard.

**Interfaces:**
- Consumes: `@s2script/commands` (`Commands`), `@s2script/config` (`config` incl. `readFile`/`writeFile`), `@s2script/db` (`Database`), `@s2script/menu` (`Menu`, `MenuStyle`), `@s2script/server` (`Server`), `@s2script/cs2` (`Player`), `@s2script/chat` (`Chat`).
- Produces: `sm_nominate`; the `mapvote` SQLite DB schema (consumed by rockthevote in sub-slice 2).

- [ ] **Step 1: package.json + tsconfig + gate**

`disabled/nominations/package.json`:
```json
{
  "name": "@s2script/nominations",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x",
    "config": {
      "map_cooldown": { "type": "int", "default": 5, "description": "Distinct maps that must play before a map is nominatable again." }
    }
  }
}
```
`disabled/nominations/tsconfig.json`: copy `plugins/basecommands/tsconfig.json`.

In `scripts/check-plugins-typecheck.sh`, change the base list `for base in examples plugins; do` to `for base in examples plugins disabled; do` and guard the inner glob so an absent/empty `disabled/` doesn't error (e.g. `[ -d "$base" ] || continue` before the `for d in "$base"/*/` loop, and skip a `$d` with no `package.json`).

- [ ] **Step 2: the plugin**

`disabled/nominations/src/plugin.ts`:
```typescript
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
```

- [ ] **Step 3: Build (typecheck) + gate + commit**

Run: `node packages/cli/dist/cli.js build disabled/nominations` (clean `.s2sp`) and `bash scripts/check-plugins-typecheck.sh` (green incl. `disabled/nominations`).
```bash
git add disabled/nominations scripts/check-plugins-typecheck.sh
git commit -m "feat(nominations): sm_nominate (fuzzy + menu) + mapvote SQLite foundation (disabled/)"
```

---

### Task 3: Sniper build + live gate

**Files:** none.

- [ ] **Step 1: Sniper build (Task 1 shim + core)**

Run:
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expect exit 0, no `error:`, GLIBC `libs2script_core` ≤ 2.30 / `s2script.so` ≤ 2.14.

- [ ] **Step 2: Deploy — the base suite to plugins/, nominations to plugins/disabled/**

```bash
mkdir -p dist/addons/s2script/plugins/disabled dist/addons/s2script/configs dist/addons/s2script/data
find plugins -path '*/dist/*.s2sp' -exec cp {} dist/addons/s2script/plugins/ \;
cp disabled/nominations/dist/*.s2sp dist/addons/s2script/plugins/disabled/
docker compose -f docker/docker-compose.yml restart cs2
```

- [ ] **Step 3: Live gate (bots-provable)**

Poll `docker logs s2script-cs2 --since 3m` for `GAMEDATA VALIDATION: 12 ok`. **Confirm nominations does NOT load** (no `[nominations] onLoad` — it's in `disabled/`). Then **enable it**: `cp dist/addons/s2script/plugins/disabled/_s2script_nominations.s2sp dist/addons/s2script/plugins/` (move up) → the file-watch loads it → `[nominations] onLoad`, and `configs/maplist.txt` is auto-generated. Via rcon: `sm_nominate de_dust2` → `[nominations] Console/... nominated de_dust2` (or the cooldown reply if de_dust2 is the current map — try another pool map); `sm_nominate nocturn` with a `jb_nocturnal` line in maplist.txt → resolves to it (fuzzy); `sm_map de_dust2` → a new `map_history` row + nominations cleared (confirm via a follow-up `sm_nominate de_dust2` now rejected as cooldown/current). `RestartCount=0`, no crash. (A player-facing nominate menu + the announce visible in chat is a human-client nicety; the DB + resolve + cooldown are rcon-provable.)

- [ ] **Step 4: Commit any live-gate fixes** as `fix(nominations): <what> (live gate)` with the session trailer.

---

## Self-Review

**Spec coverage:**
- `config.readFile`/`writeFile` (shim op pair + module + `.d.ts` + traversal-safe) → Task 1 ✅
- `maplist.txt` format + parser (stock + `name:workshopId`) + auto-gen template → Task 2 (`parseMaplist`/`loadPool`/`MAPLIST_TEMPLATE`) ✅
- Shared `mapvote` DB (`map_history` + `nominations`), `CREATE IF NOT EXISTS`, onLoad recording (deduped new-map) + nominations clear → Task 2 (`recordMapStart`) ✅
- Cooldown = last N distinct maps → Task 2 (`cooldownSet`, the GROUP BY MAX(id) query) ✅
- `sm_nominate <partial>` best-guess (exact-else-substring), 0/1/>1 handling → Task 2 (`resolveMap` + the command) ✅
- `sm_nominate` no-arg menu + disambiguation menu + one-per-player + announce → Task 2 (`nominateMenu`/`mapMenu`/`nominate`) ✅
- `map_cooldown` config → Task 2 (package.json) ✅
- Shipped in `disabled/`, loader skips it, opt-in by moving up → Task 2 (source) + Task 3 (deploy + verify not-loaded-then-loaded) ✅
- Boundary + one sniper → Tasks 1–2 run gates, Task 3 snipers ✅
- Deferred (rockthevote, workshop change, nomination cap) → not built ✅

**Placeholder scan:** no TBD/etc. Task 2 Step 1 gives the concrete gate edit (`for base in examples plugins disabled`, `[ -d "$base" ] || continue`, skip no-`package.json` dirs). All code blocks complete.

**Type consistency:** `MapEntry {name, workshopId}`, `resolveMap→MapEntry[]`, `nominate(slot, name)`, `cooldownSet`/`nominatedSet→Set<string>` are consistent across Task 2. `config.readFile(name)`/`writeFile(name, content)` match Task 1 (module + `.d.ts`) and Task 2 (`loadPool`). The DB is `@s2script/db`'s `Database.open`/`query`/`execute` (existing signatures). The ops append after `db_data_dir` in the same position across the C header, Rust mirror, and both test structs (Task 1 Steps 3–4).
