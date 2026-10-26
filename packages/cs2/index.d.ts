/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API. NO runtime code.
 * The typed field accessors are GENERATED (schema.generated.d.ts) from the schema catalog by
 * `s2script gen-schema`; the typed nav wrappers are GENERATED (nav.generated.d.ts) from
 * nav-targets.json by `s2script gen-nav`; the typed event interfaces are GENERATED (events.generated.d.ts)
 * from the event catalog by `s2script gen-events`; the `ctx` hook augmentation + its view interfaces
 * are GENERATED (hooks.generated.d.ts) from gamedata/cs2/game.cs2.jsonc's `hooks` by
 * `s2script gen-hooks`; this file adds the hand-written entry points on top.
 */
import type { EntityRef, EntityKeyValueMap } from "@s2script/sdk/entity";
import type { Vector, QAngle } from "@s2script/sdk/math";
import type { TraceHit } from "@s2script/sdk/trace";
export * from "./schema.generated";
// `export *` (not a named list): the hooks.generated PluginContext augmentation takes effect the
// moment this file is part of the program, regardless of which names are re-exported — but a
// plugin author who wants to name a view type explicitly (e.g. `OnTerminateRoundView`) needs it
// re-exported from here too, exactly like every other generated cs2 artifact. A new hook's view/ctx
// interfaces surface automatically; no index.d.ts edit needed.
export * from "./hooks.generated";
export type {
  Badge,
  BadgeSpec,
  BadgeView,
  BannerSpec,
  CalloutSpec,
  Components,
  CtxUi,
  CustomHudClickedView,
  CustomHudLayoutNs,
  CustomHudSpec,
  Dashboard,
  DashboardSpec,
  DashboardView,
  OwnedDashboard,
  DashRow,
  DashTab,
  FooterButton,
  Hud,
  HudKit,
  HudKitPlayer,
  HudLayout,
  HudPlayer,
  HudResult,
  LayoutDescriptor,
  LayoutSlot,
  Modal,
  ModalOpenResult,
  ModalSpec,
  ModalView,
  MotdHandle,
  MotdSection,
  MotdSpec,
  OnCustomHudClickedView,
  Row,
  ToastSpec,
  UiErrorCode,
  UiResult,
  UiStatus,
  UiFocusOptions,
  UiSubscription,
  UiSurfaceHandle,
  Variant,
} from "./ui";
export { CustomHudLayout, DEFAULT_HUD_DESCRIPTOR, hudkit, PROBE_LAYOUT, ui } from "./ui";
export { AcquireMethod, AcquireResult, items } from "./items";
export type { CanAcquireView, CtxItems, ItemsApi } from "./items";
export { chat } from "./chat";
export type { CanHearChatView, ChatApi } from "./chat";
export { weapons } from "./weapons";
export type { WeaponDropView, WeaponGateView, WeaponsApi } from "./weapons";
export { movement } from "./movement";
export type { MoveView, MovementApi } from "./movement";
export declare const HudInput: {
  /** Arm Tab-to-activate for `slot`. Replaces any previous arm on that slot. */
  arm(slot: number, opts: { onActivate: () => void }): void;
  /** Drop the arm. Safe if nothing is armed. Does not close the HUD. */
  disarm(slot: number): void;
  isArmed(slot: number): boolean;
  /** True after Tab (or an immediate activate) until disarm. */
  isActive(slot: number): boolean;
};
import type { CCSPlayerPawn, CCSPlayerController } from "./schema.generated";
export type { SceneNode, WeaponServices, MovementServices, AimPunchServices, MatchStats } from "./nav.generated";
import type { SceneNode, WeaponServices, MovementServices, AimPunchServices, MatchStats } from "./nav.generated";
export { GameEvent } from "@s2script/sdk/events";
export type { GameEvents } from "./events.generated";
export { CsItem } from "./csitem.generated";
export { Weapon, WeaponData } from "./weapon";
// A re-export (`export { Weapon }`) does NOT create a local binding, so the Pawn interface below
// referenced an unbound `Weapon` → TS2304, swallowed by the gate's skipLibCheck into `any`. Bind it
// locally too (the SceneNode pattern on line 14) so activeWeapon/weapons/giveNamedItem stay typed.
import type { Weapon } from "./weapon";
export { CustomPlayerCamera, CustomCameraMode } from "./camera";
export type { CameraFollowConfig, CustomCameraModeValue } from "./camera";
import type { CustomPlayerCamera } from "./camera";

/**
 * A CS2 player pawn (the in-world body): the generated CCSPlayerPawn schema fields + the liveness-gated ref.
 * `controller` is the typed reverse hop (shadows the raw generated m_hController handle).
 * Nav props (sceneNode, weaponServices, movementServices, aimPunchServices) are generated from nav-targets.json.
 */
