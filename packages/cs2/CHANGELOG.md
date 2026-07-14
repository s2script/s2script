# @s2script/cs2

## 0.4.0

### Minor Changes

- a3e5cc4: Add a CS2 `Weapon` entity object + player fire control.

  `@s2script/cs2` gains `Weapon` — an `EntityRef`-backed, serial-gated wrapper over `CCSWeaponBase` (`clip1`/`clip2`/`paintKit`/`owner`/`setAmmo`/`remove`, plus `Weapon.fromEntity`/`findAll`) — and new `Pawn` members: `activeWeapon` and `weapons` (now `Weapon`s), `giveNamedItem` (→ `Weapon`), `disarm`, and player fire control `blockFiring`/`allowFiring`/`nextAttack`.

  `@s2script/entity` gains `EntityRef.writeFloat32Via` and `writeBoolVia` — the write mirror of the `read*Via` pointer-chain accessors, over the `__s2_ent_ref_write_chain` core native.

### Patch Changes

- Updated dependencies [a3e5cc4]
- Updated dependencies [bb6b8fb]
- Updated dependencies [9bdf2bb]
  - @s2script/entity@0.3.0
  - @s2script/trace@0.1.3

## 0.3.0

### Minor Changes

- 4e69d7d: Runtime engine trigger zones. `@s2script/zones` now builds each zone as a real `trigger_multiple` entity with an arbitrary-box collision (any size, any aspect) and fires enter/leave off the engine's own touch system — replacing the previous ~8Hz origin-polling backend with engine-accurate detection that also sees non-player entities.

  New APIs powering it:

  - `@s2script/entity` — `EntityRef.setModel(name)` (build/register an entity's collision aggregate), `EntityRef.activateCollision()` (register + reshape the collision to the entity's bounds via `SetCollisionBounds` + `SetSolid(SOLID_BBOX)`), and `EntityRef.writeInt32Via(pathOffs, finalOff, value)` (write an int32 at the end of a pointer chain).
  - `@s2script/cs2` — `TriggerZone.create(min, max, opts?)` → a runtime box trigger whose `OnStartTouch`/`OnEndTouch` you hook via `Entity.onOutput`. Non-solid (pass-through), works on any map.

### Patch Changes

- Updated dependencies [4e69d7d]
  - @s2script/entity@0.2.0
  - @s2script/trace@0.1.2

## 0.2.0

### Minor Changes

- 0da49f2: Admin groups, immunity levels, and command overrides (SourceMod `admin_groups.cfg` parity, JSON-shaped).

  - New config files: `admin_groups.json` (named groups = flags + immunity + optional per-group command overrides) and `admin_overrides.json` (global command → required-flag remaps). `admins.json` is enriched to an object form (`{ groups, flags, immunity }`); the legacy flag-array form still works. Flag tokens accept names, SM single letters, or compact letter-strings.
  - `@s2script/admin`: `AdminInfo` gains `immunity` and `groups`; `Admin` gains `canTarget(callerSlot, targetSlot)` and `getGroup(name)`; `Admin.add` takes an optional `immunity`.
  - `@s2script/cs2`: `Player.target` gains an optional `filterImmunity` argument. The destructive base commands (kick / slap / slay / ban / gag / mute / gravity / noclip / freeze / blind / votekick) now refuse targets with higher immunity than the calling admin.

### Patch Changes

- 5fcc41f: Initial public npm release of the `@s2script/*` types packages and CLI (Changesets pipeline).
- Updated dependencies [5fcc41f]
  - @s2script/entity@0.1.1
  - @s2script/events@0.1.1
  - @s2script/math@0.1.1
  - @s2script/trace@0.1.1
