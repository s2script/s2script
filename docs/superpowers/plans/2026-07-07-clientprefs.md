# clientprefs (Cookie Storage/API Layer) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or a Workflow) to implement this plan task-by-task. Steps use checkbox (`- [ ]`). Tasks are SEQUENTIAL and DEPENDENT — implement in order, commit each.

**Goal:** SourceMod-parity client-preference cookies — persistent per-player key→value strings keyed by SteamID, loaded on connect and saved (dirty-only) on disconnect — via `@s2script/clientprefs` over the SQLite DB primitive.

**Architecture:** A core host-global cookie cache (`core/src/cookies.rs`, pure) + thin natives (mirrors the admin/ban caches — no engine op, no shim change). `@s2script/clientprefs` (the `register`/`get`/`set`/`areCached` API) over the natives. The `clientprefs` base plugin owns the DB lifecycle (load on `Clients.onPutInServer`, dirty-save on `Clients.onDisconnect`) using `@s2script/db` + `@s2script/clients`. A `clientprefs-demo` plugin proves the round-trip for a synthetic SteamID.

**Tech Stack:** Rust (a `HashMap` cache — no new deps), rusty_v8, TypeScript.

## SCOPE NOTE — onCached deferred

The spec (`docs/superpowers/specs/2026-07-07-clientprefs-design.md`) defers **`onCached`** (`OnClientCookiesCached`): it is a cross-context event whose trigger (the plugin's async load completion) runs under the isolate borrow, so a JS-triggered fan-out hits the `try_borrow_mut` re-entrancy skip — it needs a post-drain pending-dispatch mechanism (a follow-up). This slice ships `areCached`. Do NOT build `onCached` or a cookie notify-mux.

## Global Constraints

- **Core is engine-generic.** `core/src/cookies.rs`, the natives, and `@s2script/clientprefs` contain NO game/CS2 names. `scripts/check-core-boundary.sh` must stay green. (SteamID + opaque string cookies are Source2-generic.)
- **Host-global cache, mirrors admin/ban.** The cache is a core thread-local `HashMap`; the natives are `set_native`'d (NOT `S2EngineOps` ops — no ABI change, no shim change).
- **Degrade-never-crash.** Every native body wraps in `std::panic::catch_unwind(AssertUnwindSafe(..))` (like the existing natives). A missing entry reads `""`; nothing panics into the engine.
- **Bots** (`steamId === "0"`) are skipped by the module's `get`/`set` and by the plugin's load/save.
- **Dirty-tracked save.** `set` marks an entry dirty; `load` does not; the plugin flushes only dirty entries on disconnect (SM-parity).
- **`cargo test` runs serial** (`.cargo/config.toml` `RUST_TEST_THREADS=1`, already present) — do not change it.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`. No backticks in `git commit -m` (use `-F -` heredoc).

## File Structure

- `core/src/cookies.rs` (NEW) — the pure host-global cookie cache + unit tests.
- `core/src/lib.rs` (MODIFY) — `mod cookies;`.
- `core/src/v8host.rs` (MODIFY) — the `__s2_cookie_*` natives + registration + the `__s2pkg_clientprefs` prelude runtime + in-isolate tests.
- `packages/clientprefs/{package.json,index.d.ts}` (NEW) — the `@s2script/clientprefs` types package.
- `plugins/clientprefs/{package.json,tsconfig.json,src/plugin.ts}` (NEW) — the base plugin (DB lifecycle).
- `plugins/clientprefs-demo/{package.json,tsconfig.json,src/plugin.ts}` (NEW) — the synthetic-SteamID live-gate demo.

---

### Task 1: `core/src/cookies.rs` — the pure host-global cookie cache

**Files:** Create `core/src/cookies.rs`; Modify `core/src/lib.rs` (`mod cookies;`).

**Interfaces — Produces:**
```rust
pub fn get(steamid: &str, name: &str) -> String;      // "" on miss
pub fn set(steamid: &str, name: &str, value: &str);   // marks dirty
pub fn load(steamid: &str, name: &str, value: &str);  // NOT dirty
pub fn get_dirty(steamid: &str) -> Vec<(String, String)>;  // dirty (name, value) pairs
pub fn clear(steamid: &str);
pub fn mark_cached(steamid: &str);
pub fn is_cached(steamid: &str) -> bool;
```

- [ ] **Step 1: Write `core/src/cookies.rs`:**
```rust
//! Engine-generic host-global client-cookie cache: steamid -> { name -> (value, dirty) } plus a
//! per-client `cached` flag. Mirrors the admin/ban caches (cross-context-visible per-client string
//! KV, read/written via natives). Knows nothing about any game; holds no V8 handles.
use std::cell::RefCell;
use std::collections::HashMap;

struct Entry { value: String, dirty: bool }
#[derive(Default)]
struct ClientCookies { cached: bool, entries: HashMap<String, Entry> }

thread_local! {
    static CACHE: RefCell<HashMap<String, ClientCookies>> = RefCell::new(HashMap::new());
}

/// Cache value, or "" if the client/name is absent.
pub fn get(steamid: &str, name: &str) -> String {
    CACHE.with(|c| c.borrow().get(steamid)
        .and_then(|cc| cc.entries.get(name))
        .map(|e| e.value.clone())
        .unwrap_or_default())
}

/// Write via the API — marks the entry dirty (flushed on disconnect).
pub fn set(steamid: &str, name: &str, value: &str) {
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        let cc = m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry { value: value.to_string(), dirty: true });
    });
}

