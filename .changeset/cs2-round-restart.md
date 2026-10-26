---
"@s2script/cs2": minor
---

Round restart hooks and intermission. `GameRules.onRoundRestart(handler)` and `GameRules.onRoundRestartPost(handler)`
run at the start of every round (CCSGameRules::RestartRound), before and after the engine resets it.
`GameRules.goToIntermission(abortedMatch?)` ends the match into the scoreboard and map vote.
