# @s2script/cs2

## 0.17.2

### Patch Changes

- 03449fb: Clarify hudkit initialization and callback lifetime, update its example to claim panels in OnPluginStart, and type Menu.registerRenderer's previous-renderer return value.
- 54200e1: ui: key the paint caches to the layout entity's identity, and make disconnect teardown unconditional

  `setText` / `setClass` / `show` / `hide` suppress redundant engine calls by
  comparing against a cached last-known value. Those caches describe state that
  lives on the layout ENTITY — and they used to outlive it. After a map change
  the new entity has every panel at its markup default and no input capture, but
  the caches still claimed everything was set: any value unchanged since the
  previous map was suppressed and never re-sent (a partial paint — a sheet draws
  its rows but not its buttons, intermittently by construction), and a surviving
  cursor lease made `releaseCursor` refuse to disable a capture the new entity
  never had — a player holding a pointer no click can clear.

  The caches are now bound to the entity's host id: every resolve passes through
  an identity gate that drops all of them (`lastValue`, `cursorLeases`,
  `visiblePanels`, `meterClass`, `disabled`) the first time a DIFFERENT entity is
  resolved. That covers a map change and an entity silently replaced mid-map
  alike, without depending on any lifecycle notification having fired.
  `resetForMap` also clears them eagerly so stale cursor leases die immediately
  rather than at the next paint. Click handlers are deliberately not cleared:
  they are registered once per button id and belong to the plugin, not the
  entity.

  Disconnect teardown (`forget(slot)`) is now authoritative and engine-first:
  input capture is forced off unconditionally — never gated on the lease books,
  which menuhud's `discardSession` or an earlier forget may already have emptied
  — and every class the books say the slot holds (visible panels, meter width,
  disabled buttons) is painted back to markup default so the slot's next
  occupant starts clean.

- feb84fd: Keep modal footer actions bound to each player's painted view and gate MODALS against the workshop markup.

  Players viewing different menu pages or action sets no longer overwrite one another's footer
  handlers. This also keeps automatic pager buttons working when another player has a one-page
  list. Closing or forgetting a viewer discards their handlers. `ModalSpec.buttons` now allows
  per-player actions; the former shared-table restriction and diagnostic warning are removed.

- Updated dependencies [03449fb]
  - @s2script/sdk@0.25.1

## 0.17.1

### Patch Changes

- 5bdf00d: menuhud: release the cursor on teardown even with no tracked session

  The disconnect/activate teardown returned early when it held no session for the
  slot, on the reasoning that there was nothing to release. That is wrong in the
  exact case the teardown exists for.

  A session is deleted by `renderer.close`, but the cursor grab is separate
  per-player state on the layout entity and is released separately. Any path that
  drops one without the other leaves the player captured — pointing at a menu that
  is no longer drawn — and reconnecting could not fix it, because by then there was
  no session left to find. Measured on a live server: the teardown ran and logged
  nothing at all, having already returned.

  Releasing state that is already released is free. Failing to release it is a
  player who cannot play. The teardown no longer asks whether it thinks it has
  anything to do; only the log line stays conditional, since `onActive` fires for
  every joiner.

  Menus also no longer freeze the player. Freezing is a convenience — it stops
  someone wandering off mid-menu — while being unable to move is a trap the moment
  anything else about the menu fails. On a live server that trap was real: a broken
  `sm_admin` left admins frozen, unable to select anything and unable to close it.
  The upside is cosmetic and the downside is a player who cannot play, so it stays
  off until the menu surface can guarantee it is interactive. The unfreeze path is
  kept so anyone frozen by an older build is released on the next teardown.

- f4c7e37: ui: six pooled center sheets instead of two

  Two was never an engine limit — it was how many `s2_m*` panel trees the shared
  workshop layout happened to define, set back when nothing needed more. Then one
  plugin wanted three surfaces (a shop, a round log, an admin queue) alongside the
  framework's own menu renderer, which claims one during the prelude, and four
  things wanted two slots.

  The failure was quiet, which is the worst part: `modal pool exhausted` in the
  server log, a shop silently degrading to the chat menu and a round log to the
  developer console, with nothing user-visible saying why.

  `s2script_lib.xml` now defines `s2_m0`–`s2_m5` and `MODALS` is 6. The pool size
  belongs to the markup — a server can only address panels the client's layout
  already contains, so raising `MODALS` alone would hand out sheets that paint
  nothing. A test pins the two together, and checks every sheet has its full
  complement of rows, footers and detail lines.

  **Requires a workshop republish of item 3790153369.** A client on the older
  addon has only `s2_m0`/`s2_m1`; Panorama ignores unknown ids silently, so such a
  client sees nothing for a third concurrent sheet rather than breaking. Claims are
  handed out lowest-first, so the common case keeps working on an old addon.

  Each sheet costs ~51 panel ids: the layout interns 432 of the 1024 cap.

