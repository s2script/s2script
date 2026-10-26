---
"@s2script/cs2": minor
---

Movement hooks. `movement.onProcessMove(handler)` and `movement.onProcessMovePost(handler)` in `@s2script/cs2`
run on every player's movement tick (CCSPlayer_MovementServices::ProcessMove). The view has the player,
view angles, forward and side input, origin and velocity; assigning `velocity` before the move changes
how the engine moves the player that tick.
