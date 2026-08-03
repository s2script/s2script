# @s2script/cs2

## 0.12.0

### Minor Changes

- 952c41c: `ctx.gameRules.onTerminateRound`'s event view no longer carries `_unused3` / `_unused4`.

  Those two were typed `readonly number` and were never readable — the underlying parameters are not
  32-bit integers. The engine's third argument to `CCSGameRules::TerminateRound` is a **pointer**
  (`mov %rdx,-0xe0(%rbp)`, a full 64-bit store), so declaring it `int` truncated whatever the engine
  passed: the hook handed the original function half a pointer, and a later dereference segfaulted a
  live server.

  The descriptor now uses a shape that relays both trailing arguments at full register width as opaque
  pass-through — they reach the original bit-for-bit and are deliberately not exposed to plugins, since
  there is nothing a plugin could correctly do with them. `delay` and `reason` are unchanged and remain
  writable.

  Marked `minor` rather than `patch` because the two properties disappear from the emitted type. Any
  plugin that referenced them was reading `undefined` behind a `number`, so nothing that worked stops
  working — but it is a compile-visible change and should not arrive silently in a patch.

- f0cb022: Generate the `ctx` hook augmentation from gamedata `hooks` descriptors (`s2s gen-hooks`), and gate
  its freshness.

  `packages/cs2/hooks.generated.d.ts` is new: for every gamedata-declared hook (currently
  `onTerminateRound`/`onRespawn`), it emits a typed view interface (mutable params writable,
  everything else — including a books-gated `EntityRef` receiver — readonly) and augments
  `PluginContext` with one readonly member per `expose.ctx` namespace (`ctx.gameRules`,
  `ctx.players`), so a plugin subscribing to a hook that does not exist, or one that has drifted, is a
  `tsc` build failure rather than a silent no-op. `@s2script/sdk` gains the `s2s gen-hooks [--check]`
  CLI command that produces it, plus a fix to the typecheck gate so a CS2 plugin sees
  `@s2script/cs2`'s `ctx` augmentation even when its own source never imports a name from
  `@s2script/cs2` directly (mirrors the existing fix for `@s2script/sdk/unsafe`'s plugin-declared
  `EngineCalls`).

### Patch Changes

- Updated dependencies [f0cb022]
  - @s2script/sdk@0.20.0

## 0.11.9

### Patch Changes

- 3673991: Correct the `Player.respawn()` / `GameRules.terminateRound()` doc comments to describe the mechanism
  that now backs them. Both were retired from bespoke shim C++ into declarative gamedata descriptors
  (A5b) with their next-frame drains reimplemented in the game package, so "executes outside the JS
  isolate borrow" is no longer how the delivery guarantee is obtained — the deferred-dispatch queue is.
  Two behaviours that were always true but undocumented are now stated: `respawn()` is idempotent for
  the same player within one frame (both calls return `true`), and `terminateRound()` is single-slot
  latest-wins. No signature, argument or return type changes.
- Updated dependencies [2e7eb80]
  - @s2script/sdk@0.19.0

## 0.11.8

### Patch Changes

- Updated dependencies [f8df0d8]
  - @s2script/sdk@0.18.0

## 0.11.7

### Patch Changes

- Updated dependencies [1e3e691]
- Updated dependencies [d91add2]
  - @s2script/sdk@0.17.0

## 0.11.6

### Patch Changes

- Updated dependencies [3986f43]
  - @s2script/sdk@0.16.0

## 0.11.5

### Patch Changes

- Updated dependencies [a1594fc]
  - @s2script/sdk@0.15.0

## 0.11.4

### Patch Changes

- Updated dependencies [89042b0]
  - @s2script/sdk@0.14.0

## 0.11.3

### Patch Changes

- Updated dependencies [8b7bcc7]
  - @s2script/sdk@0.13.0

## 0.11.2

### Patch Changes

- Updated dependencies [bdd653b]
- Updated dependencies [9199631]
  - @s2script/sdk@0.12.0

## 0.11.1

### Patch Changes

- Updated dependencies [6c3ddfd]
- Updated dependencies [922b677]
- Updated dependencies [4b6f610]
  - @s2script/sdk@0.11.0

## 0.11.0

### Minor Changes

- dfcbf71: Add `notifyChanged()` to nav wrappers, so a chain write can be replicated.

  Nav setters deliberately do not flag the sub-object for replication: for most targets the server reads
  the field every tick and replication is irrelevant, and notifying with a chain-relative offset would
  mark the wrong bytes on the wrong entity.

  It is NOT irrelevant for anything a CLIENT renders. `Player.matchStats` drives the scoreboard, so a
  write nobody is told about is a write nobody sees — a gamemode hiding kill counters would set them and
  watch the old values stay on screen.

  `notifyChanged()` notifies at `path[0]`, which is a field on the ROOT entity and therefore the offset
  the engine's dirty-tracking understands — exactly what the C# reference does for this case
  (`SetStateChanged(controller, "m_pActionTrackingServices")`). It stays an explicit call rather than
  something setters do implicitly, because which hop to notify is a property of the chain, not of the
  field being written. A wrapper reached with no pointer hop no-ops rather than notifying at a
  fabricated offset 0.

