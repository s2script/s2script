# `hook` is game events only; lifecycle is named publics — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/hook-events-a8c9`.
**Date:** 2026-08-30.
**Scope:** `hook` is the game-event catalog (`hook.on` / `hook.onPre`). Engine/plugin lifecycle is `export function OnMapEnd` (and the rest of the SM-named publics). `hook.client` / `hook.entity` / `hook.server` / `hook.events` are deleted. 0.x **minor**.

## Why

`hook.client.onFullyConnected` duplicated `export function OnClientPostAdminCheck`. SourceMod’s split is the one we want: named publics for engine callbacks, `HookEvent` for the game-event catalog. `hook` is that catalog.

## Shape

```ts
import { command, hook, topmenu, HookResult } from "@s2script/sdk";
import type { Client, DamageInfo } from "@s2script/sdk";

export function OnPluginStart(): void {
  command("sm_x", handler);
  hook.on("player_spawn", (ev) => { /* post */ });
  hook.onPre("player_changename", (ev) => {
    return HookResult.Handled; // suppress client broadcast
  });
  topmenu.addCategory("Server Commands");
  onOutput("trigger_multiple", "OnStartTouch", (ev) => { /* filtered I/O */ });
}

export function OnPluginEnd(): void {}
export function OnMapStart(map: string): void {}
export function OnMapEnd(): void {}
export function OnGameFrame(): void {}
export function OnGameFramePost(): void {} // phase "post" (HUD paint)
export function OnClientPostAdminCheck(client: Client): void {}
export function OnTakeDamage(info: DamageInfo): void { info.damage /= 2; }
```

`hook.on` → `ctx.events.on`. `hook.onPre` → `ctx.events.onPre`. The `GameEvent` is only valid synchronously.

## Named publics the host subscribes

Already wired: `OnPluginStart` / `OnPluginEnd` / `OnPluginState`, `OnGameFrame`, `OnMapStart` / `OnMapEnd` / `OnConfigsExecuted`, `OnAllPluginsLoaded`, `OnClientConnected` / `PutInServer` / `Active` / `PostAdminCheck` / `Disconnect`, `OnClientSayCommand`.

This slice adds:

| Export | Wires to |
|--------|----------|
| `OnClientSettingsChanged(client)` | `ctx.clients.onSettingsChanged` |
| `OnClientVoice(client)` | `ctx.clients.onVoice` |
| `OnClientCookiesCached(client)` | `ctx.clients.onCookiesCached` |
| `OnPlayerRunCmd(cmd, info)` | `ctx.clients.onRunCmd` |
| `OnEntityCreated(entity, className)` | `ctx.entities.onCreate("*", …)` |
| `OnEntitySpawned(entity, className)` | `ctx.entities.onSpawn("*", …)` |
| `OnEntityDestroyed(entity, className)` | `ctx.entities.onDelete("*", …)` |
| `OnTakeDamage(info)` | `ctx.entities.onDamage` |
| `OnPrecache(pc)` | `ctx.server.onPrecache` |
| `OnGameFramePost()` | `ctx.server.onGameFrame(fn, { phase: "post" })` |

Missing export = not subscribed. Class-name filters for create/spawn/delete happen in the handler (SM `OnEntityCreated`).

## `onOutput` stays a free function

Entity I/O is keyed by `(classname, output)` at subscribe time (native mux). A single `OnEntityOutput` public would have to subscribe `"*" / "*"` and that is too hot for `zones`. Load-window `onOutput(classname, output, handler)` is SourceMod `HookEntityOutput`.

## Cookbook

One plugin module can export each public once. Recipes grow optional callbacks (`onGameFrame`, `onTakeDamage`, …); `cookbook/src/plugin.ts` fans them out from the named publics.

## Out of scope

- Typed catalog event-name generics
- Engine SUPERCEDE-on-Continue
- Per-entity SDKHook handles
- Frame `priority` on `OnGameFramePost` (default `"normal"`)
