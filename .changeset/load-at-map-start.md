---
"@s2script/sdk": patch
---

Plugins load when the first map starts, as in SourceMod, instead of before it. CS2 has registered its
game events, user messages and entities by then, so a plugin's `OnPluginStart` sees a ready server.
An empty server still loads them without waiting for a player.