## 0.17.0

### Minor Changes

- 8b8f46c: TopMenu hub is a tabbed dashboard; plugins declare their own tab

  `sm_admin` / `sm_menu` paint `hudkit.dashboard()` over `s2_dash` on
  `s2script_lib.xml` instead of a category → item drill-down. Each plugin that
  contributes items calls `topmenu.addTab({ id, title })` and `addItem(tabId, item)`.
  `addCategory(name)` is still the id==title form.

  Snapshot grows `tabs: [{ id, title }]`. Republish workshop addon 3790153369
  after compiling the new `s2_dash` panels (sources in `examples/hud-lab/workshop/`).

### Patch Changes

- c5ad561: menuhud: clear a slot's menu when its player leaves or re-activates

  A disconnect is not a close, so a player who left mid-menu never reached
  `renderer.close`. The session, the saved `moveType`, the cursor grab and the Tab
  arm all stayed on the slot, and the next occupant — normally the same person
  reconnecting — arrived frozen, input-captured, and waiting to click a sheet
  nobody had drawn for them.

  `Clients.onDisconnect` and `Clients.onActive` now both tear the slot down.
  Activate is the backstop, because a timeout or a crash does not always deliver
  the disconnect. The saved `moveType` is dropped rather than re-applied: the
  replacement pawn is fresh and already has the right one.

- 8ae767a: ui: add `Row.tone` so a list row can carry colour

  Rows could say "unavailable" (`disabled`) but never "good", "careful" or
  "bad" — the only levers a server has are classes and dialog variables, and no
  class existed for a row tint. Anything that wanted to call out one line in a
  list had to spend the text, prefixing a marker like `[BAD]` and hoping it read.

  `tone: "good" | "warn" | "bad"` sets a class on the row button, and the
  stylesheet tints the primary cell through a descendant rule — the same shape
  `.s2-btn-good .s2-btn-label` already uses, so the tint outranks `.s2-cell-a`
  by specificity and composes with the selection highlight and `disabled`
  instead of fighting them.

  Needs a workshop addon carrying the new `.s2-li-good` / `-warn` / `-bad`
  rules. On an older addon the client has no rule for the class and the row
  renders untinted, so a tone must never be the ONLY way a row says something.

- 84161f4: gamedata(cs2): re-derive CCSGameRules_TerminateRound for build 1.41.7.8/14178

  The pinned signature stopped matching and the descriptor degraded as designed —
  `call 'terminateRound' unavailable: signature did not match this build` — taking
  `onTerminateRound` with it, since the hook targets the same signature.

  The function did not move; its prologue was reordered. The reason capture went
  from `mov r15d,esi` to `mov r12d,esi` and the `lea rsi` anchor from fn+0xb to
  fn+0x10, so `string-xref.at` moves 11 -> 16.

  Re-derived by the recipe already in the file: the single rip-relative xref to
  "TerminateRound" (0x8f4d9b) lands at 0x13cd4a0, entry 0x13cd490, and the sole
  xref to "TerminateRound: unknown round end ID %i" sits in the same body. The new
  pattern is unique binary-wide, and the boot gate now arms both the call and the
  hook.

- Updated dependencies [8b8f46c]
  - @s2script/sdk@0.25.0

## 0.16.1

### Patch Changes

- e4348a1: Do not read `hudkit.layout` during CS2 prelude eval.

  `hudkit` methods and the `layout` getter used to close over `__s2_game_ns("ui")`, a load-window proxy. `run_prelude` evaluates `pawn.js` before `__s2_load_ctx` exists, so `menuhud`'s `hudkit.modal()` and the vote rail's eager `hudkit.layout` both threw `ui outside the load window` and aborted the rest of the prelude — `Vote.registerTallyRenderer` never ran.

  Resolve the shared kit through the `ui` factory instead. The vote rail holds `hudkit` and reads `.layout` inside functions, same shape as menuhud.

