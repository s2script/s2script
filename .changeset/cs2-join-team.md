---
"@s2script/cs2": minor
---

`Teams.onJoinTeam(handler)` in `@s2script/cs2` decides a player's team request (jointeam and the team
menu, CCSPlayerController::HandleCommandJoinTeam). Return `HookResult.Handled` or `Stop` to refuse it.
