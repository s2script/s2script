---
"@s2script/sdk": minor
---

`Plugins.load(id)` (and `sm plugins load <id|file>`) can now load a plugin kept in
`plugins/disabled/` for the current run only: it stays disabled on disk and in `plugins.json`, so a
restart or `unload` turns it off again. `PluginInfo.fromDisabled` marks such a plugin in
`Plugins.list()`.
