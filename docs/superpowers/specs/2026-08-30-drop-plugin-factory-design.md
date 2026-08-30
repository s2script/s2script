# Drop `plugin()` from the public authoring surface — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/plugins-publics-cutover-a8c9`.
**Date:** 2026-08-30.
**Scope:** `plugin((ctx) => …)` is no longer a supported authoring form. Public plugins write `export function OnPluginStart` plus load-window free APIs and named publics. 0.x **minor** bump (`@s2script/sdk`, `@s2script/cs2`, `@s2script/eslint-plugin` as needed). No production users; dual-API bloat is removed rather than deprecated.

## Authoring shape

```ts
import { command, hook, previous, translations, ADMFLAG } from "@s2script/sdk";
import { Player, ui } from "@s2script/cs2";

export function OnPluginStart(): void {
  const prev = previous() as { n: number } | undefined;
  translations.load("common");
  command.admin("sm_x", ADMFLAG.GENERIC, handler);
  hook.damage((info) => { info.damage /= 2; });
  ui.hud();
}

export function OnPluginState(): unknown { return { n: 1 }; }
export function OnPluginEnd(): void { /* best-effort */ }
```

`Player` / `ui` / `gameRules` / `players` / `items` stay on `@s2script/cs2`. `@s2script/sdk/unsafe` stays a subpath.

## What is removed from the public contract

- `plugin()`, `PluginFactory`, `PluginDefinition` — deleted from `packages/sdk/plugin.d.ts`.
- `s2s create` already scaffolds `OnPluginStart` only.
- In-repo plugins, examples, tools, SDK fixtures, and cookbook recipes stop importing `plugin`.

## What stays (internal / types)

- Prelude still builds a load-scoped ctx (`__s2_make_ctx`). Free APIs bind through `__s2_load_ctx`.
- Runtime still accepts a `{ __s2plugin: 1, factory }` default **for isolate tests** (`load_body`). That is not a typed authoring API.
- `PluginContext`, `PluginHooks`, `Scope`, and the `Ctx*` interfaces remain as types (Scope still uses them; CS2 still augments `PluginContext`).
- `no-ctx-escape` still locates a leftover `plugin()` factory if someone writes one against stubs.

## Free API fill (the ctx-only gaps)

Load-window, same throw as `command()`:

| Was | Becomes |
|-----|---------|
| `ctx.previous` | `previous(): unknown` |
| `ctx.id` | `pluginId(): string` |
| factory `return { state }` | `export function OnPluginState()` |
| factory `return { onUnload }` | `export function OnPluginEnd()` (already) |
| `ctx.entities.onCreate/onSpawn/onDelete` | `hook.create` / `hook.spawn` / `hook.delete` |
| `ctx.server.onPrecache` | `hook.precache` |
| `ctx.server.onGameFrame` (with phase/priority) | `hook.gameFrame(fn, opts?)` |
| `ctx.server.onMapStart` | `hook.mapStart` (public `OnMapStart` still exists) |
| `ctx.clients.onRunCmd` | `hook.runcmd` |
| `ctx.clients.onConnect` … `onVoice` / `onCookiesCached` / `onSay` | `hook.connect` / `putInServer` / `active` / `fullyConnect` / `disconnect` / `settingsChanged` / `voice` / `cookiesCached` / `say` |
| `ctx.commands.onClientCommand` | `command.onClientCommand` |
| `ctx.ui` / `ctx.gameRules` / `ctx.players` / `ctx.items` | `ui` / `gameRules` / `players` / `items` on `@s2script/cs2` (proxies onto the current load ctx) |

Named publics (`OnGameFrame`, `OnClientConnected`, …) remain the SourceMod-shaped path for a **single** plugin module. Cookbook recipes and live-gate tools register from `OnPluginStart`, so they use `hook.*` rather than re-exporting publics.

## Host

`__s2_run_factory`: after `OnPluginEnd` wrapping, if `exports.OnPluginState` is a function, set `hooks.state` to it (overrides a leftover factory `state()`).

Loader refuse copy for a missing artifact names `OnPluginStart` only. Legacy `onLoad` copy points at `OnPluginStart`, not `plugin()`.

## Cookbook

`Recipe.register()` takes no ctx. Recipes call `command` / `hook` / `tryUse` / `config.onChange`. Subpath imports stay (coverage corpus).

## Explicit non-goals

- Removing the internal ctx object or `load_body` plugin() wrapper used by isolate tests
- Putting `Player` on the SDK barrel
- Engine SUPERCEDE-on-Continue
- Changing live-gate behavior of tools (only the authoring wrapper)
- Major version bump
