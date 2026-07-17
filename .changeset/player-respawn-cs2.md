---
"@s2script/cs2": minor
---

Player.respawn(): respawn a dead player via the self-resolved CCSPlayerController::Respawn
(byte-sig + RTTI-vtable-membership load-validated; queued one frame outside the JS isolate borrow
so player_spawn reaches every plugin). Alive-guarded, serial-gated, degrades to false.
