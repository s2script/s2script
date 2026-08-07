---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Add five `EntityRef` property setters and `Pawn.maxSpeed`

Each of these wraps an engine function that has **no working schema equivalent**, which is the
reason they are engine calls rather than field writes:

- `EntityRef.setGravityScale(scale)` — `CBaseEntity::SetGravityScale`. The setter early-returns when
  the value is unchanged and maintains a second field (`m_flActualGravityScale`), so a plugin that
  writes `m_flGravityScale` directly sees nothing happen. That trap is the whole reason this exists.
- `EntityRef.applyAbsVelocityImpulse([x,y,z])` — `CBaseEntity::ApplyAbsVelocityImpulse`. Additive and
  physics-aware, for knockback and boosts. `teleport(null, null, velocity)` sets velocity absolutely;
  a raw `m_vecAbsVelocity` write skips the partition/physics update entirely.
- `EntityRef.stopSound(name)` — `CBaseEntity::StopSound`, the counterpart to `Sound.emit`.
- `EntityRef.setBodyGroupByName(name, group)` — `CBaseModelEntity::SetBodyGroupByName`.
  `m_bodyGroupChoices` is a `CUtlOrderedMap`, not a writable scalar.
- `EntityRef.setModelScale(scale)` — `CBaseModelEntity::SetModelScale`.
- `Pawn.maxSpeed` — `CCSPlayerPawn::GetPlayerMaxSpeed`. Computed by the engine; there is no
  `m_flMaxSpeed` on `CCSPlayerPawn` to read. `null` (never `0`) when unavailable, because `0` is a
  legitimate speed for a frozen player.

The five `EntityRef` methods are engine-generic (`CBaseEntity` / `CBaseModelEntity`) and so are core
native ops. `Pawn.maxSpeed` names a CS2 class, so it ships as a `calls` descriptor in
`gamedata/cs2/game.cs2.jsonc` and is consumed from `games/cs2/js/pawn.js` — no CS2 identifier enters
core, as `check-core-names.sh` verifies.

Every signature was located by an independent per-build derivation and then **re-resolved against our
own pinned `libserver.so` per `docs/re-strategy.md` Rule 3** — a borrowed pattern is a hint, never a
number. For each: the pattern matches exactly once in the PF_X segment, and the match address is
preceded by `int3` padding or a `ret`, confirming it is a real function entry (which is what makes
`resolve: "direct"` safe). Every prototype was then confirmed by disassembly at that address rather
than trusted from the deriver's declaration — this caught that `SetBodyGroupByName`'s group argument
is 32-bit, and that `ApplyAbsVelocityImpulse` takes its `Vector` by address.

`setModelScale` is recorded as **lower confidence than the other four**: its argument shape is
confirmed, but its body is a devirtualisation guard that hops to a sub-object and tail-calls, so the
name is a catalogue attribution the body does not itself prove. It is memory-safe to call; verify the
effect before relying on it in a shipped plugin. The gamedata comment says so too.

All six degrade per-descriptor in the usual way: an unresolved signature leaves the op null and the
accessor returns `false` (or `null` for `maxSpeed`), never a crash.
