---
"@s2script/cs2": minor
"@s2script/sdk": patch
---

`Player.target` resolves the rest of SourceMod's multi-target filters: `@alive`, `@dead`, `@bots`, `@humans`, `@!me`, `@aim`, and the CS2 team filters `@ct`, `@t` and `@spec`. `Activity.targetName` names a matched filter by its group ("all alive players", "all Counter-Terrorists") instead of a player count.
