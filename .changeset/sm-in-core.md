---
"@s2script/sdk": patch
---

`sm` is now built into the runtime instead of basecommands: a server console command that works
with no plugin loaded. Players who type `sm` get the version and credits only; in-game admins use
`sm_rcon sm …`. A plugin can no longer register a command named `sm`.