## 0.16.0

### Minor Changes

- 778061f: Reshape the CS2 HUD API around `custom_hud_layout`.

  `ui` was a kitchen-sink namespace that did not match the engine object: a `custom_hud_layout` entity, driven per-player. The public face is now `CustomHudLayout.create(spec)` returning a `HudLayout`, painted through `layout.forSlot(slot)` so drive calls do not re-thread the slot. Clicks hand back that same player view. Unchanged values are not re-sent. `hudkit` is the shared modal/toast/badge pool (formerly `ui.components()`).

  `ui` / `hud()` / `components()` remain as deprecated aliases of the same objects.

- 7bcca3b: Add callout, banner, and MOTD panels to `s2script_lib.xml` (addon 3790153369).

  `hudkit.callout` is a bottom-center hint (no cursor). `hudkit.banner` is a center-top broadcast strip. `hudkit.motd` is a scrim + OK overlay, not a third center sheet. Republish the same workshop item after compiling.

- 06ae122: Paint CS2 menus as hudkit center sheets.

  `Menu.activation` (`immediate` | `tab`, default immediate) is generic. On CS2 both `MenuStyle.Center` and `MenuStyle.Chat` use one host-lifetime hudkit modal; Tab intercept lives on `HudInput`. Exhausted modal pool keeps the existing Chat renderer.

- 0014b56: Paint CS2 votes as a right-side rail on `s2script_lib.xml` (addon 3790153369). The lib source (including `s2_vote*`) is in `examples/hud-lab/workshop/`.

  `VoteTally.choice` is this slot's cast (or null). A registered tally renderer always paints; `showLiveTally` is leftover when a renderer exists. Chat is one line. HUD clicks go through `__s2_vote_cast`.

### Patch Changes

- Updated dependencies [44ab392]
- Updated dependencies [06ae122]
- Updated dependencies [0014b56]
  - @s2script/sdk@0.24.0

## 0.15.0

### Minor Changes

- ac3fc9b: `plugin((ctx) => …)` is no longer a public authoring API. Plugins export `OnPluginStart` (plus named publics). Load-window `hook.*` / `previous()` / `pluginId()` / `command.onClientCommand` cover the remaining ctx-only gaps. CS2 `ui`, `gameRules`, `players`, and `items` are free load-window exports. 0.x minor bump.
- 63878fe: Spawn custom HUD layouts when a client becomes active.

  `hud()` / `components()` / `createLayout()` register the descriptor at load and create the layout entity on `SIGNON_ACTIVE`, so player-join and game events can drive panels. `OnMapStart` still only resets. A drive never creates the entity as a side-effect of paint.

### Patch Changes

- 0165900: Document the public HUD API as `ui` (`import { ui } from "@s2script/cs2"`), not `ctx.ui`. Runtime still hangs the same object off the load ctx; authors import `ui`.
- 3ec0430: TriggerZone / OutputEvent docs name the current `onOutput` subscribe. The `@s2script/zones` contract's `on` is ledgered (`void`, no `off`).
- Updated dependencies [4745b5c]
- Updated dependencies [877cc23]
- Updated dependencies [f13e6ab]
- Updated dependencies [ac3fc9b]
- Updated dependencies [2e51352]
- Updated dependencies [41ef7d6]
- Updated dependencies [e53d269]
- Updated dependencies [c3dde5b]
- Updated dependencies [88b508c]
- Updated dependencies [fb2434c]
- Updated dependencies [0079d74]
- Updated dependencies [2a16059]
- Updated dependencies [b4200fb]
- Updated dependencies [cd06cab]
- Updated dependencies [4df7325]
- Updated dependencies [920c823]
- Updated dependencies [3ec0430]
  - @s2script/sdk@0.23.0

## 0.14.0

### Minor Changes

