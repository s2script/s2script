/**
 * The `cs_player_camera` entity added by the same update.
 *
 * SCOPE LIMIT, stated up front: no schema dump for `CCSPlayerCamera` was available when this was
 * written — the offsets in `src/offsets.ts` cover the HUD classes only. So unlike hudstate.ts,
 * nothing here reads or writes a field. This module can do exactly two things:
 *
 *   1. create + spawn the entity, which proves the class exists in this build; and
 *   2. fire entity-IO inputs at it and report whether the engine accepted them.
 *
 * (2) is a PROBE, not an API. `acceptInput` queues onto the engine's I/O pump and returns whether
 * the event was queued — NOT whether the target actually has that input. An unknown input name is
 * dropped later, silently. So a `true` here means "queued", never "worked"; the only real evidence
 * is an observable effect in-game. The input names below are the Source convention for a camera
 * (`point_viewcontrol` uses Enable/Disable) and are guesses for this class.
 *
 * To turn this into a real API: dump CCSPlayerCamera, add its offsets alongside the HUD ones, and
 * read/write `m_bIsEnabled` / the angle-control flag directly the way hudstate.ts does — plus find
 * the pawn's camera handle field to implement `CSPlayerPawn.GetCamera`.
 */
import { createEntity, Entity } from "@s2script/sdk";
import type { EntityRef } from "@s2script/sdk";

export const CAMERA_CLASS = "cs_player_camera";
export const OWNED_TARGETNAME = "s2_hudlab_camera";

/** Every cs_player_camera in the world. */
export function findAll(): EntityRef[] {
  return Entity.findByClass(CAMERA_CLASS);
}

/** Only cameras this plugin created. */
export function findOwned(): EntityRef[] {
  return findAll().filter((e) => e.name === OWNED_TARGETNAME);
}

export interface CreateResult {
  ref: EntityRef | null;
  error: string | null;
}

/**
 * Create + spawn a camera at `origin` (an [x,y,z] triple), looking along `angles`.
 *
 * Spawning at the caller's own position is what makes the probe legible: if the camera works, the
 * view snaps to roughly where the caller already is, so "nothing happened" and "it worked" are
 * distinguishable only after you then move — which is the point of `setControllingAngles`.
 */
export function create(origin: number[], angles: number[]): CreateResult {
  const ref = createEntity(CAMERA_CLASS, {
    targetname: OWNED_TARGETNAME,
    origin: `${origin[0]} ${origin[1]} ${origin[2]}`,
    angles: `${angles[0]} ${angles[1]} ${angles[2]}`,
  });
  if (!ref) {
    return {
      ref: null,
      error:
        `createEntity("${CAMERA_CLASS}") returned null — the class is unknown to this build, or ` +
        `DispatchSpawn rejected the keyvalues`,
    };
  }
  return { ref, error: null };
}

/** The I/O names this module probes, so the status board can list them without duplicating strings. */
export const PROBED_INPUTS = {
  enable: "Enable",
  disable: "Disable",
  /** cs_script exposes `SetIsControllingAngles`; whether an identically-named INPUT exists is unknown. */
  controlAngles: "SetIsControllingAngles",
} as const;

/**
 * Fire `Enable`/`Disable` at a camera, with the activator threaded through as the player — a
 * player-scoped camera almost certainly needs to know WHICH player it is enabling for, and the
 * activator is the only channel entity-IO has for that.
 *
 * Returns whether the event was QUEUED. See the module header for why that is not success.
 */
export function setEnabled(camera: EntityRef, on: boolean, activator: EntityRef): boolean {
  const input = on ? PROBED_INPUTS.enable : PROBED_INPUTS.disable;
  return camera.acceptInput(input, undefined, activator, activator);
}

/** Probe the angle-control input. `value` is passed as the input's string argument ("0"/"1"). */
export function setControllingAngles(camera: EntityRef, on: boolean, activator: EntityRef): boolean {
  return camera.acceptInput(PROBED_INPUTS.controlAngles, on ? "1" : "0", activator, activator);
}

/** Remove every camera this plugin created. Returns how many went away. */
export function removeOwned(): number {
  let n = 0;
  for (const e of findOwned()) if (e.remove()) n++;
  return n;
}
