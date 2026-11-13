---
"@s2script/cs2": patch
---

`Teams.onJoinTeam` fires for every team request again. `CCSPlayerController::HandleCommandJoinTeam`'s
third argument is a bit field, not a bool; the game's own callers pass 2 and 3, which the hook used
to refuse ("function target N: noncanonical u8 closure argument"), so those requests skipped every
handler. The binding now takes it as an int: `view.queue` is bit 0, and the new `view.flags` is the
raw value.
