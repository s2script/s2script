---
"@s2script/sdk": minor
---

`Plugins.load("<folder>/<plugin id>")`, behind `sm plugins load fun/@edgegamers/jailbreak`, loads a
plugin kept in any folder under `plugins/` (`disabled/`, `fun/`, nested ones) for the current run
only. Only a folder prefix reaches into a folder, the id is mapped to its file for you, and
nothing outside `plugins/` is reachable. The file stays where it is, so a restart or `unload`
(which takes the same form) turns it off again. `PluginInfo.folder` names the folder in
`Plugins.list()`.
