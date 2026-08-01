---
"@s2script/sdk": patch
---

Fix `s2s build`'s plugin-gamedata naming-mismatch error message. It used to say a misnamed
`s2script.gamedata` file breaks "the same rule that names the framework's own
`gamedata/core/*.jsonc` files" — but those files are named for their **target**
(`common.gamedata.jsonc`, `engine.source2.jsonc`, `game.cs2.jsonc`), not their owner, so the
analogy no longer matches since gamedata split into an owner/target tree. The message now explains
the actual rule directly: a plugin's single gamedata file is named for its owner (the plugin
itself), the same way `gamedata/core/` and `gamedata/cs2/` are owner-named *directories* in the
framework's own tree.
