---
"@s2script/sdk": minor
---

`hook` is game events only (`hook.on` / `hook.onPre`). Lifecycle is named publics (`OnMapEnd`, `OnTakeDamage`, `OnGameFramePost`, …). `hook.client` / `hook.entity` / `hook.server` are removed. Filtered entity I/O is the free `onOutput`. 0.x minor bump.
