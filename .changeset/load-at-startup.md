---
"@s2script/sdk": patch
---

Plugins load while the server starts, before the first map, instead of on the first server frame.
An empty server hibernates and runs no frames, so plugins used to wait for the first player.
