---
"@s2script/cs2": patch
---

menuhud: clear a slot's menu when its player leaves or re-activates

A disconnect is not a close, so a player who left mid-menu never reached
`renderer.close`. The session, the saved `moveType`, the cursor grab and the Tab
arm all stayed on the slot, and the next occupant — normally the same person
reconnecting — arrived frozen, input-captured, and waiting to click a sheet
nobody had drawn for them.

`Clients.onDisconnect` and `Clients.onActive` now both tear the slot down.
Activate is the backstop, because a timeout or a crash does not always deliver
the disconnect. The saved `moveType` is dropped rather than re-applied: the
replacement pawn is fresh and already has the right one.
