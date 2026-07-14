# @s2script/entity

## 0.3.0

### Minor Changes

- a3e5cc4: Add a CS2 `Weapon` entity object + player fire control.

  `@s2script/cs2` gains `Weapon` — an `EntityRef`-backed, serial-gated wrapper over `CCSWeaponBase` (`clip1`/`clip2`/`paintKit`/`owner`/`setAmmo`/`remove`, plus `Weapon.fromEntity`/`findAll`) — and new `Pawn` members: `activeWeapon` and `weapons` (now `Weapon`s), `giveNamedItem` (→ `Weapon`), `disarm`, and player fire control `blockFiring`/`allowFiring`/`nextAttack`.

  `@s2script/entity` gains `EntityRef.writeFloat32Via` and `writeBoolVia` — the write mirror of the `read*Via` pointer-chain accessors, over the `__s2_ent_ref_write_chain` core native.

- bb6b8fb: Entity lifecycle listeners: `Entity.onCreate` / `onSpawn` / `onDelete(className, handler)` fire when the
  engine creates/spawns/deletes an entity of `className` (`"*"` = all), delivering a serial-gated
  `EntityRef` (may be null) plus the `className`. Class-keyed, notify-only. Backed by a signature-scanned
  `CGameEntitySystem::AddListenerEntity` (the CSSharp/ModSharp `IEntityListener` mechanism).
- 9bdf2bb: Add `EntityRef.name` — read an entity's targetname (`CEntityIdentity::m_name`, a `CUtlSymbolLarge`). Serial-gated `string | null`: `""` when the entity has no targetname, `null` when the ref is stale/invalid. Unblocks name-based entity/zone discovery (e.g. classifying map triggers by `map_start`/`map_end`).

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