export interface Pawn extends Omit<CCSPlayerPawn, "controller"> {
  /** The backing entity ref (the pawn's liveness-gated identity; escape hatch to the raw {@link EntityRef} surface). */
  readonly ref: EntityRef;
  /**
   * SourceMod/CSSharp-sense validity: the pawn is live (liveness-gated) AND fully spawned (out of the
   * engine EF_IN_STAGING_LIST). Prefer this over `ref.isValid()` for any pawn WRITE — a staged-but-live
   * pawn passes `ref.isValid()` yet segfaults on SetModel/SetParent. Falls back to liveness if the
   * staging-flag read is unavailable.
   */
  readonly isValid: boolean;
  /** The player controlling this pawn, or null if stale/absent. */
  readonly controller: Player | null;
  /** World-space position (via the CGameSceneNode pointer chain), or null if stale. */
  readonly origin: Vector | null;
  /** Where the camera sits: `origin` + the networked view offset (SourceMod `GetClientEyePosition`).
   *  Follows crouching. Null if stale, or if this build's view-offset layout fails its schema check. */
  readonly eyePosition: Vector | null;
  /** Body world rotation (via the CGameSceneNode pointer chain); distinct from the view/aim `eyeAngles`. */
  readonly angles: QAngle | null;
  /** The pawn's scene node (world transform) — absOrigin/absRotation/scale/…, via the CBodyComponent→CGameSceneNode chain. */
  readonly sceneNode: SceneNode | null;
  /** The pawn's weapon services (active weapon, …). */
  readonly weaponServices: WeaponServices | null;
  /** The pawn's movement services (duck/ladder/…). */
  readonly movementServices: MovementServices | null;
  /** The pawn's aim-punch services (recoil angles). */
  readonly aimPunchServices: AimPunchServices | null;
  /** Best-effort velocity write (m_vecAbsVelocity); returns false if stale/unresolved. */
  setVelocity(x: number, y: number, z: number): boolean;
  /**
   * Allocation-free movement snapshot for per-tick hot paths (run timers, replay recorders). One
   * native call writes {@link Pawn.KINEMATICS_SLOTS} (13) numbers into your preallocated
   * `out` from `outIndex` (default 0):
   *
   * `[0..2]` origin xyz · `[3..5]` eye angles pitch/yaw/roll · `[6..8]` abs velocity xyz ·
   * `[9]` `m_fFlags` · `[10]` `m_MoveType` · `[11]` duck amount (0..1) · `[12]` held buttons (low
   * 32 bits, same as {@link Pawn.buttons}). `[11]`/`[12]` are NaN if the movement services pointer
   * is null.
   *
   * False when the pawn is stale, a schema field did not resolve, or `out` is too short.
   * @example
   * const snap = new Float64Array(Pawn.KINEMATICS_SLOTS);
   * if (pawn.readKinematics(snap)) console.log(Math.hypot(snap[6], snap[7]));   // 2D speed
   */
  readKinematics(out: Float64Array, outIndex?: number): boolean;
  /** Acquire the pawn-owned camera, creating it on demand. Null if stale or bindings unavailable. */
  getCustomCamera(): CustomPlayerCamera | null;
  /** The pawn's MoveType_t (uint8; null on a stale ref). Setting writes both m_MoveType and
   *  m_nActualMoveType + notifies. Values (MoveType_t): NONE=0, WALK=2, NOCLIP=7. */
  moveType: number | null;
  /** The currently-pressed button mask (low 32 bits; IN_USE/E = 32). 0 if the mask is unreadable. */
  readonly buttons: number;
  /** Kill this pawn via the sig-resolved CommitSuicide engine op (liveness-gated; no-op if stale). */
  slay(): void;
  /** The pawn's current movement speed cap (`CCSPlayerPawn::GetPlayerMaxSpeed`).
   *
   *  A getter rather than a field because the engine computes it — there is no `m_flMaxSpeed` on
   *  `CCSPlayerPawn` in the schema, so there is nothing to read. `null` (never 0) when the descriptor
   *  is unavailable or the ref is stale: 0 is a legitimate speed for a frozen player, so it must not
   *  double as "unavailable". */
  readonly maxSpeed: number | null;
  /** Give this pawn a named item/weapon (e.g. CsItem.AK47 or a raw "weapon_*" string). Returns the created
   *  Weapon, or null if unresolved/failed/stale. */
  giveNamedItem(name: string): Weapon | null;
  /** The currently-deployed weapon (m_hActiveWeapon), or null if none/stale. */
  readonly activeWeapon: Weapon | null;
  /** This pawn's held weapons (m_hMyWeapons, a CUtlVector<CHandle>). Empty if stale/unresolved/none. */
  readonly weapons: Weapon[];
  /** Remove ONE weapon through its owner's weapon services, including inventory/deploy transitions.
   *  False if absent/stale or the owned weapon's native service is unavailable. */
  removeWeapon(weapon: Weapon): boolean;
  /** Remove ALL held weapons (folds over Weapon.remove). true iff every one removed. */
  stripWeapons(): boolean;
  /** Alias of stripWeapons — destroy all held weapons. */
  disarm(): boolean;
  /** Drop the deployed weapon as a world pickup (SourceMod `CS_DropWeapon` on the active weapon). The
   *  engine picks the throw velocity. false if there is no active weapon, the binding is degraded, or
   *  the pawn is stale. */
  dropActiveWeapon(): boolean;
  /** Drop one weapon this pawn holds as a world pickup (SourceMod `CS_DropWeapon`). Unlike
   *  {@link removeWeapon}, the weapon entity survives. false if the weapon is not this pawn's, either
   *  handle is stale, or the binding is degraded. */
  dropWeapon(weapon: Weapon): boolean;
  /** The current fire gate (m_flNextAttack, seconds), or null if unresolved/stale. Read companion to blockFiring. */
  readonly nextAttack: number | null;
  /** Block ALL weapon fire for `seconds` (default effectively-indefinite) by writing m_flNextAttack. The
   *  gate is server-authoritative and time-based: a durable block needs a large value or a per-frame refresh.
   *  Returns false if unresolved/stale. */
  blockFiring(seconds?: number): boolean;
  /** Clear a fire block (m_flNextAttack = now). Returns false if unresolved/stale. */
  allowFiring(): boolean;
  /**
   * Ray-trace from this pawn's eyes along its view angles — "what is this player looking at".
   * Eye = `eyePosition` (falling back to origin + 64u if it is unavailable); direction = `eyeAngles`. Ignores
   * this pawn's own entity unless `ignoreEntity` is given. Returns null if the transform/angles are
   * unreadable (stale ref). `distance` defaults to 8192.
   */
  aimTrace(opts?: { distance?: number; mask?: number; ignoreEntity?: EntityRef }): TraceHit | null;
  /** Play a named CS2 SoundEvent from this pawn (the liveness-gated source entity; a stale ref emits
   *  nothing). Returns the engine sound GUID (nonzero) or 0. Bot recipients are always skipped. */
  emitSound(name: string, opts?: { recipients?: number[]; volume?: number }): number;
  /** Stop a named SoundEvent playing on this pawn — the counterpart to {@link Pawn.emitSound}.
   *  No `recipients`/`volume`: the engine stops it for everyone hearing it. False if the ref is
   *  stale or the op is unavailable. */
  stopSound(name: string): boolean;
  /** Set this pawn's gravity multiplier (1 normal, 0 weightless). Prefer this over writing
   *  `m_flGravityScale`, which the engine ignores — see `EntityRef.setGravityScale`. False if the
   *  ref is stale, the op is unavailable, or `scale` is not finite. */
  setGravityScale(scale: number): boolean;
  /** Add to this pawn's velocity, physics-aware — knockback, boosts, explosions. Additive, unlike
   *  `teleport(null, null, velocity)` which sets velocity absolutely. `impulse` is an [x,y,z] triple.
   *  False if the ref is stale, the op is unavailable, or any component is not finite. */
  applyAbsVelocityImpulse(impulse: number[]): boolean;
  /** Set this pawn's model scale. The argument shape is confirmed against the pinned build, but the
   *  engine function's NAME is a catalogue attribution its body does not prove — verify the effect
   *  before relying on it in a shipped plugin. Calling it is safe either way. */
  setModelScale(scale: number): boolean;
}
/**
 * Entry point for the {@link Pawn} body object — resolve the in-world pawn for a player slot.
 * @example
 * import { Pawn } from "@s2script/cs2";
 * const pawn = Pawn.forSlot(0);
 * if (pawn) console.log(pawn.activeWeapon?.clip1);
 */
