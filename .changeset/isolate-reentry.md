---
"@s2script/sdk": patch
"@s2script/cs2": patch
---

A plugin engine call is visible to other plugins before it returns. `Events.fire` nests `on`/`onPre`. `Player.respawn()` and `GameRules.terminateRound()` run on this call (no next-frame queue). `Server.setCvar` writes through ICvar now (boolean return; `getCvar`/`onCvarChange` see the new value on the same call).
