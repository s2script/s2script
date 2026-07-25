---
'@s2script/sdk': minor
---

Add `Server.onCvarChange` — SourceMod `HookConVarChange`.

Watch one cvar by name, or `"*"` for every cvar; the handler receives the name plus the new and old
values as strings. Returns `{ dispose() }`, and subscriptions are ledgered so unload drops them
regardless.

Notify-only, and the API says so: the engine's global change callback runs *after* the value has
been applied, so a handler cannot veto a change. A handler that throws is logged and contained; the
remaining handlers still run. The engine only calls back on a real change, so a write of the same
value does not fire and plugins need not de-duplicate.
