# Base-plugin publics cutover — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/sdk-barrel-a8c9`.
**Date:** 2026-08-30.
**Scope:** rewrite `plugins/*` and `plugins/disabled/*` to `OnPluginStart` + `command`/`hook`/named publics. Behavior, translations, and live-gate copy stay identical. `plugin()` remains valid for examples that need `ctx.previous`.

## Authoring shape

```ts
import { command, hook, translations, ADMFLAG, … } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

export function OnPluginStart(): void {
  translations.load("…", "common");
  command.admin("sm_x", ADMFLAG.X, handler);
  hook.topmenu.addItem(…);
}

export function OnClientSayCommand(…): HookResult { … }
```

`Player` is never imported from the SDK barrel.

## Mapping

| Was | Becomes |
|-----|---------|
| `ctx.translations.load` | `translations.load` |
| `ctx.commands.registerAdmin` | `command.admin` |
| `ctx.commands.register` | `command` |
| `ctx.topmenu` | `hook.topmenu` |
| `ctx.entities.onDamage` | `hook.damage` |
| `ctx.entities.onOutput` | `hook.output` |
| `ctx.events.on` | `hook.event` |
| `ctx.createScope` | `createScope()` |
| `ctx.publish` | `publish()` |
| `ctx.clients.onConnect` | `OnClientConnected` |
| `ctx.clients.onPutInServer` | `OnClientPutInServer` |
| `ctx.clients.onActive` | `OnClientActive` |
| `ctx.clients.onDisconnect` | `OnClientDisconnect` |
| `ctx.clients.onSay` | `OnClientSayCommand` |
| `ctx.server.onGameFrame` | `OnGameFrame` |
| `ctx.server.onMapStart` | `OnMapStart` |
| factory `return { onUnload }` | `OnPluginEnd` |
| async factory `await Database.open` | `export async function OnPluginStart` + module `let db!` |

`config.onChange` stays a call from `OnPluginStart` (not load-window-gated on the `config` module).

## Explicit non-goals

- Changing live-gate behavior, phrase keys, or kick/ban copy
- Rewriting examples (next slice)
- Cookbook subpath imports (coverage corpus)
