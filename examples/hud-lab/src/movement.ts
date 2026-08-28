/**
 * `Entity.GetMoveType` / `SetMoveType` / the new `CSMoveType` enum, and `Entity.Move`.
 *
 * All of this is already reachable: `MoveType_t` is in the generated schema
 * (packages/cs2/schema.generated.d.ts), and `Pawn.moveType` is a read/write accessor that writes
 * BOTH `m_MoveType` and `m_nActualMoveType` and notifies — which is what makes it stick. cs_script's
 * new `CSMoveType` is the same enum under a different name, so the only thing worth testing here is
 * that the values still line up after the update.
 *
 * `Entity.Move` has no direct equivalent and is not guessed at: `EntityRef.teleport` sets position
 * absolutely, `applyAbsVelocityImpulse` adds physics-aware velocity, and which of those cs_script
 * means by "Move" is not something the patch notes settle. Both are exercised so you can compare.
 */
import { MoveType_t } from "@s2script/cs2";
import type { Pawn } from "@s2script/cs2";

/** Reverse lookup so a raw uint8 can be reported by name. */
const NAMES = new Map<number, string>(
  Object.entries(MoveType_t).map(([k, v]) => [v as number, k]),
);

/** Human name for a movetype value, or a bare number when the update added one we do not know. */
export function moveTypeName(value: number | null): string {
  if (value === null) return "unreadable";
  const name = NAMES.get(value);
  return name ? `${name} (${value})` : `UNKNOWN (${value}) — new in this build?`;
}

/** The subset worth cycling through by hand; the rest are engine-internal or unsafe on a pawn. */
export const TESTABLE: ReadonlyArray<{ name: string; value: number }> = [
  { name: "NONE", value: MoveType_t.NONE },
  { name: "WALK", value: MoveType_t.WALK },
  { name: "FLY", value: MoveType_t.FLY },
  { name: "FLYGRAVITY", value: MoveType_t.FLYGRAVITY },
  { name: "NOCLIP", value: MoveType_t.NOCLIP },
  { name: "LADDER", value: MoveType_t.LADDER },
  { name: "OBSERVER", value: MoveType_t.OBSERVER },
];

/** Resolve a user-typed movetype ("noclip", "7") to a value, or null when it matches nothing. */
export function parseMoveType(token: string): number | null {
  if (!token) return null;
  const asNumber = Number(token);
  if (Number.isInteger(asNumber) && asNumber >= 0 && asNumber <= MoveType_t.LAST) return asNumber;
  const hit = TESTABLE.find((t) => t.name.toLowerCase() === token.toLowerCase());
  return hit ? hit.value : null;
}

/** Read a pawn's movetype — `Entity.GetMoveType`. */
export function get(pawn: Pawn): number | null {
  return pawn.moveType;
}

/**
 * Write a pawn's movetype — `Entity.SetMoveType`.
 *
 * Reads back rather than trusting the write: the accessor maintains two fields and the engine
 * re-derives movetype during simulation, so a value set in the wrong frame phase can be silently
 * overwritten before it is ever networked.
 */
export function set(pawn: Pawn, value: number): { ok: boolean; readBack: number | null } {
  pawn.moveType = value;
  const readBack = pawn.moveType;
  return { ok: readBack === value, readBack };
}

/** `Entity.Move`, read as "teleport": set position absolutely, leaving angles and velocity alone. */
export function moveTo(pawn: Pawn, origin: number[]): boolean {
  return pawn.ref.teleport(origin, null, null);
}

/** `Entity.Move`, read as "nudge": physics-aware additive velocity. */
export function nudge(pawn: Pawn, impulse: number[]): boolean {
  return pawn.applyAbsVelocityImpulse(impulse);
}
