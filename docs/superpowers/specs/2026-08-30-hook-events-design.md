# `hook.events.on` / `onPre`; drop `hook.topmenu` — design spec

**Status:** per-slice design. Lands as a GitHub-native stacked PR on `cursor/hook-subjects-a8c9`.
**Date:** 2026-08-30.
**Scope:** game-event subscriptions match the subject style (`hook.events.on` / `hook.events.onPre`). `hook.topmenu` is removed; use the free `topmenu` export. 0.x **minor**. Flat `hook.event(name, handler, phase?)` is deleted.

## Shape

```ts
import { hook, topmenu, HookResult } from "@s2script/sdk";

export function OnPluginStart(): void {
  hook.events.on("player_spawn", (ev) => { /* post */ });
  hook.events.onPre("player_spawn", (ev) => {
    return HookResult.Handled; // suppress client broadcast
  });
  topmenu.addCategory("Server Commands");
}
```

`hook.events.on` → `ctx.events.on`. `hook.events.onPre` → `ctx.events.onPre`. The `GameEvent` is only valid synchronously.

`hook.server.onGameFrame(fn, { phase, priority })` stays an options object — frame phase is not event pre.

## Out of scope

- Typed catalog event-name generics (existing `string` names stay)
- Renaming named publics
- Engine SUPERCEDE-on-Continue
