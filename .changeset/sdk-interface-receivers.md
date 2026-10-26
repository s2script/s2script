---
"@s2script/sdk": minor
---

Declared calls can target engine interfaces. A `calls` entry in plugin gamedata may now use
`"receiver": { "kind": "interface", "name": "<interface version string>" }`, which calls the
function on that engine interface (found through the engine and server factories) instead of on an
entity. The generated callable takes no `self`. A new return kind, `utlstringlist`, copies an
engine `CUtlStringList` out as `string[]` (or null).