/// Write from the DB load — NOT dirty (a loaded value is not a change).
pub fn load(steamid: &str, name: &str, value: &str) {
    CACHE.with(|c| {
        let mut m = c.borrow_mut();
        let cc = m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry { value: value.to_string(), dirty: false });
    });
}

/// The dirty (name, value) pairs for a client — the disconnect flush set.
pub fn get_dirty(steamid: &str) -> Vec<(String, String)> {
    CACHE.with(|c| {
        let m = c.borrow();
        match m.get(steamid) {
            Some(cc) => cc.entries.iter()
                .filter(|(_, e)| e.dirty)
                .map(|(n, e)| (n.clone(), e.value.clone()))
                .collect(),
            None => Vec::new(),
        }
    })
}

/// Drop a client's entries (on disconnect, after the flush captures the dirty set).
pub fn clear(steamid: &str) {
    CACHE.with(|c| { c.borrow_mut().remove(steamid); });
}

/// Mark a client's cookies loaded (a zero-cookie client is still "cached").
pub fn mark_cached(steamid: &str) {
    CACHE.with(|c| { c.borrow_mut().entry(steamid.to_string()).or_default().cached = true; });
}

pub fn is_cached(steamid: &str) -> bool {
    CACHE.with(|c| c.borrow().get(steamid).map(|cc| cc.cached).unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    // NOTE: CACHE is thread-local + tests run serial (RUST_TEST_THREADS=1); use a unique steamid per
    // test so they don't observe each other's entries.
    #[test]
    fn set_get_and_dirty() {
        set("A1", "color", "red");
        assert_eq!(get("A1", "color"), "red");
        let d = get_dirty("A1");
        assert_eq!(d, vec![("color".to_string(), "red".to_string())]);
        assert_eq!(get("A1", "missing"), "");
    }
    #[test]
    fn load_is_not_dirty() {
        load("A2", "k", "v");
        assert_eq!(get("A2", "k"), "v");
        assert!(get_dirty("A2").is_empty(), "a loaded value is not dirty");
        set("A2", "k2", "v2");   // a later set IS dirty
        assert_eq!(get_dirty("A2"), vec![("k2".to_string(), "v2".to_string())]);
    }
    #[test]
    fn clear_removes_client() {
        set("A3", "k", "v");
        clear("A3");
        assert_eq!(get("A3", "k"), "");
        assert!(get_dirty("A3").is_empty());
    }
    #[test]
    fn cached_flag_tracks() {
        assert!(!is_cached("A4"));
        mark_cached("A4");        // a zero-cookie client can still be cached
        assert!(is_cached("A4"));
    }
}
```

- [ ] **Step 2: Add the module.** In `core/src/lib.rs`, add `mod cookies;` near the other `mod` lines.

- [ ] **Step 3: Run tests.** `cargo test --manifest-path core/Cargo.toml cookies::` — expect the 4 `cookies::tests::*` green (+ full suite green).

- [ ] **Step 4: Commit.**
```bash
git add core/src/cookies.rs core/src/lib.rs
git commit -F - <<'EOF'
feat(clientprefs): host-global cookie cache (core/src/cookies.rs)

steamid -> {name -> (value, dirty)} + a cached flag; get/set/load/get_dirty/
clear/mark_cached/is_cached; unit-tested. Engine-generic, V8-free (mirrors the
admin/ban caches).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 2: The `__s2_cookie_*` natives

**Files:** Modify `core/src/v8host.rs` (natives + registration + an in-isolate test).

**Interfaces — Consumes:** `cookies::*` (Task 1). **Produces (JS natives):**
- `__s2_cookie_get(steamid, name) -> string`
- `__s2_cookie_set(steamid, name, value) -> void`
- `__s2_cookie_load(steamid, name, value) -> void`
- `__s2_cookie_get_dirty(steamid) -> { [name]: value }` (a JS object)
- `__s2_cookie_clear(steamid) -> void`
- `__s2_cookie_mark_cached(steamid) -> void`
- `__s2_cookie_is_cached(steamid) -> boolean`

**Pattern:** mirror the ADMIN natives (`s2_admin_get`/`s2_admin_set` near `core/src/v8host.rs:3663`) for the string-arg / cache-access shape, and the DB query native's `v8::Object::new` + `obj.set` for building the `get_dirty` object. Every body wrapped in `catch_unwind(AssertUnwindSafe(..))`.

- [ ] **Step 1: Write the seven natives.** Shapes (verify the exact rusty_v8 calls against neighboring natives — `to_rust_string_lossy`, `v8::Boolean::new`, `v8::Object::new`, `obj.set`):
```rust
fn s2_cookie_get(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let v = crate::cookies::get(&sid, &name);
        if let Some(s) = v8::String::new(scope, &v) { rv.set(s.into()); }
    }));
}
fn s2_cookie_set(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let name = args.get(1).to_rust_string_lossy(scope);
        let val = args.get(2).to_rust_string_lossy(scope);
        crate::cookies::set(&sid, &name, &val);
    }));
}
// s2_cookie_load — identical to set but calls crate::cookies::load.
fn s2_cookie_get_dirty(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        let pairs = crate::cookies::get_dirty(&sid);
        let obj = v8::Object::new(scope);
        for (name, value) in pairs.iter() {
            let k = v8::String::new(scope, name).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            let v = v8::String::new(scope, value).unwrap_or_else(|| v8::String::new(scope, "").unwrap());
            obj.set(scope, k.into(), v.into());
        }
        rv.set(obj.into());
    }));
}
// s2_cookie_clear / s2_cookie_mark_cached — like s2_cookie_set (one string arg, call the cookies:: fn).
fn s2_cookie_is_cached(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let sid = args.get(0).to_rust_string_lossy(scope);
        rv.set(v8::Boolean::new(scope, crate::cookies::is_cached(&sid)).into());
    }));
}
```

- [ ] **Step 2: Register the natives** (near the admin-native registrations, `core/src/v8host.rs:3271`):
```rust
set_native(scope, global_obj, "__s2_cookie_get", s2_cookie_get);
set_native(scope, global_obj, "__s2_cookie_set", s2_cookie_set);
set_native(scope, global_obj, "__s2_cookie_load", s2_cookie_load);
set_native(scope, global_obj, "__s2_cookie_get_dirty", s2_cookie_get_dirty);
set_native(scope, global_obj, "__s2_cookie_clear", s2_cookie_clear);
set_native(scope, global_obj, "__s2_cookie_mark_cached", s2_cookie_mark_cached);
set_native(scope, global_obj, "__s2_cookie_is_cached", s2_cookie_is_cached);
```

- [ ] **Step 3: In-isolate test.** In `frame_tests`, load a plugin that drives the natives and assert the round-trip:
```rust
#[test]
fn cookie_natives_round_trip() {
    let _ = init(dummy_logger());
    load_plugin_js("ck", r#"
        __s2_cookie_load("S1", "a", "1");         // loaded, not dirty
        __s2_cookie_set("S1", "b", "2");          // set, dirty
        __s2_cookie_mark_cached("S1");
        var dirty = __s2_cookie_get_dirty("S1");
        globalThis.__out = __s2_cookie_get("S1","a") + "," + __s2_cookie_get("S1","b")
            + "," + __s2_cookie_is_cached("S1") + "," + Object.keys(dirty).join("|") + "=" + dirty.b;
    "#, "{}");
    assert_eq!(read_global_string("ck", "__out"), "1,2,true,b=2"); // only b is dirty
    shutdown();
}
```
(Adjust `read_global_string` / the fixture helpers to match the existing tests.)

- [ ] **Step 4: Run tests.** `cargo test --manifest-path core/Cargo.toml` — green.

- [ ] **Step 5: Commit.**
```bash
git add core/src/v8host.rs
git commit -F - <<'EOF'
feat(clientprefs): __s2_cookie_* natives over the host-global cache

get/set/load/get_dirty(->JS object)/clear/mark_cached/is_cached; catch_unwind
on every body; registered in every context (no engine op, mirrors admin).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 3: `@s2script/clientprefs` — types package + prelude runtime

**Files:** Create `packages/clientprefs/{package.json,index.d.ts}`; Modify `core/src/v8host.rs` (the `__s2pkg_clientprefs` prelude block near `__s2pkg_db` ~line 1006, + an in-isolate test).

**Interfaces — Consumes:** the `__s2_cookie_*` natives (Task 2). **Produces:** `@s2script/clientprefs` (`Cookies`, `Cookie`, `CookieAccess`, `CookieOptions`) resolved as `globalThis.__s2pkg_clientprefs` (the generic `@s2script/<name>` → `__s2pkg_<name>` rule).

- [ ] **Step 1: `packages/clientprefs/package.json`:**
```json
{ "name": "@s2script/clientprefs", "version": "0.1.0", "types": "index.d.ts" }
```

- [ ] **Step 2: `packages/clientprefs/index.d.ts`:**
```ts
/** @s2script/clientprefs — SM-parity client preference cookies. NO runtime code (injected as __s2pkg_clientprefs). */
import type { Client } from "@s2script/clients";
export enum CookieAccess { Public, Protected, Private }
export interface Cookie { readonly name: string; readonly access: CookieAccess; readonly default: string; }
export interface CookieOptions { description?: string; access?: CookieAccess; default?: string; }
export declare const Cookies: {
  /** Register (or return an existing) cookie definition. */
  register(name: string, opts?: CookieOptions): Cookie;
  /** Cache value for this client, else the cookie's default, else "". "" for bots. */
  get(client: Client, cookie: Cookie): string;
  /** Write the cache + mark dirty (flushed to the DB on disconnect). No-op for bots. */
  set(client: Client, cookie: Cookie, value: string): void;
  /** Has this client's cookies finished loading from the DB? */
  areCached(client: Client): boolean;
};
```

- [ ] **Step 3: The prelude runtime.** In `core/src/v8host.rs`, near the `__s2pkg_db` block (~line 1006), add a `__s2pkg_clientprefs` block (match the surrounding prelude string style):
```js
// --- @s2script/clientprefs: SM-parity cookies over the __s2_cookie_* host-global cache ---
var __s2_cookie_defs = {};   // per-context registry: name -> Cookie (idempotent register)
var __s2_Cookies = {
  register: function (name, opts) {
    if (__s2_cookie_defs[name]) return __s2_cookie_defs[name];
    opts = opts || {};
    var cookie = { name: name, access: (opts.access == null ? 0 : opts.access), default: (opts.default == null ? "" : String(opts.default)) };
    __s2_cookie_defs[name] = cookie;
    return cookie;
  },
  get: function (client, cookie) {
    if (!client || client.steamId === "0") return cookie.default;      // bots have no cookies
    var v = __s2_cookie_get(client.steamId, cookie.name);
    return v === "" ? cookie.default : v;
  },
  set: function (client, cookie, value) {
    if (!client || client.steamId === "0") return;                     // no-op for bots
    __s2_cookie_set(client.steamId, cookie.name, String(value));
  },
  areCached: function (client) {
    return !!client && client.steamId !== "0" && __s2_cookie_is_cached(client.steamId);
  },
};
globalThis.__s2pkg_clientprefs = { Cookies: __s2_Cookies, CookieAccess: { Public: 0, Protected: 1, Private: 2 } };
```
(The `.d.ts` `export enum CookieAccess` erases to a value object at runtime — export `CookieAccess` from the pkg object so `import { CookieAccess } from "@s2script/clientprefs"` resolves. `Cookies` is the named export; `import { Cookies } from "@s2script/clientprefs"` → `require(...).Cookies`.)

- [ ] **Step 4: In-isolate test.** In `frame_tests`, a plugin that uses the module with a fake client object:
```rust
#[test]
fn clientprefs_module_get_set_default_and_bot_skip() {
    let _ = init(dummy_logger());
    load_plugin_js("cp", r#"
        var { Cookies } = require("@s2script/clientprefs");
        var c = Cookies.register("hud", { default: "white" });
        var real = { steamId: "S9" };
        var bot  = { steamId: "0" };
        globalThis.__out = Cookies.get(real, c)                 // default (empty cache) -> "white"
            + "," + (function(){ Cookies.set(real, c, "red"); return Cookies.get(real, c); })()  // "red"
            + "," + Cookies.get(bot, c)                          // bot -> default "white"
            + "," + (function(){ Cookies.set(bot, c, "x"); return __s2_cookie_get("0","hud"); })(); // bot set is a no-op -> ""
    "#, "{}");
    assert_eq!(read_global_string("cp", "__out"), "white,red,white,");
    shutdown();
}
```

- [ ] **Step 5: Run tests.** `cargo test --manifest-path core/Cargo.toml` — green.

- [ ] **Step 6: Commit.**
```bash
git add packages/clientprefs core/src/v8host.rs
git commit -F - <<'EOF'
feat(clientprefs): @s2script/clientprefs — Cookies register/get/set/areCached

Types package + the __s2pkg_clientprefs prelude runtime over the __s2_cookie_*
natives. Bot-skip (steamId "0"); default fallback; idempotent register.
onCached deferred (see the spec/plan).

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 4: The `clientprefs` base plugin (the DB lifecycle)

**Files:** Create `plugins/clientprefs/{package.json,tsconfig.json,src/plugin.ts}`.

**Interfaces — Consumes:** `@s2script/db`, `@s2script/clients`, `@s2script/clientprefs`. Mirror an existing plugin's `package.json`/`tsconfig.json` (e.g. `plugins/reservedslots/`).

- [ ] **Step 1: `package.json`:**
```json
{ "name": "@s2script/clientprefs", "version": "0.1.0", "main": "src/plugin.ts", "s2script": { "apiVersion": "1.x" } }
```
(NOTE: the plugin's npm name matches the module package name — that is fine; they are different artifacts, and the CLI keys plugins by their built `.s2sp` id.)

- [ ] **Step 2: `tsconfig.json`** — copy `plugins/reservedslots/tsconfig.json` verbatim.

- [ ] **Step 3: `src/plugin.ts`:**
```ts
// @s2script/clientprefs (plugin) — the cookie DB lifecycle: load a client's cookies from SQLite into
// the core cache on connect, flush the dirty ones back on disconnect. The cookie API itself is the
// @s2script/clientprefs MODULE; this plugin only drives persistence.
import { Database } from "@s2script/db";
import { Clients, Client } from "@s2script/clients";

// The core natives (injected globals; not in the module's typed surface).
declare function __s2_cookie_load(steamid: string, name: string, value: string): void;
declare function __s2_cookie_mark_cached(steamid: string): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;
declare function __s2_cookie_clear(steamid: string): void;

let db: Database | null = null;

export async function onLoad(): Promise<void> {
  try {
    db = await Database.open("clientprefs");
    await db.execute(
      "CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))"
    );
    Clients.onPutInServer(loadCookies);
    Clients.onDisconnect(saveCookies);
    console.log("[clientprefs] onLoad — table ready, lifecycle hooked");
  } catch (e) {
    console.log("[clientprefs] onLoad ERROR: " + String(e));
  }
}

async function loadCookies(client: Client): Promise<void> {
  if (!db || client.steamId === "0") return;   // skip bots
  const steamId = client.steamId;
  try {
    const rows = await db.query("SELECT name, value FROM cookies WHERE steamid = ?", [steamId]);
    for (const row of rows) __s2_cookie_load(steamId, String(row.name), String(row.value));
    __s2_cookie_mark_cached(steamId);
  } catch (e) {
    console.log("[clientprefs] load ERROR for " + steamId + ": " + String(e));
  }
}

async function saveCookies(client: Client): Promise<void> {
  if (!db || client.steamId === "0") return;   // skip bots
  const steamId = client.steamId;
  const dirty = __s2_cookie_get_dirty(steamId);   // capture synchronously
  __s2_cookie_clear(steamId);                     // then clear (writes below use the captured values)
  const now = Math.floor(Date.now() / 1000);
  try {
    for (const name of Object.keys(dirty)) {
      await db.execute(
        "INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
        [steamId, name, dirty[name], now]
      );
    }
  } catch (e) {
    console.log("[clientprefs] save ERROR for " + steamId + ": " + String(e));
  }
}

export function onUnload(): void { console.log("[clientprefs] onUnload"); }
```
(If `Date.now()` is unavailable in the plugin sandbox, use `0` for `updated` — it is not read this slice. Verify against the typecheck; the sandbox lib is `es2020`, so `Date.now()` should exist. If the typecheck rejects the `declare function` globals, add them to the plugin's ambient types the same way other plugins reference injected natives, or check `packages/globals/globals.d.ts`.)

- [ ] **Step 4: Build it.** `node packages/cli/dist/cli.js build plugins/clientprefs` — expect a typecheck pass + `plugins/clientprefs/dist/_s2script_clientprefs.s2sp`.

- [ ] **Step 5: Commit.**
```bash
git add plugins/clientprefs
git commit -F - <<'EOF'
feat(clientprefs): the base plugin — DB load/save lifecycle

CREATE TABLE cookies; load a client's cookies into the core cache on
Clients.onPutInServer; dirty-flush + clear on Clients.onDisconnect (capture-
then-clear). Skips bots. Uses @s2script/db + @s2script/clients.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 5: `clientprefs-demo` — the synthetic-SteamID live-gate proof

**Files:** Create `plugins/clientprefs-demo/{package.json,tsconfig.json,src/plugin.ts}`.

The real lifecycle needs a human client (bots have `steamId "0"`), so the demo drives the full cache+DB round-trip for a FAKE SteamID via the raw natives + `@s2script/db` — a boot counter that persists across restart.

- [ ] **Step 1: `package.json`:**
```json
{ "name": "@demo/clientprefs-demo", "version": "0.1.0", "main": "src/plugin.ts", "s2script": { "apiVersion": "1.x" } }
```

- [ ] **Step 2: `tsconfig.json`** — copy `plugins/reservedslots/tsconfig.json` verbatim.

- [ ] **Step 3: `src/plugin.ts`:**
```ts
// clientprefs-demo — proves the cookie cache + DB round-trip for a synthetic SteamID (bots have no
// cookies, so the real client lifecycle is a deferred human-client test). A boot counter climbs
// across restarts: load DB -> cache, get+increment+set, flush dirty -> DB.
import { Database } from "@s2script/db";
import { Cookies } from "@s2script/clientprefs";

declare function __s2_cookie_load(steamid: string, name: string, value: string): void;
declare function __s2_cookie_get_dirty(steamid: string): Record<string, string>;

const FAKE = "76561199999999999";

export async function onLoad(): Promise<void> {
  try {
    const db = await Database.open("clientprefs");
    await db.execute("CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))");
    // load the fake client's cookies into the cache
    const rows = await db.query("SELECT name, value FROM cookies WHERE steamid = ?", [FAKE]);
    for (const row of rows) __s2_cookie_load(FAKE, String(row.name), String(row.value));
    // register + get + increment + set (a fake Client is just { steamId })
    const fakeClient = { steamId: FAKE } as any;
    const boots = Cookies.register("demo_boots", { default: "0" });
    const n = parseInt(Cookies.get(fakeClient, boots) || "0", 10) + 1;
    Cookies.set(fakeClient, boots, String(n));
    // flush the dirty set to the DB (what the clientprefs plugin does on disconnect)
    const dirty = __s2_cookie_get_dirty(FAKE);
    for (const name of Object.keys(dirty)) {
      await db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)", [FAKE, name, dirty[name], 0]);
    }
    console.log("[clientprefs-demo] onLoad — demo_boots=" + n + " (persisted via cookie cache + DB)");
    await db.close();
  } catch (e) {
    console.log("[clientprefs-demo] onLoad ERROR: " + String(e));
  }
}

export function onUnload(): void { console.log("[clientprefs-demo] onUnload"); }
```

- [ ] **Step 4: Build it.** `node packages/cli/dist/cli.js build plugins/clientprefs-demo` — expect a typecheck pass + a `.s2sp`.

- [ ] **Step 5: Commit.**
```bash
git add plugins/clientprefs-demo
git commit -F - <<'EOF'
feat(clientprefs): clientprefs-demo (synthetic-SteamID persist-across-restart)

Boot counter through the cookie cache + DB for a fake SteamID (bots have no
cookies): load DB->cache, get+increment+set, flush->DB. demo_boots climbs each
boot, proving the cookie stack end-to-end without a human client.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

## Post-implementation (controller / me — NOT a workflow task)

1. **Sniper rebuild** (core natives + prelude): `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`.
2. **Deploy**: recreate `dist/addons/s2script/{configs,data}` (writable), copy all `.s2sp` (incl. clientprefs + clientprefs-demo), `docker compose -f docker/docker-compose.yml restart cs2`. (The `data/` RW mount already exists from the DB slice.)
3. **Live gate (bots-provable):** `[clientprefs] onLoad — table ready, lifecycle hooked`; `[clientprefs-demo] onLoad — demo_boots=1`; then `docker compose restart cs2` → `demo_boots=2` (the cookie persisted across restart through the cache + DB). No crash, gamedata 11/0.
4. **Gates:** `scripts/check-core-boundary.sh`, full `cargo test`, `scripts/check-plugins-typecheck.sh`.
5. **Final whole-branch review** (opus), then merge `slice-clientprefs` → main + push.

## Self-review notes

- **Spec coverage:** cache + natives (T1/T2), `@s2script/clientprefs` API (T3), the plugin DB lifecycle (T4), the live-gate demo (T5). `onCached`, the menu, `SetAuthIdCookie`, access enforcement remain deferred (in no task). ✓
- **Placeholder scan:** none — every step has concrete code/commands.
- **Type consistency:** `get/set/load/get_dirty/clear/mark_cached/is_cached` (Rust) ↔ the `__s2_cookie_*` natives ↔ the `__s2pkg_clientprefs` runtime ↔ `Cookies.register/get/set/areCached` (`.d.ts`). `Cookie{name,access,default}`/`CookieAccess{Public,Protected,Private}` consistent across the `.d.ts` + the prelude. The plugin + demo reference the raw natives via `declare function`.
