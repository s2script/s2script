# Nest `hook` by subject — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/drop-plugin-factory-a8c9`.
**Date:** 2026-08-30.
**Scope:** `hook` is `hook.<subject>.on…` instead of a flat bag. 0.x **minor**. No production users; flat aliases are deleted, not kept.

## Why

`hook.fullyConnect` / `hook.create` / `hook.gameFrame` dump every subscription onto one object. The subject (`client` / `entity` / `server`) is the extra dot.

## Authoring shape

```ts
import { hook } from "@s2script/sdk";

export function OnPluginStart(): void {
  hook.client.onFullyConnected((c) => { /* Steam ticket validated */ });
  hook.entity.onDamage((info) => { info.damage /= 2; });
  hook.server.onGameFrame(() => { /* HUD paint */ }, { phase: "post" });
  hook.event("player_spawn", (ev) => { /* catalog event, name is the subject */ });
}
```

Named publics (`OnClientConnected`, `OnClientPostAdminCheck`, `OnGameFrame`, …) stay the SourceMod-shaped path for a single plugin module.

## Source 2 client lifecycle (still valid)

Engine mux names in `core/src/client.rs` / `ffi.rs`: `connect`, `putinserver`, `active`, `fullyconnect`, `disconnect`, `settingschanged`. Plus `onVoice`, `onCookiesCached`, `onSay`, `onRunCmd` already wired on `ctx.clients`.

| `hook.client` | Wires to | SM public analog |
|---------------|----------|------------------|
| `onConnect` | `clients.onConnect` | `OnClientConnected` |
| `onPutInServer` | `clients.onPutInServer` | `OnClientPutInServer` |
| `onActive` | `clients.onActive` | `OnClientActive` |
| `onFullyConnected` | `clients.onFullyConnect` | `OnClientPostAdminCheck` (Steam ticket validated) |
| `onDisconnect` | `clients.onDisconnect` | `OnClientDisconnect` |
| `onSettingsChanged` | `clients.onSettingsChanged` | — |
| `onVoice` | `clients.onVoice` | — |
| `onCookiesCached` | `clients.onCookiesCached` | — |
| `onSay` | `clients.onSay` | `OnClientSayCommand` |
| `onRunCmd` | `clients.onRunCmd` | `OnPlayerRunCmd` |

`onFullyConnected` is the public name; the engine fact stays `fullyconnect`.

## Other subjects

| `hook.entity` | Was |
|---------------|-----|
| `onCreate` / `onSpawn` / `onDelete` | `hook.create` / `spawn` / `delete` |
| `onOutput` | `hook.output` |
| `onDamage` | `hook.damage` |

| `hook.server` | Was |
|---------------|-----|
| `onGameFrame` | `hook.gameFrame` |
| `onMapStart` | `hook.mapStart` |
| `onPrecache` | `hook.precache` |

**Stay top-level:** `hook.event(name, handler, phase?)` (the game-event catalog; the name is the subject), `hook.topmenu`.

**Deleted:** every flat `hook.create` / `hook.gameFrame` / `hook.fullyConnect` / … alias.

Load-window throw copy names the nested path (`hook.client.onFullyConnected()`).

## Out of scope

- Renaming named publics (`OnClientPostAdminCheck` stays)
- Renaming `ctx.clients.onFullyConnect` (internal load ctx)
- `hook.events.on` restyle of `hook.event`
- Engine SUPERCEDE-on-Continue
- Live CS2 re-run (authoring wrapper only)
