# @s2script/entity

## 0.2.0

### Minor Changes

- 4e69d7d: Runtime engine trigger zones. `@s2script/zones` now builds each zone as a real `trigger_multiple` entity with an arbitrary-box collision (any size, any aspect) and fires enter/leave off the engine's own touch system — replacing the previous ~8Hz origin-polling backend with engine-accurate detection that also sees non-player entities.

  New APIs powering it:

  - `@s2script/entity` — `EntityRef.setModel(name)` (build/register an entity's collision aggregate), `EntityRef.activateCollision()` (register + reshape the collision to the entity's bounds via `SetCollisionBounds` + `SetSolid(SOLID_BBOX)`), and `EntityRef.writeInt32Via(pathOffs, finalOff, value)` (write an int32 at the end of a pointer chain).
  - `@s2script/cs2` — `TriggerZone.create(min, max, opts?)` → a runtime box trigger whose `OnStartTouch`/`OnEndTouch` you hook via `Entity.onOutput`. Non-solid (pass-through), works on any map.

## 0.1.1

### Patch Changes

- 5fcc41f: Initial public npm release of the `@s2script/*` types packages and CLI (Changesets pipeline).
- Updated dependencies [5fcc41f]
  - @s2script/events@0.1.1