- e74edfd: Add `ctx.ui.components()` — a shared, pooled Panorama component library.

  `ctx.ui.hud()` drives panel ids that some `.xml` declares, so using it directly
  means authoring and publishing your own workshop layout before you can draw a
  row. `components()` is the library over that primitive: plugins describe data
  (rows, titles, handlers) and never touch an id. Paging, selection, per-player
  state and the reveal are handled for them.

  This also keeps plugins inside an engine limit. `CCSCustomHudLayout` interns
  every panel id, class name and dialog variable the server references into three
  networked vectors, each capped at 1024, and those vectors belong to the ENTITY —
  every plugin shares them. Private per-plugin layouts consume that budget
  multiplicatively and fail late, when the Nth plugin loads. A shared pool is
  interned once and reused, so cost tracks what is on screen rather than plugin
  count. `Components.budget()` reports the three counts separately, because they
  are three separate 1024s and a combined total is wrong in both directions.

  Also in this change:

  - `ctx.ui.createLayout()` spawns the layout entity explicitly. Drives no longer
    create one implicitly: creating a `custom_hud_layout` from inside a frame or
    event dispatch segfaults the server, and lazily creating it on first draw made
    that hazard depend on which plugin happened to draw first.
  - `Modal.cursor()`, `select()` and the `detail()` callback all speak ABSOLUTE
    indices, matching `onPick`. They previously mixed absolute and page-relative,
    so "act on the selection" operated on the wrong row on every page but the
    first — silently, and only once a list was long enough to page.

- e74edfd: Add lifecycle-bound `ctx.ui` custom HUD API with promoted engine calls (`setHasClassForPlayer`, `setDialogVariableStringForPlayer`, `setInputCaptureEnabledForPlayer`) and `onCustomHudClicked` hook. Ships default `s2script_hud.xml` descriptor for workshop addon 3790153369.
- 747f312: Add `ctx.items.onCanAcquire` / `onCanAcquirePost` — a first-class pickup gate over `CCSPlayer_ItemServices::CanAcquire`. Plugins can refuse a pickup (or a `giveNamedItem`) with `AcquireResult` + `HookResult`. The item view is block-scoped scalars, never a pointer.

### Patch Changes

- 747f312: A plugin engine call is visible to other plugins before it returns. `Events.fire` nests `on`/`onPre`. `Player.respawn()` and `GameRules.terminateRound()` run on this call (no next-frame queue). `Server.setCvar` writes through ICvar now (boolean return; `getCvar`/`onCvarChange` see the new value on the same call).
- Updated dependencies [747f312]
- Updated dependencies [747f312]
- Updated dependencies [dd8f333]
  - @s2script/sdk@0.22.0

## 0.13.0

### Minor Changes

- 9cf800c: Add five entity property setters, `Pawn.maxSpeed`, and their `Sound` / `Pawn` spellings

  Each of these wraps an engine function that has **no working schema equivalent**, which is the
  reason they are engine calls rather than field writes:

  - `EntityRef.setGravityScale(scale)` — `CBaseEntity::SetGravityScale`. The setter early-returns when
    the value is unchanged and maintains a second field (`m_flActualGravityScale`), so a plugin that
    writes `m_flGravityScale` directly sees nothing happen. That trap is the whole reason this exists.
  - `EntityRef.applyAbsVelocityImpulse([x,y,z])` — `CBaseEntity::ApplyAbsVelocityImpulse`. Additive and
    physics-aware, for knockback and boosts. `teleport(null, null, velocity)` sets velocity absolutely;
    a raw `m_vecAbsVelocity` write skips the partition/physics update entirely.
  - `EntityRef.stopSound(name)` — `CBaseEntity::StopSound`, the counterpart to `Sound.emit`.
    Also spelled `Sound.stop(name, { entity })` and `pawn.stopSound(name)`.
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

  **Reachable where you would look for them, not only on `EntityRef`.** The ops live at the entity
  layer because that is what the engine functions are, but that is not where a plugin author looks:

  - `Sound.stop(name, { entity })` sits beside `Sound.emit`. `entity` is **required** here, unlike
    `emit` — the engine call is an instance method reached through the books-gated entity resolve, so
    there is no global/2D form to fall back to the way `emit` defaults to worldspawn.
  - `pawn.stopSound(name)` mirrors the existing `pawn.emitSound(name)`.
  - `pawn.setGravityScale()`, `pawn.applyAbsVelocityImpulse()` and `pawn.setModelScale()` forward to
    the pawn's own serial-gated `EntityRef`, so gravity and knockback are one call on the object a
    plugin actually holds rather than `pawn.ref.setGravityScale(...)`.

  `setBodyGroupByName` is deliberately **not** forwarded to `Pawn`: it is a model concern rather than a
  player one, and it stays reachable as `pawn.ref.setBodyGroupByName(...)`.

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

### Patch Changes

- Updated dependencies [9cf800c]
- Updated dependencies [1c00b89]
  - @s2script/sdk@0.21.0

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
