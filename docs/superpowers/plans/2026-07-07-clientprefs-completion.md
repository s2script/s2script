# clientprefs Completion — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or a Workflow). Steps use checkbox (`- [ ]`). Tasks are SEQUENTIAL and DEPENDENT — implement in order, commit each. Builds on the merged clientprefs slice.

**Goal:** Complete `@s2script/cookies` to SM-parity (sans menu): rename the module, fix empty-string-vs-miss, add `GetClientCookieTime`, `SetAuthIdCookie` (full offline), and `onCached`.

**Architecture:** All in the existing cookie cache (`core/src/cookies.rs`) + natives (`core/src/v8host.rs`) + the module prelude + the `clientprefs`/`clientprefs-demo` plugins. Two new mechanisms: a core offline-write queue drained by the plugin (`setAuthId`), and a cookie-cached mux + pending queue fanned out post-`frame_async_drain` (`onCached`).

**Tech Stack:** Rust (HashMap/Vec, `event_mux::EventMux` reuse — no new deps), rusty_v8, TypeScript.

## Global Constraints

- **Core engine-generic:** `cookies.rs`, the natives, `@s2script/cookies` — NO game/CS2 names (`scripts/check-core-boundary.sh`).
- **No `S2EngineOps` op / no shim change** — the natives are `set_native`'d (like the existing cookie natives); the only non-native core touch is one call in `ffi.rs::s2script_core_dispatch_game_frame`.
- **Degrade-never-crash:** every native `catch_unwind(AssertUnwindSafe(..))`.
- **`dispatch_pending_cookie_cached` runs with HOST FREE** (called AFTER `frame_async_drain()` returns) and mirrors `dispatch_client_event` (`core/src/v8host.rs:1888`) exactly — snapshot-release, `try_borrow_mut`, per-sub `is_live` + context clone + HandleScope/ContextScope/TryCatch.
- **Bots** (`steamId "0"`) skipped by the module `get`/`set`/`setAuthId`/`areCached`/`getTime`.
- **`cargo test` serial** (`.cargo/config.toml`) — do not change.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`; use a `-F -` heredoc (no backticks).

## File Structure
- `packages/clientprefs/` → `packages/cookies/` (renamed), `packages/cookies/index.d.ts` (extended).
- `core/src/cookies.rs` — `Entry.updated`, `get`→`Option`, `set`/`load` take `updated`, `get_time`, `set_authid`, `take_offline_writes`, extend `reset`.
- `core/src/v8host.rs` — updated cookie natives + new natives + the `__s2pkg_cookies` prelude (renamed) + `COOKIE_CACHED_MUX`/`COOKIE_CACHED_PENDING` + `dispatch_pending_cookie_cached` + teardown/shutdown.
- `core/src/ffi.rs` — call `dispatch_pending_cookie_cached()` after `frame_async_drain()`.
- `plugins/clientprefs/src/plugin.ts` — pass `updated` on load; call `__s2_cookie_dispatch_cached` after load; drain offline writes on frame.
- `plugins/clientprefs-demo/src/plugin.ts` — extend the live-gate demo.

---

### Task 1: Rename the module `@s2script/clientprefs` → `@s2script/cookies`

**Files:** `packages/clientprefs/*` → `packages/cookies/*`; `core/src/v8host.rs` (prelude global); `plugins/clientprefs-demo/src/plugin.ts` (import); the in-isolate tests.

- [ ] **Step 1: Move the package.** `git mv packages/clientprefs packages/cookies`. Edit `packages/cookies/package.json` → `"name": "@s2script/cookies"`. (`index.d.ts` content is unchanged except any `@s2script/clientprefs` self-reference in comments.)
- [ ] **Step 2: Rename the prelude global.** In `core/src/v8host.rs`, rename `globalThis.__s2pkg_clientprefs = { Cookies: __s2_Cookies, CookieAccess: {…} }` → `globalThis.__s2pkg_cookies = { … }`. (The `s2require` rule maps `@s2script/cookies` → `__s2pkg_cookies` automatically.)
- [ ] **Step 3: Update consumers.** In `plugins/clientprefs-demo/src/plugin.ts`, change `import { Cookies } from "@s2script/clientprefs"` → `from "@s2script/cookies"`. In the in-isolate tests (`core/src/v8host.rs` `frame_tests`: `clientprefs_module_*`), change `require("@s2script/clientprefs")` → `require("@s2script/cookies")`. (The `clientprefs` PLUGIN uses raw natives, not the module import — no change there; its package.json `name` stays `@s2script/clientprefs`.)
- [ ] **Step 4: Build + test.** `cargo test --manifest-path core/Cargo.toml` (module tests pass under the new name); `node packages/cli/dist/cli.js build plugins/clientprefs-demo` (typecheck resolves `@s2script/cookies`).
- [ ] **Step 5: Commit.**
```bash
git add packages/cookies packages/clientprefs core/src/v8host.rs plugins/clientprefs-demo core/src/lib.rs 2>/dev/null; git add -A
git commit -F - <<'EOF'
refactor(cookies): rename module @s2script/clientprefs -> @s2script/cookies

Resolves the module<->plugin name collision (review Minor 2): the capability
module is @s2script/cookies (like entity/timers/db/clients); the base plugin
keeps @s2script/clientprefs (SM's plugin name). Prelude __s2pkg_clientprefs ->
__s2pkg_cookies; demo + in-isolate tests updated.

Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn
EOF
```

---

### Task 2: Empty-string-vs-miss + `GetClientCookieTime`

**Files:** `core/src/cookies.rs`, `core/src/v8host.rs` (natives + prelude), `packages/cookies/index.d.ts`.

**Interfaces — Produces:** `cookies::get(steamid,name) -> Option<String>`; `set(steamid,name,value,updated)`; `load(steamid,name,value,updated)`; `get_time(steamid,name) -> i64`. Module: `Cookies.get` (default only on true miss), `Cookies.getTime(client, cookie) -> number`.

- [ ] **Step 1: `cookies.rs` — `Entry.updated` + `get`→`Option` + `get_time`.** Change `struct Entry { value: String, dirty: bool }` → `{ value: String, dirty: bool, updated: i64 }`. Update `set`/`load` to take `updated: i64` and store it. Change `get` to return `Option<String>`:
```rust
pub fn get(steamid: &str, name: &str) -> Option<String> {
    CACHE.with(|c| c.borrow().get(steamid).and_then(|cc| cc.entries.get(name)).map(|e| e.value.clone()))
}
pub fn set(steamid: &str, name: &str, value: &str, updated: i64) {
    CACHE.with(|c| { let mut m=c.borrow_mut(); let cc=m.entry(steamid.to_string()).or_default();
        cc.entries.insert(name.to_string(), Entry{value:value.to_string(), dirty:true, updated}); });
}
pub fn load(steamid: &str, name: &str, value: &str, updated: i64) { /* same but dirty:false */ }
pub fn get_time(steamid: &str, name: &str) -> i64 {
    CACHE.with(|c| c.borrow().get(steamid).and_then(|cc| cc.entries.get(name)).map(|e| e.updated).unwrap_or(0))
}
```
Update `get_dirty` (unchanged shape) and the existing unit tests to the new signatures (pass an `updated` like `0`; add asserts: `get` returns `None` on miss, `Some("")` for a stored `""`; `get_time` returns the stored value).

- [ ] **Step 2: `v8host.rs` natives.** `__s2_cookie_get` returns `undefined` on `None`:
```rust
match crate::cookies::get(&sid, &name) {
    Some(v) => { if let Some(s) = v8::String::new(scope, &v) { rv.set(s.into()); } }
    None => { rv.set(v8::undefined(scope).into()); }
}
```
`__s2_cookie_set`/`__s2_cookie_load` gain a 4th arg `updated` (`args.get(3).integer_value(scope).unwrap_or(0)`), passed to `cookies::set`/`load`. Add `__s2_cookie_get_time(steamid,name) -> Number` (register it). (`get_dirty` marshalling unchanged.)

- [ ] **Step 3: Prelude + `.d.ts`.** In `__s2pkg_cookies`: `get` uses `var v = __s2_cookie_get(client.steamId, cookie.name); return v === undefined ? cookie.default : v;`; `set` passes a timestamp: `__s2_cookie_set(client.steamId, cookie.name, String(value), Math.floor(Date.now()/1000));`; add `getTime: function (client, cookie) { return (!client || client.steamId === "0") ? 0 : __s2_cookie_get_time(client.steamId, cookie.name); }`. In `packages/cookies/index.d.ts`, add `getTime(client: Client, cookie: Cookie): number;`.

- [ ] **Step 4: Plugin load carries `updated`.** In `plugins/clientprefs/src/plugin.ts` `loadCookies`, `__s2_cookie_load` gains the row's `updated` (add `updated` to the SELECT: `SELECT name, value, updated FROM cookies …`; call `__s2_cookie_load(steamId, name, value, Number(row.updated))`). Update the `declare function __s2_cookie_load` signature.

- [ ] **Step 5: Tests + build.** `cargo test …` (unit + an in-isolate test: `set(...,"")` → `get` returns `""` not default; `getTime` reads back the set timestamp). Build both plugins.

- [ ] **Step 6: Commit** (`feat(cookies): empty-string-vs-miss + getTime`).

---

### Task 3: `SetAuthIdCookie` — full offline (queue + plugin drain)

**Files:** `core/src/cookies.rs`, `core/src/v8host.rs`, `packages/cookies/index.d.ts`, `plugins/clientprefs/src/plugin.ts`.

**Interfaces — Produces:** `cookies::set_authid(steamid,name,value,updated)` (cache write + queue push); `cookies::take_offline_writes() -> Vec<(String,String,String,i64)>`. Natives `__s2_cookie_set_authid`, `__s2_cookie_take_offline_writes` (→ JS array of `[steamid,name,value,updated]`). Module `Cookies.setAuthId(steamId, cookie, value)`.

- [ ] **Step 1: `cookies.rs` — the offline queue.** Add `thread_local! { static OFFLINE: RefCell<Vec<(String,String,String,i64)>> = RefCell::new(Vec::new()); }`.
```rust
pub fn set_authid(steamid: &str, name: &str, value: &str, updated: i64) {
    set(steamid, name, value, updated);   // cache write (dirty) — an online SteamID also flushes on disconnect
    OFFLINE.with(|q| q.borrow_mut().push((steamid.to_string(), name.to_string(), value.to_string(), updated)));
}
pub fn take_offline_writes() -> Vec<(String,String,String,i64)> {
    OFFLINE.with(|q| std::mem::take(&mut *q.borrow_mut()))
}
```
Extend `reset()` to also clear `OFFLINE`. Add a unit test (push via `set_authid` → `take_offline_writes` returns it + clears; a second `take` is empty).

- [ ] **Step 2: Natives.** `__s2_cookie_set_authid(steamid,name,value,updated)` → `cookies::set_authid`. `__s2_cookie_take_offline_writes()` builds a `v8::Array` of 4-element `v8::Array`s (`[steamid,name,value,updated]`; strings + a Number) — mirror the `get_dirty` object-building loop but as nested arrays. Register both.

- [ ] **Step 3: Module + `.d.ts`.** `Cookies.setAuthId: function (steamId, cookie, value) { if (steamId === "0") return; __s2_cookie_set_authid(String(steamId), cookie.name, String(value), Math.floor(Date.now()/1000)); }`. `.d.ts`: `setAuthId(steamId: string, cookie: Cookie, value: string): void;`.

- [ ] **Step 4: Plugin drains the queue each frame.** In `plugins/clientprefs/src/plugin.ts`, import `OnGameFrame` from `@s2script/frame`, and in `onLoad` subscribe a handler that drains + upserts:
```ts
import { OnGameFrame } from "@s2script/frame";
declare function __s2_cookie_take_offline_writes(): Array<[string, string, string, number]>;
// in onLoad, after hooking load/save:
OnGameFrame(() => {
  const writes = __s2_cookie_take_offline_writes();
  if (writes.length === 0) return;   // cheap idle check
  for (const [steamid, name, value, updated] of writes) {
    db!.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?, ?, ?, ?)",
      [steamid, name, value, updated]).catch((e) => console.log("[clientprefs] offline-write ERROR: " + String(e)));
  }
});
```
(Verify the exact `OnGameFrame` subscribe signature against `@s2script/frame`'s `.d.ts` / an existing user; if it needs `SubscribeOptions`, use the default/Post phase.)

- [ ] **Step 5: Tests + build.** `cargo test …` (unit + an in-isolate: `setAuthId` writes the cache AND `take_offline_writes` returns the row). Build both plugins.

- [ ] **Step 6: Commit** (`feat(cookies): setAuthId with an offline-write queue drained by the plugin`).

---

### Task 4: `onCached` (`OnClientCookiesCached`) — post-drain fan-out

**Files:** `core/src/cookies.rs` (or `v8host.rs` for the mux — keep the mux in `v8host.rs` with the other muxes), `core/src/v8host.rs`, `core/src/ffi.rs`, `packages/cookies/index.d.ts`, `plugins/clientprefs/src/plugin.ts`.

**Interfaces — Produces:** natives `__s2_cookie_on_cached(handler)`, `__s2_cookie_dispatch_cached(slot)`; `pub(crate) fn dispatch_pending_cookie_cached()`. Module `Cookies.onCached(handler: (client) => void)`.

**Mirror:** `CLIENT_MUX` (`core/src/v8host.rs:366`) for the mux static + teardown + shutdown-reset; `s2_client_subscribe` (`:2567`) for the subscribe native; `dispatch_client_event` (`:1888`) for the fan-out body.

- [ ] **Step 1: Statics.** In `v8host.rs`, add near `CLIENT_MUX`:
```rust
static COOKIE_CACHED_MUX: std::cell::RefCell<crate::event_mux::EventMux<v8::Global<v8::Function>>>
    = std::cell::RefCell::new(crate::event_mux::EventMux::new());
