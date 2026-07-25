---
'@s2script/sdk': minor
---

Add `Client.command` and `Client.fakeCommand` — SourceMod `ClientCommand` / `FakeClientCommand`.

`command(cmd)` asks the client to run it in their own console; `fakeCommand(cmd)` has the SERVER
execute it attributed to that player, so it works on bots. Both return `false` rather than silently
no-opping when the slot is bad, the text is empty, or the engine interface is unavailable.

`fakeCommand` dispatches through `ICvar::DispatchConCommand` with a `CCommandContext` carrying the
slot — verified live by attribution, where a faked `say` prints as that player rather than Console.
Engine commands execute; a command registered by an s2script *plugin* is dispatched but its JS
handler does not run, because the core holds the isolate borrow across all JS and the nested
dispatch hits the documented re-entrancy skip. Use a cross-plugin interface to drive another
plugin's behaviour.