/**
 * The world transform (`absOrigin`, `absRotation`, …) of any entity, through the same
 * CBodyComponent → CGameSceneNode chain as `pawn.sceneNode`. Null for a stale ref.
 * @example
 * import { Entity } from "@s2script/sdk";
 * import { sceneNodeOf } from "@s2script/cs2";
 * for (const dest of Entity.findByClass("info_teleport_destination")) {
 *   const node = sceneNodeOf(dest);
 *   console.log(dest.name, node?.absOrigin, node?.absRotation);
 * }
 */
export declare function sceneNodeOf(ref: EntityRef): SceneNode | null;

export declare const Pawn: {
  /** The Pawn for a player slot, or null if unoccupied / invalidated. */
  forSlot(slot: number): Pawn | null;
  /** Slots one {@link Pawn.readKinematics} snapshot fills (13). */
  readonly KINEMATICS_SLOTS: number;
};

/**
 * A CS2 player (the persistent controller entity): the generated CCSPlayerController schema fields
 * (team/score/ping/…) + the liveness-gated controller ref. `pawn` is the typed body (shadows the
 * raw generated m_hPawn handle). Bound to a host connection lifetime — a stored Player does not
 * target a replacement in the same slot.
 */
export interface Player extends Omit<CCSPlayerController, "pawn"> {
  /**
   * The backing entity ref (the persistent controller's liveness-gated identity). A saved handle
   * does not target a replacement in the same slot.
   */
  readonly ref: EntityRef;
  /** The 0-based player slot (`CPlayerSlot`) — not sufficient by itself to identify a connection. */
  readonly slot: number;
  /** This player's in-world pawn (the body), or null if dead/absent. */
  readonly pawn: Pawn | null;
  /**
   * The pawn this player is spectating (dead or on the spectator team), via the controller's
   * observer pawn and its observer services. `null` while alive, in free-roam, or when unresolved.
   * Resolve the watched player with `observerTarget.controller`.
   */
  readonly observerTarget: Pawn | null;
  /** The engine observer mode (`ObserverMode_t`: 0 none, 2 in-eye, 3 chase, 4 roaming…), `-1` while alive/unresolved. */
  readonly observerMode: number;
  /**
   * Per-match scoreboard counters — kills/deaths/assists/damage — via the
   * `m_pActionTrackingServices` pointer hop plus the embedded `m_matchStats` block.
   *
   * `null` when the services pointer is not resolvable (a controller with no tracking services yet).
   * Only the four counters a gamemode legitimately rewrites are writable; the rest of the block is
   * engine bookkeeping and stays read-only. See nav-targets.json.
   */
  readonly matchStats: MatchStats | null;
  /** The engine user-id (session-stable; NOT a schema field). `-1` if unassigned/absent. */
  readonly userId: number;
  /**
   * The client's SteamID64 as a decimal string (engine `GetClientXUID`). `"0"` for bots / unauthenticated.
   * This is the AUTHORITATIVE id for admin lookups (`Admin.get`/`forSlot` use it). Do NOT confuse it with
   * the schema-generated `steamID` (capital ID, from `m_steamID`) — that controller field is `string | null`
   * and can be `"0"`/`null`, so using it for authorization decisions is unreliable.
   */
  readonly steamId: string;
  /** Disconnect this player (engine KickClient). */
  kick(reason?: string): void;
  /** Overwrite the player's display name (m_iszPlayerName); returns false if stale/unresolved. */
  setName(name: string): boolean;
  /**
   * Move this player's controller to `team` (Spectator=1, Terrorist=2, CounterTerrorist=3) via the
   * sig-resolved CCSPlayerController::ChangeTeam. Serial-gated; a no-op if the ref is stale or the
   * signature is unresolved. `team` is bounded to 0..3 engine-side.
   */
  changeTeam(team: number): void;
  /** Move this player to the Spectator team (= `changeTeam(1)`). */
  spectate(): void;
  /**
   * NON-LETHAL team switch between Terrorist (2) and CounterTerrorist (3) via the sig-resolved
   * CCSPlayerController::SwitchTeam: the player stays alive and keeps their weapons (vs `changeTeam`,
   * which has jointeam semantics and usually kills). Works on DEAD controllers too (a pure
   * scoreboard/win-condition team move). CAVEAT: the engine MAY respawn the pawn during the call —
   * re-resolve `player.pawn` on the next frame before any pawn write. Game events the engine fires
   * inside the call run other plugins' handlers before this returns. For None (0) /
   * Spectator (1) this dispatches to `changeTeam` (CSSharp/SwiftlyS2 parity) — prefer `spectate()`.
   * Serial-gated; a no-op if the ref is stale or the signature is unresolved. Bounded 0..3.
   */
  switchTeam(team: number): void;
  /** Respawn this (dead) player via the self-resolved CCSPlayerController::Respawn (byte-sig +
   *  RTTI-vtable-membership load-validated). Synchronous: SetPawn then Respawn on this call;
   *  `Events.on` / `onPre("player_spawn")` run other plugins before this returns.
   *  `players.onRespawn` does not fire (SourceMod `blockhook` / `bypassWith`).
   *  Returns false when the player is already alive, the ref is stale, or Respawn/SetPawn failed
   *  their boot gates. */
  respawn(): boolean;
}
/**
 * Entry point for the {@link Player} controller object — resolve players by slot or user-id, enumerate
 * connected players, or resolve a SourceMod target string.
 * @example
 * import { Player } from "@s2script/cs2";
 * for (const p of Player.all()) p.pawn?.slay();
 */
