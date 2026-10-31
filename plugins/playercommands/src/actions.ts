import type { CommandInvocation, EntityRef, PhraseKey } from "@s2script/sdk";
import { CsItem, Player } from "@s2script/cs2";
import type { Pawn } from "@s2script/cs2";

export function sameRef(a: EntityRef, b: EntityRef): boolean {
  return a.index === b.index && a.id === b.id;
}

// allConnected includes controllers without a pawn, including spectators and dead players.
export function connectedPlayer(slot: number): Player | null {
  return Player.allConnected().find(p => p.slot === slot) ?? null;
}

export function replyFor(cmd: CommandInvocation): (key: PhraseKey, ...args: (string | number)[]) => void {
  const actor = cmd.callerSlot < 0 ? null : connectedPlayer(cmd.callerSlot);
  return (key, ...args) => {
    if (cmd.callerSlot < 0 || actor?.ref.isValid()) cmd.replyT(key, ...args);
  };
}

export function livePawn(player: Player): Pawn | null {
  if (!player.ref.isValid()) return null;
  const pawn = player.pawn;
  return pawn?.isValid && pawn.health !== null && pawn.health > 0 ? pawn : null;
}

export function stillOwnsPawn(player: Player, pawn: Pawn): boolean {
  return player.ref.isValid() && pawn.isValid && !!player.pawn && sameRef(player.pawn.ref, pawn.ref);
}

export function integer(token: string, min: number, max: number): number | null {
  if (!/^[+-]?\d+$/.test(token)) return null;
  const value = Number(token);
  return Number.isSafeInteger(value) && value >= min && value <= max ? value : null;
}

const items = new Map<string, string>();
for (const [name, value] of Object.entries(CsItem)) {
  items.set(name.toLowerCase(), value);
  items.set(value.toLowerCase(), value);
  items.set(value.replace(/^(weapon|item)_/, "").toLowerCase(), value);
}

export function resolveItem(input: string): string | null {
  return items.get(input.toLowerCase()) ?? null;
}

// Give first: neither a typo nor failed/degraded creation is allowed to strip inventory.
// Snapshot only original weapons and use the created weapon's VData gear slot (not m_iSlot).
export function giveItem(player: Player, item: string, equip: boolean): boolean {
  const pawn = livePawn(player);
  if (!pawn) return false;
  const previous = equip ? [...pawn.weapons] : [];
  const given = pawn.giveNamedItem(item);
  if (!given?.isValid() || !stillOwnsPawn(player, pawn)) return false;
  if (!equip) return true;
  const hasGiven = () => given.isValid() && stillOwnsPawn(player, pawn)
    && pawn.weapons.some(w => sameRef(w.ref, given.ref));
  if (!hasGiven()) return false;
  const gearSlot = given.data?.gearSlot;
  if (gearSlot === null || gearSlot === undefined || !Number.isInteger(gearSlot) || gearSlot < 0) {
    // No VData means replacement cannot be verified. Undo only our newly created held item.
    if (hasGiven()) pawn.removeWeapon(given);
    return false;
  }
  let removed = true;
  for (const old of previous) {
    if (!hasGiven()) return false; // Native calls can run other plugins synchronously.
    if (!old.isValid() || sameRef(old.ref, given.ref) || old.data?.gearSlot !== gearSlot) continue;
    if (!pawn.weapons.some(w => sameRef(w.ref, old.ref))) continue;
    if (!pawn.removeWeapon(old)) removed = false;
  }
  return removed && hasGiven();
}
