/**
 * @s2script/math — author-time type stubs for the injected math value types.
 * NO runtime code: the engine injects the implementation (core prelude) at load time.
 */

/**
 * A 3-component vector value (a copied snapshot; never a live pointer).
 * @example
 * import { Vector } from "@s2script/sdk/math";
 * const start = new Vector(0, 0, 100);
 * console.log(start.length());
 */
export declare class Vector {
  /** The X component. */
  x: number;
  /** The Y component. */
  y: number;
  /** The Z component. */
  z: number;
  constructor(x: number, y: number, z: number);
  /** Euclidean magnitude — e.g. speed from a velocity vector. */
  length(): number;
  /** Format as `"Vector(x, y, z)"` (for logging). */
  toString(): string;
}

/** A Source 2 Euler angle value (x=pitch, y=yaw, z=roll), a copied snapshot. */
export declare class QAngle {
  /** Pitch, in degrees. */
  x: number;
  /** Yaw, in degrees. */
  y: number;
  /** Roll, in degrees. */
  z: number;
  constructor(x: number, y: number, z: number);
  /** Format as `"QAngle(x, y, z)"` (for logging). */
  toString(): string;
}

/** The unit forward-direction vector for a Euler angle (pitch=`a.x`, yaw=`a.y`; ignores roll). */
export declare function forwardVector(a: QAngle): Vector;
