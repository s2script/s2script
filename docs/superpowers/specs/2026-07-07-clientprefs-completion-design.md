# clientprefs Completion — Design

**Status:** Approved (brainstorm — design + both forks decided by the user), ready for the plan.
**Slice:** close the bounded clientprefs deferrals to SM-parity (minus the menu). Builds on the merged clientprefs slice (`docs/superpowers/specs/2026-07-07-clientprefs-design.md`).

## Goal

Complete the `@s2script/cookies` API to SourceMod parity (sans the menu): fix the two review Minors (the name collision, empty-string-vs-miss), and add `onCached`, `SetAuthIdCookie` (full offline), and `GetClientCookieTime`. Explicitly skip the YAGNI deferrals (access enforcement, fast-reconnect tracking, two-table normalization).

## The five items

### 1. Rename the module `@s2script/clientprefs` → `@s2script/cookies` (review Minor 2)
The module (the capability API) and the base plugin can't share `@s2script/clientprefs`. **Decision (user):** the **module** becomes `@s2script/cookies` (capability-named, like `entity`/`timers`/`db`/`clients`); the **plugin** keeps `@s2script/clientprefs` (SM's plugin name). Consumers write `import { Cookies } from "@s2script/cookies"`. Rename: `packages/clientprefs` → `packages/cookies`, the prelude global `__s2pkg_clientprefs` → `__s2pkg_cookies`, the demo's import, and the in-isolate tests. Only the demo imports it today, so it's cheap.

### 2. Empty-string vs miss (review Minor 3)
`cookies::get` returns `Option<String>` (`None` = the name isn't present; `Some(v)` = present, including `Some("")`). The native `__s2_cookie_get` returns **`undefined`** for `None` and the string (incl. `""`) for `Some`. The module: `var v = __s2_cookie_get(...); return v === undefined ? cookie.default : v;`. A deliberately-stored `""` now round-trips instead of reading back as the default.

### 3. `GetClientCookieTime`
The cache `Entry` gains an `updated: i64` (unix seconds), set by `set`/`load`/`set_authid`. A `__s2_cookie_get_time(steamid, name) -> number` native (0 if absent). `Cookies.getTime(client, cookie): number` on the module. (Timestamps are passed IN by the caller — the core has no clock; `set`/`set_authid` take an `updated` arg, and `load` carries the DB's stored `updated`.)

### 4. `SetAuthIdCookie` — full offline support (queue + drain)
`Cookies.setAuthId(steamId, cookie, value)` writes a cookie by **SteamID**, for a disconnected player too. **Decision (user):** full offline. Mechanism:
- `__s2_cookie_set_authid(steamid, name, value, updated)` — writes the cache (dirty, so an *online* SteamID also flushes normally on disconnect) **and** pushes `(steamid, name, value, updated)` to a core `COOKIE_OFFLINE_WRITES` queue.
- `__s2_cookie_take_offline_writes() -> [[steamid, name, value, updated], …]` — drains + clears the queue.
- The `clientprefs` plugin drains it on each `OnGameFrame` (a cheap empty-check when idle) and DB-upserts each row (`INSERT OR REPLACE INTO cookies …`). This is the only path that can persist an offline write (the plugin owns the DB connection; the module can't).

### 5. `onCached` (`OnClientCookiesCached`) — the post-drain pending-dispatch
The deferred cross-context event. A cookie-cached notify-mux + a pending queue drained *after* the frame drain (when HOST is free), dodging the isolate-borrow re-entrancy limit:
- `COOKIE_CACHED_MUX: RefCell<EventMux<v8::Global<v8::Function>>>` (single un-keyed list — name `""`, like `CHAT_MSG_SUBS`).
- `COOKIE_CACHED_PENDING: RefCell<Vec<i32>>` — slots awaiting fan-out.
- `__s2_cookie_on_cached(handler)` — subscribe (owner-tagged, ledgered via the existing event-sub ledger path).
- `__s2_cookie_dispatch_cached(slot)` — pushes `slot` to `COOKIE_CACHED_PENDING` (the plugin calls this after a load completes; it runs under the drain's borrow, so it only does a `Vec` push — no HOST access).
- `dispatch_pending_cookie_cached()` (pub) — drains `COOKIE_CACHED_PENDING` and fans each slot out to the mux subscribers, **mirroring `dispatch_client_event`** (snapshot-release, `try_borrow_mut`, per-sub `is_live` + context clone + HandleScope/ContextScope/TryCatch, call `handler(slot)`). Called from `ffi.rs::s2script_core_dispatch_game_frame` **immediately after `frame_async_drain()`** (HOST released → no re-entrancy). Teardown: `remove_by_owner` on unload; reset `MUX` + clear `PENDING` on shutdown.
- Module: `Cookies.onCached(handler)` → `__s2_cookie_on_cached(function(slot){ handler(globalThis.__s2pkg_clients.Clients.fromSlot(slot)); })`. The `.d.ts` types the handler `(client: Client) => void`.
- Plugin: after the load loop + `mark_cached`, call `__s2_cookie_dispatch_cached(client.slot)`.

## Skipped (YAGNI — user-confirmed)
Access-level enforcement (no surface without the menu); fast-reconnect pending-op tracking (a ~15ms race); SM's two-table dbId normalization (a storage optimization). Also still deferred: the **menu** (its own slice, needs the menu primitive) and the real-client auto-lifecycle live test (human-client).

## Boundary & safety
All core additions are engine-generic (cookie cache/queue/mux over SteamID + opaque strings — no game names). The natives are `set_native`'d (no `S2EngineOps` op / no shim change). `dispatch_pending_cookie_cached` runs with HOST free (post-drain), and the offline-write drain is the plugin's. Every native `catch_unwind`-wrapped. One sniper rebuild (core natives + the `ffi.rs` post-drain call + prelude).

## Testing
- **Core unit** (`cookies.rs`): `get` → `Option` (miss vs stored `""`); `set`/`load` carry `updated`, `get_time`; the offline-write queue push/take; `reset` clears the mux/pending too.
- **In-isolate:** the `@s2script/cookies` module (`getTime`, empty-string round-trip, `setAuthId` cache write + queue); `onCached` — subscribe, `dispatch_cached(slot)` + a manual `dispatch_pending_cookie_cached()` fires the subscriber with the slot.
- **Live gate (bots-provable):** extend `clientprefs-demo` — set a cookie to `""` and read it back as `""` (not the default); `setAuthId` for a fake offline SteamID persists across restart (the plugin's offline drain writes it); log a `getTime`. `onCached`'s real fire needs a human client (the load path) — a deferred human-client test; the in-isolate test covers the mechanism.
- **Gates:** core-boundary, typecheck (`@s2script/cookies` + both plugins), full `cargo test`.
