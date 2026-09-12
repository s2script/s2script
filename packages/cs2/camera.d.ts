import type { EntityRef } from "@s2script/sdk/entity";
import type { Pawn } from "./index";

/** CS2's experimental camera modes. Requires the September 9, 2026 update. */
export declare const CustomCameraMode: {
  readonly DISABLED: 0;
  readonly CONTROLLED: 1;
  readonly CONTROLLED_POSITION: 2;
  readonly FOLLOW_POSITION: 3;
};
export type CustomCameraModeValue = typeof CustomCameraMode[keyof typeof CustomCameraMode];

export interface CameraFollowConfig {
  readonly followEntity: EntityRef;
  /** Follow eyes instead of origin. Default false. */
  readonly followEyes?: boolean;
  /** Offset from the followed origin/eyes. Defaults to zero. */
  readonly followOffset?: { x: number; y: number; z: number };
  /** Offset rotated by player eye angles: forward/left/up. Defaults to zero. */
  readonly cameraOffset?: { x: number; y: number; z: number };
  /** Pull the camera offset inward to avoid solids. Default false. */
  readonly clipCameraOffset?: boolean;
  /** Return strength after clipping. Default 1 (instant). */
  readonly cameraOffsetReturnStrength?: number;
}

/**
 * Borrowed, pawn-owned engine camera. Acquire with Pawn.getCustomCamera().
 * Shared by all scripts controlling that pawn; the latest mutation wins.
 * Disable explicitly when finished; plugin unload does not undo engine state.
 * Do not remove the entity: the engine owns its association with the pawn.
 * Experimental, matching Valve's API. All accesses are entity-liveness gated.
 */
export declare class CustomPlayerCamera {
  private constructor();
  /** Use ref.teleport(position, angles) to position a controlled camera. */
  readonly ref: EntityRef;
  isValid(): boolean;
  getPlayer(): Pawn | null;
  getMode(): CustomCameraModeValue | null;
  /** False if stale, unavailable, invalid mode, or native invocation rejected. */
  setMode(mode: CustomCameraModeValue): boolean;
  /** Configure before selecting FOLLOW_POSITION. False on invalid/stale input or unavailable native. */
  setFollowConfig(config: CameraFollowConfig): boolean;
}
