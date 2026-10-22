---
"@s2script/sdk": minor
---

Engine functions (`gamedata/functions.jsonc`) accept a `ptr` parameter and return type, seen in
JavaScript as a `Ptr` or null, for calls and PRE/POST hooks. `Engine.fromSignature(name, declaration)`
and `Engine.fromVirtual(name, declaration)` declare an engine function inline: `s2s build` lifts the
literal declaration into the plugin's function bundle and manifest, exactly as if it were in
`functions.jsonc`, and a non-literal declaration fails the build.
