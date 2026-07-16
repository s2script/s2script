/**
 * @s2script/math — author-time type stubs for the injected math value types.
 * NO runtime code: the engine injects the implementation (core prelude) at load time.
 */

/** A 3-component vector value (a copied snapshot; never a live pointer). */
export declare class Vector {
  x: number;
  y: number;
  z: number;
  constructor(x: number, y: number, z: number);
  /** Euclidean magnitude — e.g. speed from a velocity vector. */
  length(): number;
  toString(): string;
}

/** A Source 2 Euler angle value (x=pitch, y=yaw, z=roll), a copied snapshot. */
export declare class QAngle {
  x: number;
  y: number;
  z: number;
  constructor(x: number, y: number, z: number);
  toString(): string;
}

/** The unit forward-direction vector for a Euler angle (pitch=`a.x`, yaw=`a.y`; ignores roll). */
export declare function forwardVector(a: QAngle): Vector;
