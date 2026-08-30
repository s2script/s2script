---
"@s2script/sdk": minor
"@s2script/cs2": minor
"@s2script/eslint-plugin": patch
---

`plugin((ctx) => …)` is no longer a public authoring API. Plugins export `OnPluginStart` (plus named publics). Load-window `hook.*` / `previous()` / `pluginId()` / `command.onClientCommand` cover the remaining ctx-only gaps. CS2 `ui`, `gameRules`, `players`, and `items` are free load-window exports. 0.x minor bump.