## 0.10.0

### Minor Changes

- b6f1884: Named enum constants: `moveType = MoveType_t.VPHYSICS` instead of `moveType = 5`.

  Enum fields were typed `number`, so every value had to be hardcoded from Source convention or another
  framework — the borrowed-constant problem, one level below the offsets it was already solved for. The
  dump now walks each enum's enumerators (`SchemaEnumInfoData_t::m_pEnumerators`) alongside the width it
  already read, into a sibling `schema-enums.json`.

  87 enums are emitted as frozen const objects with a value-union type, and the fields that hold them
  narrow from `number` to the enum type — so a stray integer is a compile error. Only enums a generated
  field can actually hold are emitted; the other 430 in the dump belong to animgraph, particle and
  renderer classes that are not generated, and would be names for values nothing can be assigned.

  THIS FINDS REAL BUGS. Three of the five constants a TTT port hardcoded the same day were wrong:
  MOVETYPE_VPHYSICS is 5 (it used 9, which is MOVETYPE_LADDER), MOVETYPE_FLY is 3 (it used 5, which is
  VPHYSICS), and kRenderNone is 2 (it used 1, which is kRenderTransAlpha — the glow only looked right
  because transparent happened to be close enough).

  Case is preserved rather than PascalCased: `MOVETYPE_VPHYSICS` has no unambiguous PascalCase form. The
  enum's own redundant prefix is stripped (`MoveType_t.VPHYSICS`), all-or-nothing per enum — one member
  that does not fit, or two that would collapse, and that enum keeps raw names, because a
  partly-stripped enum is worse than an unstripped one.

  Nested schema enums (`CFuncMover::Move_t`) sanitise to `CFuncMover__Move_t`; two that would collapse
  onto one identifier are both dropped to plain integers rather than emitted ambiguously.

  Absent `schema-enums.json` degrades to exactly the previous output — verified byte-identical.

### Patch Changes

- 3d6702a: Ship `weapon.d.ts`. `Weapon` was resolving to `any` for every consumer.

  `index.d.ts` has always done `export { Weapon } from "./weapon"`, but `weapon.d.ts` was missing from
  the package's `files` array, so it never reached the tarball. A dangling relative specifier does not
  error — TypeScript resolves it to `any` — so `pawn.activeWeapon` and everything reached through it
  silently lost its type and code that should not have compiled compiled fine. A downstream plugin hit
  this and hand-wrote the interface plus a runtime property probe to work around it.

  Adds a test that walks every `.d.ts` in each publishable package's REAL tarball (via
  `npm pack --dry-run --json`, so it cannot disagree with npm's own packing rules) and asserts each
  relative re-export target ships too, plus a companion check that every declared `exports` subpath
  points at a shipped file. Verified against the bug: reverting the one-line fix fails the test with the
  offending specifier named.

- Updated dependencies [b6f1884]
  - @s2script/sdk@0.10.2

## 0.9.0

### Minor Changes

- e2b1e99: Reach fields behind a struct embedded in a pointer target, and add `Player.matchStats`.

  A nav target could only expose fields declared on the target class or an ancestor. Anything behind an
  embedded struct was unreachable, because every entry in a `read*Via` path is DEREFERENCED and an
  embedded struct must not be — it is part of the object already reached.

  `nav-targets.json` entries take an optional `base`: hops whose offsets are SUMMED into the field
  offset instead. `CCSPlayerController_ActionTrackingServices::m_matchStats` is one — the stats live
  inside the services object, so `m_iKills` is one pointer hop plus a base offset plus the field. A base
  lookup that fails returns `null` rather than falling through as 0, which would silently read the start
  of the target object and hand back plausible numbers from the wrong field.

  `Player.matchStats` uses it: kills/deaths/assists/damage/utilityDamage are writable, the rest of the
  block is engine bookkeeping and stays read-only, per the existing opt-in allowlist. This is the path a
  gamemode needs to hide scoreboard counters — TTT must zero them until a body is identified, and was
  carrying ~186 lines of hardcoded offsets, a probe and an operator override file to try. Those offsets
  were also wrong (`+0x7f8`/`+0x98` against a real `+2760`/`+208`), so the feature was silently disabled
  on every build — exactly the failure mode a borrowed constant produces, and why offsets now resolve by
  name at runtime instead.

  Wrapper constructors take a third `base` argument. Generated code only, but it is a signature change
  in `nav.generated.js` — regenerate rather than hand-merging.

