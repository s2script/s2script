---
"@s2script/cs2": minor
---

`Player.switchTeam(team)` — non-lethal T/CT team switch (the player stays alive and keeps weapons; the
pawn may be respawned) via the self-resolved `CCSPlayerController::SwitchTeam`. None/Spectator
dispatches to ChangeTeam (CSSharp/SwiftlyS2 parity). Serial-gated; degrades to a no-op when the
signature is unresolved. Closes the TTT-port "role→team without killing the player" gap.
