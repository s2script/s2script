# clientprefs (Cookie Storage/API Layer) — Design

**Status:** Approved (brainstorm), ready for implementation plan.
**Slice:** the SourceMod clientprefs cookie system — the DB primitive's first real consumer.

## Goal

Give plugins SourceMod-parity **client preference cookies** — persistent per-player key→value strings, keyed by SteamID, loaded on connect and saved on disconnect — through an engine-generic `@s2script/clientprefs` API, backed by the SQLite DB primitive. This is the cookie storage/API layer; the `sm_settings`/`sm_cookies` menu is deferred (it needs the still-deferred menu primitive).

## Motivation & context

SourceMod's clientprefs is two pieces: a native **extension** that owns the cookie system (a per-client in-memory cache + an async DB for load/save + the natives `RegClientCookie`/`SetClientCookie`/`GetClientCookie`/…), and a separate SP **plugin** (`clientprefs.sp`) that is *only* the settings menu. The extension's natives are native+in-process, so every plugin reads/sets cookies synchronously with no cross-plugin hop.

This slice mirrors that split, and is the first consumer of the just-built SQLite primitive (`@s2script/db`) plus `@s2script/clients` (lifecycle) — validating both as real building blocks (the charter's "the base-plugin suite is the std lib's acceptance test").

## Scope

**In scope:** the cookie cache (core host-global + natives), the `@s2script/clientprefs` module (register/get/set/areCached/onCached), and the `clientprefs` base plugin (the DB load/save lifecycle + the `cookies` table).

**Deferred (named follow-ons, NOT built here):**
- The whole **menu** surface — `SetCookiePrefabMenu`/`SetCookieMenuItem`/`ShowCookieMenu`/the cookie iterator + the `sm_settings`/`sm_cookies` commands (needs the menu primitive; SM keeps this in a separate plugin too).
- `SetAuthIdCookie` (offline / by-SteamID writes for a not-connected player).
- `GetClientCookieTime`; access-level **enforcement** (`CookieAccess` is stored now, enforced with the menu).
- Dirty-op pending tracking for the fast-reconnect race (see Edge cases).
- SM's two-table `sm_cookies`/`sm_cookie_cache` dbId normalization (we use one table keyed by `(steamid, name)`).

## Architecture (the SM-validated hybrid)

The cookie *system* splits across three layers; one-way dependencies (game → core):

1. **Core — a host-global cookie cache + natives (no engine op; mirrors the admin/ban caches → no shim change).** The globally-visible part: a `steamid → { name → { value, dirty } }` map, plus natives `set_native`'d into every plugin context. Reads/sets are synchronous **in-context native calls** — never a cross-plugin hop. The cache logic lives in a pure, unit-testable `core/src/cookies.rs` (like `db.rs`); the natives wrap it.
2. **`@s2script/clientprefs` (engine-generic TS module) — the SM-parity API** (`Cookies.register`/`get`/`set`/`areCached`/`onCached`) over those natives. Any plugin imports it directly.
3. **The `clientprefs` base plugin (CS2-layer plugin) — the DB lifecycle in JS.** Hooks `@s2script/clients` events to load a client's cookies from `@s2script/db` into the core cache on connect and flush the dirty ones back on disconnect. This is where SM does it in native C++ — we do it in JS because the DB + client events are already JS primitives. It owns the `cookies` table.

**Boundary:** the cookie cache + natives + `@s2script/clientprefs` are engine-generic (SteamID + opaque string cookies — Source2-generic), so core + a module; the `clientprefs` *plugin* is a base plugin. Both gates stay green.

## Core: the cookie cache + natives

`core/src/cookies.rs` — a thread-local host-global `HashMap<String /*steamid*/, ClientCookies>` where `ClientCookies = { cached: bool, entries: HashMap<String /*name*/, Entry> }` and `Entry = { value: String, dirty: bool }`. Pure functions (no V8) + unit tests. The natives (registered via `set_native`, no `S2EngineOps` op):
- `__s2_cookie_get(steamid, name) → value | ""` — cache read (`""` on miss).
- `__s2_cookie_set(steamid, name, value)` — write + mark `dirty = true` (the API path).
- `__s2_cookie_load(steamid, name, value)` — write **without** marking dirty (the DB-load path — a loaded value is not a change).
- `__s2_cookie_get_dirty(steamid) → JSON "{name: value}"` — the dirty subset, for the disconnect flush (JSON string; the config bridge uses the same native-returns-JSON pattern).
- `__s2_cookie_clear(steamid)` — drop a client's entries (on disconnect, after flush).
- `__s2_cookie_mark_cached(steamid)` / `__s2_cookie_is_cached(steamid) → bool` — the `AreClientCookiesCached` state (a client with zero cookies is still "cached").

No shim change (the cache is core-internal); one sniper rebuild for the core natives.

**`onCached` (`OnClientCookiesCached`) is DEFERRED** — it is a *cross-context* event (the plugin loads in its context; a consumer's handler is in another), but the load completes inside a `.then` that runs under the isolate borrow (the frame-drain microtask checkpoint), so a JS-triggered fan-out hits the `try_borrow_mut` re-entrancy skip (the `Events.fire`-can't-re-dispatch limitation). Firing it reliably needs a post-drain pending-dispatch (queue the slot during the drain, fan out to subscribers after HOST is free) — a scoped follow-up. This slice ships `areCached`; a consumer that needs the exact "ready" moment is the forcing function for building `onCached` then.

## `@s2script/clientprefs` — the API

```ts
export enum CookieAccess { Public, Protected, Private }   // stored now; enforced with the menu
export interface Cookie { readonly name: string; readonly access: CookieAccess; readonly default: string; }
export interface CookieOptions { description?: string; access?: CookieAccess; default?: string; }

export declare const Cookies: {
  /** Register (or find) a cookie definition. */
  register(name: string, opts?: CookieOptions): Cookie;
  /** Cache value for this client, else the cookie's default, else "". */
  get(client: Client, cookie: Cookie): string;
  /** Write the cache + mark dirty (flushed to the DB on disconnect). No-op for bots. */
  set(client: Client, cookie: Cookie, value: string): void;
  /** Has this client's cookies finished loading from the DB? */
  areCached(client: Client): boolean;
  // onCached (OnClientCookiesCached) is DEFERRED — see the core section. Gate on areCached for now.
};
```

- `client` is a `Client` (`@s2script/clients`); the module resolves `client.steamId`. **Bots** (`steamId === "0"`) are skipped by `get`/`set` (return `default`/no-op).
- `get`/`set` are synchronous in-context native calls against the core cache.
- `register` records the definition (name/default/access) in a lightweight per-context map (idempotent — a repeat `register(name)` returns the same `Cookie`) and returns a `Cookie`. `CookieAccess` is carried for the future menu.

## The `clientprefs` base plugin — the DB lifecycle

Uses `@s2script/db` + `@s2script/clients`; owns the table. On first load: `db.execute("CREATE TABLE IF NOT EXISTS cookies (steamid TEXT, name TEXT, value TEXT, updated INTEGER, PRIMARY KEY (steamid, name))")`.

- **Load** on `Clients.onPutInServer(client)` (SteamID valid + client in server — SM's `OnClientAuthorized` analog); skip bots:
  `db.query("SELECT name, value FROM cookies WHERE steamid = ?", [client.steamId])` → for each row `__s2_cookie_load(steamId, name, value)` → `__s2_cookie_mark_cached(steamId)`. (`onCached` fan-out deferred — see the core section.)
- **Save** on `Clients.onDisconnect(client)`; skip bots:
  read `__s2_cookie_get_dirty(steamId)` (sync), `__s2_cookie_clear(steamId)` (sync), then for each dirty `(name, value)` `db.execute("INSERT OR REPLACE INTO cookies (steamid, name, value, updated) VALUES (?,?,?,?)", [steamId, name, value, nowSeconds])` — the writes use the captured values, so the clear can't race them.

## Edge cases (all degrade, never crash)

- **Read before load** → cache miss → the cookie's `default`. `areCached` false until loaded; consumers gate on `onCached`/`areCached`.
- **Disconnect flush** = capture-then-clear (dirty read + clear are synchronous; the async upserts use captured values). Known minor edge: a very fast same-SteamID reconnect inside the ~1-frame async-save window could load slightly-stale data — pending-op tracking is a deferred follow-up.
- **DB errors** — a failed load leaves the client uncached (gets return defaults); a failed save logs + drops the change. The plugin try/catches every DB op.
- **Bots** (`steamId === "0"`) skip the whole path.
- **clientprefs unload/reload** — the core cache (host-global) survives (like admin/ban); the lifecycle stops until reload.

## Testing & live gate

- **Core Rust unit tests** (`cookies.rs`, pure — no engine): `set` marks dirty, `load` doesn't, `get_dirty` returns only the dirty subset, `clear` empties, `mark_cached`/`is_cached` track (incl. a zero-cookie client reads "cached").
- **In-isolate** — `@s2script/clientprefs` `register`/`get` (default fallback) / `set` / `areCached` over the natives (a plugin drives the module).
- **Live gate (bots-provable via a synthetic SteamID):** a demo drives the full stack for a fake steamid — `set` a cookie → flush to the DB (invoke the plugin's save path / a `db.execute`) → **restart** → load from the DB → `get` → the value survived. Proves core cache + module + the DB load/save round-trip without a real client (bots have no cookies).
- **Deferred human-client test:** a real client connects → a cookie is set → reconnect → the cookie persists (the auto `onPutInServer`-load / `onDisconnect`-save lifecycle with a real SteamID) — same ceiling as the ban-reason / SayText2 human-client tests.
- **Gates:** core-boundary (`cookies.rs` + the module engine-generic — no CS2 names), typecheck (`@s2script/clientprefs` + the plugin).

## Deferred follow-ons summary

`onCached` (`OnClientCookiesCached` — needs the post-drain pending-dispatch mechanism); the menu surface (+ `sm_settings`/`sm_cookies`, needs the menu primitive); `SetAuthIdCookie`; `GetClientCookieTime`; access-level enforcement; fast-reconnect pending-op tracking; SM's two-table dbId normalization.