export declare const Player: {
  /** The Player for a 0-based slot, or null if the slot is unoccupied / the controller is stale. */
  fromSlot(slot: number): Player | null;
  /** Every connected player (slots with a valid controller). */
  all(): Player[];
  /** Look up a connected player by engine user-id. `null` if no such player. Pawnless-safe. */
  fromUserId(userId: number): Player | null;
  /** Every connected player regardless of pawn (the pawnless enumeration). Complements `all()`. */
  allConnected(): Player[];
  /** Resolve a SourceMod target string to matching connected players. `#userid`/name/`@all`/`@me`; empty on no match. `callerSlot < 0` = server console (no `@me`). */
  target(pattern: string, callerSlot: number, filterImmunity?: boolean): Player[];
};

import type { GameEvent, HookResultValue } from "@s2script/sdk/events";
export { HookResult } from "@s2script/sdk/events";
export type { HookResultValue } from "@s2script/sdk/events";
import type { GameEvents } from "./events.generated";
/**
 * Game-event subscription (typed overlay). Importing from `@s2script/cs2` gives the typed overloads:
 * `Events.on("player_death", ev => ev.getPlayerSlot("attacker"))` typechecks via the GameEvents map.
 * `Events.onPre` runs before broadcast and may return a HookResult to block.
 * `Events.fire` fires an event with typed field constraints.
 * The `off` signature matches `@s2script/events` semantics: removes ALL of this plugin's handlers for `name`.
 */
