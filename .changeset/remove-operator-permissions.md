---
"@s2script/sdk": minor
---

Plugins no longer need an operator allow-list entry to use engine calls or hooks: the runtime's
`configs/permissions.json` is gone. `s2s build` now records `engine:calls` / `engine:hooks` in the
manifest from the plugin's gamedata on its own, so `s2script.permissions` no longer has to be
written in `package.json` (an authored value is still accepted).
