---
"@s2script/sdk": patch
---

A plugin engine call is visible to other plugins before it returns. `Events.fire` nests `on`/`onPre`. `Server.setCvar` writes through ICvar now (boolean return; `getCvar`/`onCvarChange` see the new value on the same call).