export declare const Events: {
  /** Subscribe to a game event; the typed overload delivers the `GameEvents[K]` payload with typed field accessors. */
  on<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => void): void;
  /** Subscribe to a game event by raw name; the handler receives the generic {@link GameEvent}. */
  on(name: string, handler: (ev: GameEvent) => void): void;
  /** Remove ALL of this plugin's handlers for `name` (the `handler` arg is accepted for API symmetry but not matched). */
  off(name: string, handler: (ev: GameEvent) => void): void;
  /** Subscribe before broadcast; return a {@link HookResultValue} to block/modify. Typed payload overload. */
  onPre<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => HookResultValue | void): void;
  /** Subscribe before broadcast by raw name; return a {@link HookResultValue} to block/modify. */
  onPre(name: string, handler: (ev: GameEvent) => HookResultValue | void): void;
  /** Fire a game event with typed field constraints; `dontBroadcast` keeps it server-side (not sent to clients). Returns false if the event is unknown. */
  fire<K extends keyof GameEvents>(name: K, fields?: Record<string, number | string | boolean | bigint>, dontBroadcast?: boolean): boolean;
  /** Fire a game event by raw name; `dontBroadcast` keeps it server-side. Returns false if the event is unknown. */
  fire(name: string, fields?: Record<string, number | string | boolean | bigint>, dontBroadcast?: boolean): boolean;
};

/**
 * Show-activity helper: SourceMod's FormatActivitySource per-recipient decision.
 * For each connected recipient, call `formatSource(actorSlot, recipientSlot)` to get
 * `{ show, name }` — whether to display the action to that recipient, and under what name
 * (real name for admins / self, generic label for non-admins, per the SHOW_ACTIVITY flags).
 * `actorSlot < 0` = server console (always real "Console" label).
 */
export declare const Activity: {
  /** SourceMod FormatActivitySource: per-recipient {show, name} for an admin action by actorSlot (actorSlot < 0 = server console). */
  formatSource(actorSlot: number, recipientSlot: number): { show: boolean; name: string };
};

/**
 * CS2 chat color control bytes (values from CounterStrikeSharp's ChatColors enum). Prepend one to a chat
 * message to color it — CS2 requires a leading control byte for the message to render at all. The plugin
 * owns color (SourceMod-parity): e.g. `Chat.toAll(ChatColors.Green + "[S2S] hello")`.
 */
export declare const ChatColors: {
  /** Reset to the client's default chat color. */ readonly Default: string;
  /** White control byte. */ readonly White: string;
  /** Dark-red control byte. */ readonly DarkRed: string;
  /** Light-purple control byte. */ readonly LightPurple: string;
  /** Green control byte. */ readonly Green: string;
  /** Olive control byte. */ readonly Olive: string;
  /** Lime control byte. */ readonly Lime: string;
  /** Red control byte. */ readonly Red: string;
  /** Grey control byte. */ readonly Grey: string;
  /** Yellow control byte. */ readonly Yellow: string;
  /** Silver control byte. */ readonly Silver: string;
  /** Blue control byte. */ readonly Blue: string;
  /** Dark-blue control byte. */ readonly DarkBlue: string;
  /** Blue-grey control byte. */ readonly BlueGrey: string;
  /** Purple control byte. */ readonly Purple: string;
  /** Light-red control byte. */ readonly LightRed: string;
  /** Orange control byte. */ readonly Orange: string;
};

/**
 * Show a target-picker Center menu of connected players to `adminSlot` (the adminmenu framework's
 * shared player picker). The picked player is re-resolved via `Player.fromUserId` at select time,
 * so `onPicked` only ever receives a live target — a player who left in the meantime is skipped
 * with a chat notice to `adminSlot`, and `onPicked` is not called.
 */
export declare function pickPlayer(adminSlot: number, onPicked: (target: Player) => void): void;

/** A live CEnvBeam handle. update() moves both endpoints; remove() destroys it. */
export interface BeamHandle {
  /** The backing CEnvBeam entity ref. */
  readonly ref: EntityRef;
  /** Move both endpoints — the beam redraws from `start` to `end`. */
  update(start: Vector, end: Vector): void;
  /** Destroy the beam entity; returns false if it's already gone / the ref is stale. */
  remove(): boolean;
}
/** Draw a point-to-point beam (a CEnvBeam) from start to end. Returns a handle, or null if the entity
 *  couldn't be created. The beam is game-world-owned — call handle.remove() to clean up. */
export declare const Beam: {
  /** Draw a point-to-point beam from `start` to `end`. `opts.color` is RGBA, `opts.width` in units. Returns a {@link BeamHandle}, or null if the entity couldn't be created. */
  draw(start: Vector, end: Vector, opts?: { color?: [number, number, number, number]; width?: number }): BeamHandle | null;
};

/** A live info_particle_system handle from {@link Particles.create}. */
export interface ParticleHandle {
  /** The backing info_particle_system entity ref. */
  readonly ref: EntityRef;
  /** Start (or restart) the effect. False if the ref is stale. */
  start(): boolean;
  /** Stop emitting; live particles finish their lifetime. False if the ref is stale. */
  stop(): boolean;
  /** Replace who sees the effect (slots or players); an empty list hides it from everyone. */
  setVisibleTo(viewers: readonly (number | Player)[]): boolean;
  /** Destroy the entity; false if it's already gone. */
  remove(): boolean;
}
/** Options for {@link Particles.create}. */
export interface ParticleOptions {
  /** Orientation (pitch yaw roll). Default 0 0 0. */
  angles?: Vector | { x: number; y: number; z: number } | [number, number, number];
  /** Only these players (slots or {@link Player}s) see it — the recipient filter of SourceMod's
   *  `TE_SendToClient`. Omitted = everyone. Enforced every snapshot through `@s2script/sdk/transmit`. */
  visibleTo?: readonly (number | Player)[];
  /** Follow this entity (the `SetParent` input). */
  parent?: EntityRef;
  /** Remove the entity after this many seconds. Runs on the calling plugin's timer, so an unload
   *  before it fires leaves the entity in the world. */
  lifetime?: number;
  /** false = create stopped; call {@link ParticleHandle.start}. Default true. */
  start?: boolean;
  /** Extra info_particle_system keyvalues, e.g. control points (`cpoint1`). */
  keyvalues?: EntityKeyValueMap;
}
/**
 * Particle effects (SourceMod `TE_SetupParticle` / ModSharp `DispatchParticleEffect`), as an
 * info_particle_system entity. The effect must be precached: built-in effects are, a custom `.vpcf`
 * is added in `ctx.server.onPrecache`. The entity is game-world-owned; call `remove()` or pass
 * `lifetime`.
 * @example
 * import { Particles } from "@s2script/cs2";
 * Particles.create("particles/explosions_fx/explosion_basic.vpcf", pawn.origin!, { lifetime: 3, visibleTo: [player] });
 */