- 8d25254: Schema codegen: embedded structs, enums, `wrapEntity`, and every entity class.

  Catch-up changeset — this work merged in #26, #28 and #29 without one, so neither package was
  versioned or published and the new surface is unreachable from npm.

  `@s2script/cs2` grows from 62 to 415 interfaces and 793 to 2740 exposed fields:

  - **Embedded structs** as nested accessors, at any depth — `pawn.glow.glowing`,
    `pawn.collision.collisionGroup`, and struct-inside-struct via
    `collision.collisionAttribute.collisionGroup`.
  - **`wrapEntity(className, ref)`** — schema accessors on an entity you created. `createEntity`
    returns a bare `EntityRef`; this is how it gets fields. Keyed on a generated class map, so a wrong
    name is a compile error rather than an object with silently missing accessors.
  - **Enums** as unsigned integers of the width their binding declares. They were skipped wholesale
    because the category names the type but not its width; the width is now dumped from the live
    SchemaSystem. 533 fields catalog-wide. Notably `moveType`, `solidType` and `renderMode`.
  - **`Color` as a packed uint32** (R in the low byte), which also exposes `m_clrRender` as `render`.
    Previously skipped as "not a scalar", which is what kept `m_glowColorOverride` unreachable.
  - **367 entity classes** instead of 13 — everything deriving from `CEntityInstance` that has fields,
    plus `CCSGameRules`. Costs 3.6 ms once at boot.
  - **Transparent value wrappers flattened**: `pawn.deathTime` is a `number`, not `{ value }`.
    `GameTime_t`/`GameTick_t` were the majority of embedded fields on a pawn.

  Offsets still resolve by NAME at runtime, so none of this bakes in a layout constant.

  `@s2script/sdk` is a patch: only the codegen internals and its tests changed, no published `.d.ts`,
  but the shipped `dist/cli.js` differs.

### Patch Changes

- Updated dependencies [8d25254]
  - @s2script/sdk@0.10.1

## 0.8.0

### Minor Changes

- a64da95: Make curated `pawn.movementServices` fields writable — the `GetMaxSpeed` equivalent.

  All 53 generated accessors on the movement services were read-only. 14 now have setters:
  `maxspeed`, `stamina`, `surfaceFriction`, `fallVelocity`, the duck group, and the six move-input
  fields. Writability is opt-in per field via a `writable` allowlist in `nav-targets.json` rather than
  derived from the field's type — which byte a field lives at is regenerable layout, but whether
  writing it is safe is a reviewed behavioural decision, and the type-derived set would have exposed
  engine bookkeeping such as `m_nTraceCount`.

  A bad allowlist entry now fails codegen instead of silently emitting nothing: an unknown field name
  (what a CS2 rename produces) and a kind with no `EntityRef.write*Via` both throw, naming the field.

  These writes are not flagged for replication — the change-notifier addresses the root entity while a
  nav write changes a subobject — so a predicting client may see brief mismatch. Stated on the emitted
  `MovementServices` interface.

- a64da95: Publish standard interface contracts for econ/skins and workshop — types only, no implementation.

  New subpaths: `@s2script/sdk/contracts/workshop` (`WorkshopService`, engine-generic) and
  `@s2script/cs2/econ` (`EconService`, `WeaponSkin`, `Loadout`, CS2-specific). A community plugin
  implements one and publishes it; consumers depend on the agreed shape via `ctx.tryUse` instead of a
  different ad-hoc interface per plugin.

  The framework ships no implementation deliberately: applying skins means driving CS2's economy item
  model and workshop means Steam's UGC services, neither of which is a Source 2 engine touchpoint.

  Every 64-bit value in these contracts — workshop published-file IDs, SteamIDs — is typed as a
  decimal **string**, because a `BigInt` throws crossing the plugin boundary and silently drops the
  whole payload. Everything Steam-facing is `Promise`-returning so an implementation cannot block the
  game frame.

  Also fixes game-package subpath resolution in the plugin typecheck: `@s2script/*` mapped to
  `<pkg>/index.d.ts`, which cannot express a subpath, so `@s2script/cs2/econ` was `TS2307`.

### Patch Changes

- Updated dependencies [a64da95]
- Updated dependencies [a64da95]
- Updated dependencies [a64da95]
- Updated dependencies [a64da95]
- Updated dependencies [a64da95]
- Updated dependencies [291e017]
  - @s2script/sdk@0.10.0

## 0.7.5

### Patch Changes

- Updated dependencies [6a423a1]
- Updated dependencies [6de4606]
  - @s2script/sdk@0.9.0

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
