---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Timer-grade engine surface for movement plugins (surf/bhop timers, replay recorders):

- `@s2script/sdk/files`: sandboxed per-plugin file IO (`read`/`readText`/`write`/`remove`/`list`)
  that runs on the async runtime's worker threads, with optional off-thread gzip and atomic
  replacing writes. The game thread only copies the payload.
- `Server.tickCount` and `Server.tickInterval`: the integer simulation tick and its length,
  derived from the engine clock.
- `EntityRef.gather(plan, out)` + `GatherKind`: one native call fills a preallocated
  `Float64Array` with any set of fields and pointer-chained fields, for per-tick hot paths.
- `Pawn.readKinematics(out)` (`@s2script/cs2`): origin, eye angles, velocity, flags, move type
  duck amount and buttons in one allocation-free call.
- `Player.observerTarget` / `Player.observerMode` (`@s2script/cs2`): who a spectator is watching,
  for spectator-aware HUDs.
- `sceneNodeOf(ref)` (`@s2script/cs2`): the world transform of any entity (e.g. a teleport
  destination's rotation) through the generated `SceneNode` wrapper.