export declare const Particles: {
  /** Place `effect` at `origin`. Returns a handle, or null if the entity couldn't be created. */
  create(
    effect: string,
    origin: Vector | { x: number; y: number; z: number } | [number, number, number],
    opts?: ParticleOptions
  ): ParticleHandle | null;
};

/** A live read view over CCSGameRules (via the cs_gamerules proxy). Every field is liveness-gated at the
 *  proxy root and reads null if the proxy is gone (e.g. between maps). */
export interface GameRulesView {
  /** In warmup (m_bWarmupPeriod). */
  readonly warmupPeriod: boolean | null;
  /** In freeze time (m_bFreezePeriod) — players frozen at round start. */
  readonly freezePeriod: boolean | null;
  /** Configured round length in seconds (m_iRoundTime). */
  readonly roundTime: number | null;
  /** Configured freeze-time length in seconds (m_iFreezeTime); the first slice of the round span. */
  readonly freezeTime: number | null;
  /** Rounds played this match (m_totalRoundsPlayed). */
  readonly totalRoundsPlayed: number | null;
  /** Current game phase enum (m_gamePhase). */
  readonly gamePhase: number | null;
  /** The bomb is currently planted (m_bBombPlanted). */
  readonly bombPlanted: boolean | null;
  /** Rounds played in the current phase (m_nRoundsPlayedThisPhase). */
  readonly roundsPlayedThisPhase: number | null;
  /** A game restart is queued (m_bGameRestart). */
  readonly gameRestart: boolean | null;
  /** Match start time (m_flGameStartTime, GameTime_t). */
  readonly gameStartTime: number | null;
  /** Match is paused awaiting resume (m_bMatchWaitingForResume). */
  readonly matchWaitingForResume: boolean | null;
  /** The match has started (m_bHasMatchStarted). */
  readonly hasMatchStarted: boolean | null;
  /** m_fRoundStartTime (GameTime_t): the map-time at which the current round started. */
  readonly roundStartTime: number | null;
  /** Server.gameTime - roundStartTime — real seconds since the round started (freeze included; the
   *  engine ends the round at roundStartTime + roundTime). null pre-round / no proxy. */
  readonly timeElapsed: number | null;
  /** roundTime - timeElapsed — real seconds until the engine ends the round (matches the HUD clock). */
  readonly timeRemaining: number | null;
  /** Write m_iRoundTime and renetwork it (proxy notifyStateChanged at m_pGameRules — the HUD clock
   *  repaints on clients). Returns false if the proxy is stale or an offset fails to resolve. */
  setRoundTime(seconds: number): boolean;
  /** Set the REMAINING round time (writes roundTime = timeElapsed + seconds). */
  setTimeRemaining(seconds: number): boolean;
  /** Extend/shrink the round clock by delta seconds (writes roundTime += seconds). */
  addTimeRemaining(seconds: number): boolean;
  /** Force the round to end with a RoundEndReason (sig-resolved CCSGameRules::TerminateRound).
   *  Synchronous: the engine call runs now; `Events.on` / `onPre("round_end")` run other plugins
   *  before this returns. `gameRules.onTerminateRound` does not fire for this call
   *  (SourceMod `CS_OnTerminateRound` / `blockhook`). To cancel an *engine* round end, subscribe
   *  to `onTerminateRound`. delay (default 5s) is the engine's pre-restart delay. Returns false
   *  when degraded (unresolved signature, stale proxy, or reason outside 0..22). */
  terminateRound(reason: number, delay?: number): boolean;
  /** End the match now and go to the scoreboard and map vote (CGameRules::GoToIntermission); the
   *  engine changes level after `mp_match_restart_delay`. `abortedMatch` marks the match as abandoned
   *  rather than finished. Returns false when degraded (unresolved slot or stale proxy). */
  goToIntermission(abortedMatch?: boolean): boolean;
}
/** Read + drive CCSGameRules state. get() re-finds the cs_gamerules proxy each call (liveness-gated
 *  cache); returns null when no proxy exists (e.g. pre-map-load). */
