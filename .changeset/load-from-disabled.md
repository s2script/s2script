---
"@s2script/sdk": minor
---

`Plugins.load("disabled/<plugin id>")`, behind `sm plugins load disabled/@edgegamers/jailbreak`,
loads a plugin kept in `plugins/disabled/` for the current run only. Only the `disabled/` prefix
reaches into that folder, and the id is mapped to its file for you. The plugin stays disabled on
disk and in `plugins.json`, so a restart or `unload` turns it off again. `PluginInfo.fromDisabled`
marks such a plugin in `Plugins.list()`.
