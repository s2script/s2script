---
"@s2script/sdk": minor
---

Add `Server.commandEx(cmd)`, which runs a server console command and resolves with the console output it printed (SourceMod `ServerCommandEx`). `sm_rcon` in basecommands now uses it to send that output back to the admin who ran the command.