export declare const GameRules: {
  /** Re-find the cs_gamerules proxy and return a live {@link GameRulesView}, or null when no proxy exists (e.g. pre-map-load). */
  get(): GameRulesView | null;
  /** Convenience over get()?.terminateRound(reason, delay) — false when no proxy. */
  terminateRound(reason: number, delay?: number): boolean;
  /** Convenience over get()?.goToIntermission(abortedMatch) — false when no proxy. */
  goToIntermission(abortedMatch?: boolean): boolean;
  /** Run `handler` at the start of every round (and on `mp_restartgame`), before the engine resets
   *  the round (CCSGameRules::RestartRound). Every handler runs and the round always restarts.
   *  Subscribe during OnPluginStart; unsubscribed at unload. */
  onRoundRestart(handler: () => void): void;
  /** As {@link onRoundRestart}, after the engine has reset the round and respawned players. */
  onRoundRestartPost(handler: () => void): void;
};

/** The grenade kinds {@link Grenades.emit} can spawn. */
export type GrenadeKind = "hegrenade" | "flashbang" | "smokegrenade" | "molotov" | "decoy";
/** A vector as a `Vector`, any `{x, y, z}`, or an `[x, y, z]` triple. */
export type GrenadeVector = { x: number; y: number; z: number } | readonly [number, number, number];
/** Where and how {@link Grenades.emit} throws the projectile. */
export interface GrenadeOptions {
  /** World position the projectile starts at. */
  origin: GrenadeVector;
  /** Initial velocity in units per second (default zero: it drops). */
  velocity?: GrenadeVector;
  /** Initial orientation (default zero). */
  angles?: GrenadeVector;
  /** Initial spin in degrees per second (default zero). */
  angularVelocity?: GrenadeVector;
  /** Who threw it, for kill credit and flash/smoke team rules. A Player is mapped to its pawn.
   *  Default: no owner. */
  owner?: Pawn | Player | null;
  /** Smoke only: the team whose colour the smoke takes (2 = T, 3 = CT). Default: the owner's team. */
  team?: number;
}
/** Spawn live grenades (Swiftly `EmitGrenade`). */
export declare const Grenades: {
  /** Spawn an already-thrown `kind` projectile and return it. Null when the engine function is
   *  unavailable on this build, the options are not finite numbers, the owner is not a live pawn,
   *  or the kind is unknown. */
  emit(kind: GrenadeKind, opts: GrenadeOptions): EntityRef | null;
};

/** Team scoreboard scores (cs_team_manager entities, CTeam.m_iScore + notifyStateChanged). team is
 *  0..3 (Unassigned/Spectator/T/CT), matched by m_iTeamNum; entities are re-found per call. */
export declare const Teams: {
  /** Read the scoreboard score for `team` (0..3), or null if no matching cs_team_manager entity. */
  getScore(team: number): number | null;
  /** Overwrite the scoreboard score for `team` (writes m_iScore + notifyStateChanged). false if the entity isn't found. */
  setScore(team: number, score: number): boolean;
  /** Add `delta` to `team`'s scoreboard score. false if the entity isn't found. */
  addScore(team: number, delta: number): boolean;
  /** Decide a player's team request (CCSPlayerController::HandleCommandJoinTeam): the jointeam
   *  command and the team menu. Return `HookResult.Handled` or `Stop` to refuse it, so the player
   *  stays on their team; `Stop` also skips later handlers. Plugin `changeTeam`/`switchTeam` calls do
   *  not come through here. Subscribe during OnPluginStart; unsubscribed at unload. */
  onJoinTeam(handler: (view: JoinTeamView) => HookResultValue | void): void;
};
/** A team request, valid during the {@link Teams.onJoinTeam} handler. */
export interface JoinTeamView {
  /** Who asked, or null when the controller could not be adopted. */
  readonly player: Player | null;
  /** The team asked for: 0 = auto-select, 1 = spectator, 2 = T, 3 = CT. */
  readonly team: number;
  /** The request asked to queue for the team instead of joining now. */
  readonly queue: boolean;
}

/** CS2 round-end reasons (CCSGameRules::TerminateRound / round_end.reason). Binary-validated against
 *  our build (reason bound = 22; #SFUI_Notice_* switch). Gaps 2/3/15 are removed legacy VIP reasons. */
export declare const RoundEndReason: {
  /** No/unknown reason. */ readonly Unknown: 0;
  /** T's bombed the target. */ readonly TargetBombed: 1;
  /** T's escaped (legacy es_ maps). */ readonly TerroristsEscaped: 4;
  /** CT's prevented the escape. */ readonly CTsPreventEscape: 5;
  /** Escaping T's were neutralized. */ readonly EscapingTerroristsNeutralized: 6;
  /** The bomb was defused. */ readonly BombDefused: 7;
  /** CT's win the round. */ readonly CTsWin: 8;
  /** T's win the round. */ readonly TerroristsWin: 9;
  /** The round was a draw. */ readonly RoundDraw: 10;
  /** All hostages were rescued. */ readonly AllHostagesRescued: 11;
  /** The VIP target was saved. */ readonly TargetSaved: 12;
  /** Hostages were not rescued in time. */ readonly HostagesNotRescued: 13;
  /** T's did not escape in time. */ readonly TerroristsNotEscaped: 14;
  /** The game is (re)commencing / warmup. */ readonly GameCommencing: 16;
  /** T's surrendered. */ readonly TerroristsSurrender: 17;
  /** CT's surrendered. */ readonly CTsSurrender: 18;
  /** T's planted the bomb (round-end variant). */ readonly TerroristsPlanted: 19;
  /** CT's reached the hostages. */ readonly CTsReachedHostage: 20;
  /** Survival-mode win. */ readonly SurvivalWin: 21;
  /** Survival-mode draw. */ readonly SurvivalDraw: 22;
};

