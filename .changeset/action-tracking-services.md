---
"@s2script/cs2": minor
---

`player.actionTrackingServices` exposes the controller's action-tracking services: this round's
kill counters (`numRoundKills` / `numRoundKillsHeadshots`, the icons under each avatar in the top
bar) are writable, with `notifyChanged()` to re-network them, and `totalRoundDamageDealt` is
readable. Gamemodes that hide who killed whom (TTT) need the round-kill counter to stop the top bar
giving it away.