static COOKIE_CACHED_PENDING: std::cell::RefCell<Vec<i32>> = std::cell::RefCell::new(Vec::new());
```

- [ ] **Step 2: Subscribe native.** `s2_cookie_on_cached` — mirror `s2_client_subscribe` but subscribe under the name `""` into `COOKIE_CACHED_MUX`, and record the sub in the ledger the same way (`EventSub`). Register `__s2_cookie_on_cached`.

- [ ] **Step 3: Dispatch-enqueue native.** `s2_cookie_dispatch_cached(slot)` — just `COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().push(args.get(0).int32_value(scope).unwrap_or(-1)))`, `catch_unwind`-wrapped (no HOST access — safe under the drain borrow). Register `__s2_cookie_dispatch_cached`.

- [ ] **Step 4: The post-drain fan-out.**
```rust
/// Drain COOKIE_CACHED_PENDING and fan each slot out to the onCached subscribers. Called from ffi.rs
/// AFTER frame_async_drain() (HOST free → no re-entrancy). Mirrors dispatch_client_event.
pub(crate) fn dispatch_pending_cookie_cached() {
    let slots: Vec<i32> = COOKIE_CACHED_PENDING.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if slots.is_empty() { return; }
    let snap = COOKIE_CACHED_MUX.with(|m| m.borrow().snapshot(""));
    if snap.is_empty() { return; }
    for slot in slots {
        HOST.with(|h| {
            let Ok(mut borrow) = h.try_borrow_mut() else { return };
            let Some(host) = borrow.as_mut() else { return };
            for (owner, generation, handler_g) in &snap {
                // per-sub is_live + context clone + HandleScope/ContextScope/TryCatch + call handler(slot)
                // — COPY the body of dispatch_client_event's Phase-2 loop verbatim (it passes a single
                // Integer slot to the handler and ignores the return).
            }
        });
    }
}
```
(The agent must lift `dispatch_client_event`'s per-subscriber invocation block verbatim so the HOST/scope/TryCatch discipline matches. Note `snap` is re-usable across slots — the `Global<Function>`s are clones.)

- [ ] **Step 5: Teardown + shutdown.** In `unload_plugin` (near `CLIENT_MUX.with(|m| m.borrow_mut().remove_by_owner(id))`), add `COOKIE_CACHED_MUX.with(|m| m.borrow_mut().remove_by_owner(id));`. In `shutdown()`, add `COOKIE_CACHED_MUX.with(|m| *m.borrow_mut() = crate::event_mux::EventMux::new()); COOKIE_CACHED_PENDING.with(|q| q.borrow_mut().clear());`.

- [ ] **Step 6: `ffi.rs`.** In `s2script_core_dispatch_game_frame`, the Post branch, AFTER `frame_async_drain()` (and before/after `poll_plugins()`):
```rust
v8host::frame_async_drain();
v8host::dispatch_pending_cookie_cached();  // HOST free here — fan out queued onCached
crate::loader::poll_plugins();
```

- [ ] **Step 7: Module + `.d.ts` + plugin.** Prelude: `onCached: function (h) { __s2_cookie_on_cached(function (slot) { h(globalThis.__s2pkg_clients.Clients.fromSlot(slot)); }); }`. `.d.ts`: `onCached(handler: (client: Client) => void): void;`. In `plugins/clientprefs/src/plugin.ts` `loadCookies`, after `__s2_cookie_mark_cached(steamId)`, add `__s2_cookie_dispatch_cached(client.slot);` (+ its `declare function`).

- [ ] **Step 8: In-isolate test.** A plugin subscribes `Cookies.onCached` (or `__s2_cookie_on_cached`) → `__s2_cookie_dispatch_cached(5)` → call `dispatch_pending_cookie_cached()` directly (test helper) → assert the handler fired with slot 5. Mirror an existing mux test.

- [ ] **Step 9: Tests + build.** `cargo test …` green; build both plugins.

- [ ] **Step 10: Commit** (`feat(cookies): onCached via a post-drain pending-dispatch`).

---

## Post-implementation (controller / me — NOT a workflow task)
1. **Sniper rebuild.** 2. **Deploy** (recreate `data/`+`configs/`, copy `.s2sp`, restart). 3. **Live gate (bots):** `clientprefs-demo` — a `""` cookie reads back `""` (not default); `setAuthId` for a fake offline SteamID persists across restart (the offline drain wrote it); `getTime` logs a nonzero. `onCached` real fire = deferred human test. 4. **Gates:** boundary, full `cargo test`, plugins-typecheck. 5. **Final opus review** → merge + push.

## Self-review notes
- **Spec coverage:** rename (T1), empty-string + getTime (T2), setAuthId offline (T3), onCached (T4). Skipped YAGNI (access/fast-reconnect/two-table) in no task. ✓
- **Type consistency:** `get`→`Option`/native `undefined`/module `=== undefined`; `set`/`load`/`set_authid` all take `updated:i64` ↔ the natives' 4th arg ↔ module `Math.floor(Date.now()/1000)` / plugin `row.updated`. `onCached`/`dispatch_cached`/`dispatch_pending_cookie_cached` names consistent. `@s2script/cookies` used everywhere post-rename.
