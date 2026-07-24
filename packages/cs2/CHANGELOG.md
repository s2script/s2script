# @s2script/cs2

## 0.7.4

### Patch Changes

- Updated dependencies [afce5a2]
- Updated dependencies
  - @s2script/sdk@0.8.0

## 0.7.3

### Patch Changes

- Updated dependencies [d6949a1]
- Updated dependencies [74d45bd]
  - @s2script/sdk@0.7.0

## 0.7.2

### Patch Changes

- Updated dependencies [24864c0]
  - @s2script/sdk@0.6.0

## 0.7.1

### Patch Changes

- c9f0293: Rich TSDoc across the hand-authored `@s2script/cs2` game-type stubs (`Pawn`, `Weapon`, `ChatColors`, `RoundEndReason`, and the entry points) — descriptions, `{@link}` cross-references, and `@example`s drawn from real plugin/example usage — for complete in-editor intellisense. The generated schema/nav/event fields are intentionally left bare (a separate future effort). Types are unchanged; this is a comments-only pass verified against every cs2-consuming plugin and example.
- Updated dependencies [c9f0293]
  - @s2script/sdk@0.5.1

## 0.7.0

### Minor Changes

- ddcb4c6: BREAKING (pre-1.0 minor): `EntityRef` is now `{index, id}` — `id` is a host-minted
  liveness id replacing the raw engine `serial` on the public surface. Liveness is
  decided by the host's books (listener-fed, cleared per map), never by entity memory;
  stale refs — including across a changelevel — deterministically resolve to
  `null`/`false`. The inter-plugin/handoff wire format is `{__s2ref: [index, id]}`;
  pre-E1 `{__entref__}` blobs revive as inert data. The `EntityRef` constructor is no
  longer part of the public typed surface — the framework mints every ref.

### Patch Changes

- Updated dependencies [cb50b95]
- Updated dependencies [ddcb4c6]
- Updated dependencies [6cec7d0]
  - @s2script/sdk@0.5.0

## 0.6.1

### Patch Changes

- Updated dependencies [bd40c35]
- Updated dependencies [4db1f4f]
  - @s2script/sdk@0.4.0

## 0.6.0

### Minor Changes

- 4979320: Player.respawn(): respawn a dead player via the self-resolved CCSPlayerController::Respawn
  (byte-sig + RTTI-vtable-membership load-validated; queued one frame outside the JS isolate borrow
  so player_spawn reaches every plugin). Alive-guarded, serial-gated, degrades to false.
- 4050ac1: Round control: GameRules.terminateRound(reason, delay?) (sig-resolved CCSGameRules::TerminateRound,
  deferred one frame so round_end reaches every plugin), round-clock write surface
  (setRoundTime/setTimeRemaining/addTimeRemaining + roundStartTime/timeElapsed/timeRemaining reads),
  Teams score API (cs_team_manager CTeam.m_iScore), and the RoundEndReason/WinPanelFinalEvent const maps.
- e9a0640: `Player.switchTeam(team)` — non-lethal T/CT team switch (the player stays alive and keeps weapons; the
  pawn may be respawned) via the self-resolved `CCSPlayerController::SwitchTeam`. None/Spectator
  dispatches to ChangeTeam (CSSharp/SwiftlyS2 parity). Serial-gated; degrades to a no-op when the
  signature is unresolved. Closes the TTT-port "role→team without killing the player" gap.

### Patch Changes

- Updated dependencies [972103b]
- Updated dependencies [c8639f2]
- Updated dependencies [bb2891c]
  - @s2script/sdk@0.3.0

## 0.5.1

### Patch Changes

- Updated dependencies [d858f38]
- Updated dependencies [2ad151b]
  - @s2script/sdk@0.2.0

## 0.5.0

### Minor Changes

- 1675ba9: Team change + writable narrow-int schema fields.

  - `@s2script/cs2`: `Player.changeTeam(team)` and `Player.spectate()` — move a player's controller between teams (Spectator=1/T=2/CT=3) via the sig-resolved `CCSPlayerController::ChangeTeam` (serial-gated, degrade-never-crash). Narrow-int schema fields (`int8`/`int16`/`uint8`/`uint16`/`uint32`) now generate setters — `player.desiredFOV`, `player.teamNum`, etc. are writable.
  - `@s2script/cli`: `gen-schema` emits setters for narrow-int atomic fields (the `EntityRef.writeInt8/16`/`writeUInt8/16/32` methods already existed; the WRITE/ATOMIC maps were stale). 64-bit fields stay read-only.

- 9965b5b: Sound slice: new `@s2script/sound` module — `Sound.emit(name, { entity?, recipients?, volume? })`
  plays a named CS2 SoundEvent (engine GUID or 0; serial-gated source, bot recipients skipped) and
  `Sound.onPrecache(ctx => ctx.add(path))` registers custom resources into the session manifest at
  map load. CS2 sugar: `pawn.emitSound(name, opts)` + the curated `Sounds` constants.

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
