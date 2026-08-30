# SM lifecycle publics + hook.event — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/basecommands-dogfood-a8c9`.
**Date:** 2026-08-30.
**Scope:** the host finds more SourceMod-named exports; `hook` grows event/output/topmenu; `createScope` and `Command` alias land. `plugin()` stays valid. No barrel in this slice. `Player` stays `@s2script/cs2`.

---

## Publics the host subscribes

`__s2_run_factory` already subscribes `OnGameFrame` / `OnMapStart` and calls `OnPluginStart` / wraps `OnPluginEnd`. This slice adds:

| Export | Wires to | Notes |
|--------|----------|--------|
| `OnClientConnected(client)` | `ctx.clients.onConnect` | SM name |
| `OnClientPutInServer(client)` | `ctx.clients.onPutInServer` | |
| `OnClientActive(client)` | `ctx.clients.onActive` | CS2 signon; steamId is reliable here |
| `OnClientPostAdminCheck(client)` | `ctx.clients.onFullyConnect` | closest analog: Steam ticket validated; admin cache is host-global |
| `OnClientDisconnect(client)` | `ctx.clients.onDisconnect` | |
| `OnClientSayCommand(slot, text, teamonly)` | `ctx.clients.onSay` | return `HookResultValue`; `Handled`/`Stop` suppress broadcast |
| `OnMapEnd()` | see map wrapper | |
| `OnConfigsExecuted()` | see map wrapper | config is already materialized at load |
| `OnAllPluginsLoaded()` | after Active, when no Loading/Waiting plugins remain | load window is **sealed**; `tryUse` stays load-window — this public means “the set is stable” |

Missing export = the host does not subscribe (same as `OnGameFrame` today).

### Map wrapper

ISource2Server on CS2 has no `LevelShutdown`. OnMapEnd is derived from a **subsequent** `StartupServer` (the existing `Server.onMapStart` mux):

1. First map start for this plugin: `OnConfigsExecuted` then `OnMapStart` (no `OnMapEnd`).
2. Later map starts: `OnMapEnd`, then `OnConfigsExecuted`, then `OnMapStart`.

Server-shutdown `OnMapEnd` is deferred (needs a real engine shutdown fact). Old-map entities are already gone at StartupServer POST — `OnMapEnd` is for resetting plugin state, not walking the previous world’s refs.

A single `ctx.server.onMapStart` subscription drives all three exports so `OnMapStart` does not double-fire.

### OnAllPluginsLoaded

After `finalize_loading_plugins` activates settled loads and `start_unblocked_waiters`, if `LOADING` is empty and the loader has no WAITING plugins, each Active plugin runs its pending `OnAllPluginsLoaded` once (cleared after fire). A late `sm plugins load` fires it immediately when that plugin becomes Active and the set is quiet.

`tryUse` is still load-window-only. Optional deps are `tryUse` in `OnPluginStart` (may be null) or producer-as-import.

---

## `hook` expansion

Load-window, same `__s2_load_ctx` throw as `hook.damage`:

- `hook.event(name, handler)` — post (`ctx.events.on`)
- `hook.event(name, handler, "pre")` — `ctx.events.onPre`
- `hook.output(classname, output, handler)` — `ctx.entities.onOutput`
- `hook.topmenu` — same object as the existing `topmenu` export (kept for compat)

## Other

- `createScope()` — load-window free function, same as `ctx.createScope`
- `export type Command = CommandInvocation` on `@s2script/sdk/commands`
- `config.onChange` added to `@s2script/sdk/config` types (runtime already had it)

## Explicit non-goals

- Root barrel `from "@s2script/sdk"` (next slice; was previously rejected at `s2require`)
- Putting `Player` on the SDK (game package)
- Engine SUPERCEDE-on-Continue
- Rewriting remaining plugins (follows once this surface exists)
- Shim `LevelShutdown` / shutdown `OnMapEnd`
