---
"@s2script/sdk": minor
---

`clients.onRunCmd(handler, { phase: "post" })` and the `OnPlayerRunCmdPost(cmd, info)` export: a usercmd hook that runs once per executed command, after its movement was applied to the pawn. Read-only `cmd`, `info` = `{ slot, tick, commandNumber }`. `UserCmdView` gains read-only `pressedButtons`, `changedButtons` and `scrollButtons` from the pawn's movement services.
