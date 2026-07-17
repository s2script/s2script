---
"@s2script/cs2": minor
---

Round control: GameRules.terminateRound(reason, delay?) (sig-resolved CCSGameRules::TerminateRound,
deferred one frame so round_end reaches every plugin), round-clock write surface
(setRoundTime/setTimeRemaining/addTimeRemaining + roundStartTime/timeElapsed/timeRemaining reads),
Teams score API (cs_team_manager CTeam.m_iScore), and the RoundEndReason/WinPanelFinalEvent const maps.
