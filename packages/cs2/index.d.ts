/**
 * @s2script/cs2 — author-time type stubs for the injected CS2 game API. NO runtime code.
 * The typed field accessors are GENERATED (schema.generated.d.ts) from the schema catalog by
 * `s2script gen-schema`; the typed event interfaces are GENERATED (events.generated.d.ts) from
 * the event catalog by `s2script gen-events`; this file adds the hand-written entry points on top.
 */
import type { EntityRef } from "@s2script/entity";
import type { Vector, QAngle } from "@s2script/math";
export * from "./schema.generated";
import type { CCSPlayerPawn, CCSPlayerController } from "./schema.generated";
export { GameEvent } from "@s2script/events";
export type { GameEvents } from "./events.generated";

/**
 * A CS2 player pawn (the in-world body): the generated CCSPlayerPawn schema fields + the serial-gated ref.
 * `controller` is the typed reverse hop (shadows the raw generated m_hController handle).
 */
export interface Pawn extends Omit<CCSPlayerPawn, "controller"> {
  readonly ref: EntityRef;
  /** The player controlling this pawn, or null if stale/absent. */
  readonly controller: Player | null;
  /** World-space position (via the CGameSceneNode pointer chain), or null if stale. */
  readonly origin: Vector | null;
  /** Body world rotation (via the CGameSceneNode pointer chain); distinct from the view/aim `eyeAngles`. */
  readonly angles: QAngle | null;
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
}
export declare const Player: {
  /** The Player for a 0-based slot, or null if the slot is unoccupied / the controller is stale. */
  fromSlot(slot: number): Player | null;
  /** Every connected player (slots with a valid controller). */
  all(): Player[];
};

import type { GameEvent } from "@s2script/events";
import type { GameEvents } from "./events.generated";
/**
 * Game-event subscription (typed overlay). Importing from `@s2script/cs2` gives the typed overload:
 * `Events.on("player_death", ev => ev.getPlayerSlot("attacker"))` typechecks via the GameEvents map.
 * The `off` signature matches `@s2script/events` semantics: removes ALL of this plugin's handlers for `name`.
 */
export declare const Events: {
  on<K extends keyof GameEvents>(name: K, handler: (ev: GameEvents[K]) => void): void;
  on(name: string, handler: (ev: GameEvent) => void): void;
  off(name: string, handler: (ev: GameEvent) => void): void;
};
