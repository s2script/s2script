/**
 * @s2script/trace — author-time type stubs for the injected ray-tracing API.
 * NO runtime code: the engine injects the implementation (core prelude) at load time.
 *
 * ENGINE-GENERIC (Source-2 physics) — over `CNavPhysicsInterface::TraceShape`, RTTI-resolved
 * shim-side. Degrades to a MISS `TraceHit` (`didHit:false, fraction:1, entity:null`) when the
 * underlying vtable can't be resolved (a different Source 2 game build, or in-isolate tests) —
 * never a crash.
 */
import type { Vector, QAngle } from "./math";
import type { EntityRef } from "./entity";

/** Named `InteractionLayers` bitmasks for `TraceOptions.mask`/`exclude` (bitwise-OR combinable). */
export declare const TraceMask: {
  /** World + player-clip + windows + players/NPCs/physics-props — a bullet trace (the default). */
  readonly ShotPhysics: number;
  /** Hitboxes only (headshot-style detection). */
  readonly ShotHitbox: number;
  /** Physics + hitboxes (a full bullet trace: world geometry AND hitbox precision). */
  readonly ShotFull: number;
  /** World geometry only, no entities at all. */
  readonly WorldOnly: number;
  /** Grenade trajectory trace (world + physics props, no players). */
  readonly Grenade: number;
  /** Brushes only (no clip volumes, no entities). */
  readonly BrushOnly: number;
  /** Player movement collision (world + player-clip). */
  readonly PlayerMove: number;
  /** NPC movement collision (world + npc-clip). */
  readonly NPCMove: number;
};

/** The result of a `Trace.line`/`ray`/`hull` call. */
export interface TraceHit {
  /** True iff the trace hit something before reaching `endPos`. */
  didHit: boolean;
  /** 0..1 — the fraction of the start->end segment traveled before the hit (1 = no hit). */
  fraction: number;
  /** The trace's final position (the hit point, or the requested end on a miss). */
  endPos: Vector;
  /** The surface normal at the hit point ((0,0,0) on a miss). */
  normal: Vector;
  /** The hit entity, or null if none was hit (or it went stale in the same frame). */
  entity: EntityRef | null;
  /** True iff the trace STARTED embedded in solid geometry (Source `startsolid`; CS2's CGameTrace
   *  exposes only `m_bStartInSolid`, not a whole-segment `allsolid` flag — don't confuse with SM's TR_AllSolid). */
  startSolid: boolean;
}

/** Options shared by `Trace.line`/`ray`/`hull`. */
export interface TraceOptions {
  /** `InteractionLayers` bits the trace should interact with. Default `TraceMask.ShotPhysics`. */
  mask?: number;
  /** `InteractionLayers` bits to explicitly exclude. Default 0 (none). */
  exclude?: number;
  /** An entity to ignore (e.g. the tracing pawn itself, for `pawn.aimTrace`'s self-ignore). */
  ignoreEntity?: EntityRef;
}

export declare const Trace: {
  /** A line trace from `start` to `end`. */
  line(start: Vector, end: Vector, opts?: TraceOptions): TraceHit;
  /** A line trace from `start` along `angles`' forward direction, `distance` units long. */
  ray(start: Vector, angles: QAngle, distance: number, opts?: TraceOptions): TraceHit;
  /** A swept-box (hull) trace from `start` to `end` with the given `mins`/`maxs` extents. */
  hull(start: Vector, end: Vector, mins: Vector, maxs: Vector, opts?: TraceOptions): TraceHit;
};
