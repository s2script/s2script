/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API. NO runtime code.
 * The typed field accessors are GENERATED (schema.generated.d.ts) from the schema catalog by
 * `s2script gen-schema`; the typed nav wrappers are GENERATED (nav.generated.d.ts) from
 * nav-targets.json by `s2script gen-nav`; the typed event interfaces are GENERATED (events.generated.d.ts)
 * from the event catalog by `s2script gen-events`; this file adds the hand-written entry points on top.
 */
import type { EntityRef } from "@s2script/entity";
import type { Vector, QAngle } from "@s2script/math";
export * from "./schema.generated";
import type { CCSPlayerPawn, CCSPlayerController } from "./schema.generated";
export type { SceneNode, WeaponServices, MovementServices, AimPunchServices } from "./nav.generated";
import type { SceneNode, WeaponServices, MovementServices, AimPunchServices } from "./nav.generated";
export { GameEvent } from "@s2script/events";
export type { GameEvents } from "./events.generated";

/**
 * A CS2 player pawn (the in-world body): the generated CCSPlayerPawn schema fields + the serial-gated ref.
 * `controller` is the typed reverse hop (shadows the raw generated m_hController handle).
 * Nav props (sceneNode, weaponServices, movementServices, aimPunchServices) are generated from nav-targets.json.
 */
export interface Pawn extends Omit<CCSPlayerPawn, "controller"> {
  readonly ref: EntityRef;
  /** The player controlling this pawn, or null if stale/absent. */
  readonly controller: Player | null;
  /** World-space position (via the CGameSceneNode pointer chain), or null if stale. */
  readonly origin: Vector | null;
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
  /** The pawn's MoveType_t (uint8; null on a stale ref). Setting writes both m_MoveType and
   *  m_nActualMoveType + notifies. Values (MoveType_t): NONE=0, WALK=2, NOCLIP=7. */
  moveType: number | null;
  /** Kill this pawn via the sig-resolved CommitSuicide engine op (serial-gated; no-op if stale). */
  slay(): void;
}
export declare const Pawn: {
  /** The Pawn for a player slot, or null if unoccupied / invalidated. */
  forSlot(slot: number): Pawn | null;
};

/**
 * A CS2 player (the persistent controller entity): the generated CCSPlayerController schema fields
 * (team/score/ping/…) + the serial-gated controller ref. `pawn` is the typed body (shadows the raw
 * generated m_hPawn handle). Referenced by slot (0-based); a stored Player degrades to null on reuse.
 */
export interface Player extends Omit<CCSPlayerController, "pawn"> {
  readonly ref: EntityRef;
  /** The 0-based player slot (CPlayerSlot). */
  readonly slot: number;
  /** This player's in-world pawn (the body), or null if dead/absent. */
  readonly pawn: Pawn | null;
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
}
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
  target(pattern: string, callerSlot: number): Player[];
};

import type { GameEvent, HookResultValue } from "@s2script/events";
export { HookResult } from "@s2script/events";
export type { HookResultValue } from "@s2script/events";
import type { GameEvents } from "./events.generated";
/**
 * Game-event subscription (typed overlay). Importing from `@s2script/cs2` gives the typed overloads:
 * `Events.on("player_death", ev => ev.getPlayerSlot("attacker"))` typechecks via the GameEvents map.
 * `Events.onPre` runs before broadcast and may return a HookResult to block.
 * `Events.fire` fires an event with typed field constraints.
 * The `off` signature matches `@s2script/events` semantics: removes ALL of this plugin's handlers for `name`.
 */
export declare const Events: {
  on<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => void): void;
  on(name: string, handler: (ev: GameEvent) => void): void;
  off(name: string, handler: (ev: GameEvent) => void): void;
  onPre<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => HookResultValue | void): void;
  onPre(name: string, handler: (ev: GameEvent) => HookResultValue | void): void;
  fire<K extends keyof GameEvents>(name: K, fields?: Record<string, number | string | boolean | bigint>, dontBroadcast?: boolean): boolean;
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
 * owns color (SourceMod-parity): e.g. `Chat.toAll(ChatColors.Green + "[SM] hello")`.
 */
export declare const ChatColors: {
  readonly Default: string; readonly White: string; readonly DarkRed: string; readonly LightPurple: string;
  readonly Green: string; readonly Olive: string; readonly Lime: string; readonly Red: string;
  readonly Grey: string; readonly Yellow: string; readonly Silver: string; readonly Blue: string;
  readonly DarkBlue: string; readonly BlueGrey: string; readonly Purple: string; readonly LightRed: string;
  readonly Orange: string;
};