/** cs_win_panel_round final_event values (validated at the live gate against a natural round end). */
export declare const WinPanelFinalEvent: {
  /** CT's won (final_event = 2). */ readonly CTsWin: 2;
  /** T's won (final_event = 3). */ readonly TerroristsWin: 3;
};

/** Screen-fade user message (CUserMessageFade). duration/holdTime are engine fade units; color is a
 *  packed RGBA fixed32. Returns false if the message/fields don't resolve. */
export declare const Fade: {
  /** Send a screen fade to `slot`. `duration`/`holdTime` are engine fade units; `color` is packed RGBA fixed32. false if unresolved. */
  to(slot: number, opts: { duration?: number; holdTime?: number; color?: number; flags?: number }): boolean;
  /** Convenience full-screen blind (opaque fade) for `duration` — the canonical flashbang-style effect. */
  blind(slot: number, duration?: number): boolean;
};
/** Screen-shake user message (CUserMessageShake). command 0 = start. Returns false if unresolved. */
export declare const Shake: {
  /** Send a screen shake to `slot`. `command` 0 = start; `amplitude`/`frequency`/`duration` are engine shake units. false if unresolved. */
  to(slot: number, opts: { command?: number; amplitude?: number; frequency?: number; duration?: number }): boolean;
};
/** Best-effort hint text (TextMsg-family). Returns false if the message/fields don't resolve. */
export declare const HintText: {
  /** Show `text` as hint text to `slot`. false if the message/fields don't resolve. */
  to(slot: number, text: string): boolean;
};

/** Center-screen text (SourceMod `PrintCenterText` / `PrintCenterTextAll`), a `TextMsg` with
 *  `HUD_PRINTCENTER`. false if the message does not resolve or `slot` has no client. */
export declare const CenterText: {
  /** Show `text` in the center of `slot`'s screen. */
  to(slot: number, text: string): boolean;
  /** Show `text` in the center of every client's screen. */
  toAll(text: string): boolean;
};
/** CS2's alert line above the center text (`TextMsg` with `HUD_PRINTALERT`). Same returns as
 *  {@link CenterText}. */
export declare const AlertText: {
  /** Show `text` as an alert to `slot`. */
  to(slot: number, text: string): boolean;
  /** Show `text` as an alert to every client. */
  toAll(text: string): boolean;
};
/**
 * The HTML center panel (`show_survival_respawn_status`; other frameworks call this
 * `PrintCenterHtml` / `SendCenterHTML`). The client paints it one frame per event, so the package
 * re-sends it every frame until `seconds` (default 5) run out. Each slot shows one message per
 * plugin; a later call replaces it. The re-send stops at the calling plugin's unload.
 * Escape any player-supplied text before putting it in `html`.
 */
export declare const CenterHtml: {
  /** Show `html` to `slot` for `seconds` (default 5; 0 or less clears it). Dead and spectating players
   *  see it too. false if `slot` has no connected human client. */
  to(slot: number, html: string, seconds?: number): boolean;
  /** Show `html` to every connected human client for `seconds`. true if at least one got it. */
  toAll(html: string, seconds?: number): boolean;
  /** Stop showing this plugin's message to `slot`. false if none was showing. */
  clear(slot: number): boolean;
};

/** A world-space coordinate triple (a corner or center of a {@link TriggerZoneHandle} box). */
export interface ZoneBox {
  /** World X. */ x: number;
  /** World Y. */ y: number;
  /** World Z. */ z: number;
}
/** A live runtime trigger_multiple zone (from {@link TriggerZone.create}). */
export interface TriggerZoneHandle {
  /** The backing trigger_multiple entity ref. */ ref: EntityRef;
  /** The box center in world space. */ center: ZoneBox;
  /** Destroy the trigger entity; false if already gone/stale. */ remove(): boolean;
}
/** Create arbitrary-box touch zones at runtime (a runtime trigger_multiple). */
export declare const TriggerZone: {
  /** Create a runtime engine trigger_multiple whose touch volume is the arbitrary box [min,max].
   *  Fires OnStartTouch/OnEndTouch (hook via `onOutput` from `@s2script/sdk`). Non-solid (pass-through). */
  create(min: ZoneBox, max: ZoneBox, opts?: { model?: string; spawnflags?: number }): TriggerZoneHandle | null;
};

/** Curated built-in CS2 soundevent names (see @s2script/sound `Sound.emit` / `Pawn.emitSound`). */
export declare const Sounds: {
  /** Player ping soundevent (`UI.PlayerPing`). */
  readonly Ping: string;
  /** Urgent player ping soundevent (`UI.PlayerPingUrgent`). */
  readonly PingUrgent: string;
  /** AK-47 single-shot soundevent (`Weapon_AK47.Single`). */
  readonly Ak47Shot: string;
  /** Desert Eagle single-shot soundevent (`Weapon_DEagle.Single`). */
  readonly DeagleShot: string;
};
