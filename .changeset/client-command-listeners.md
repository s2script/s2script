---
"@s2script/sdk": minor
---

Add `Commands.onClientCommand` — the SourceMod `AddCommandListener` equivalent

`ctx.commands.register` CREATES a command, so it cannot be used to observe one the ENGINE already
owns: `RegisterConCommand` refuses the name ("unable to link multiple ConCommands named X") and the
handler never fires. That put every built-in client command — `player_ping`, `jointeam`, `drop`,
`buy` — out of reach of a plugin, and the workaround for at least one of them (reconstructing
`jointeam` from the resulting `player_team` event) is already in the tree.

`onClientCommand(name, handler)` fills that gap. The shim's `ClientCommand` hook already forwarded
every command name to the core, so nothing changed there; the core simply had nowhere to put a
listener that does not own the name.

Semantics deliberately INVERT a registered command, matching SourceMod: a registered ConCommand
supersedes (the engine never sees it), whereas a listener OBSERVES and passes through unless it
returns `>= HookResult.Handled`. Superseding by default would break the commands this exists to
watch — hooking `player_ping` would stop the ping marker ever being placed.

```ts
// Middle-mouse ping opens a menu, and still places the ping.
ctx.commands.onClientCommand("player_ping", (slot) => { openShop(slot); });
```

Listeners are owner-tracked and unsubscribe when their plugin unloads.
