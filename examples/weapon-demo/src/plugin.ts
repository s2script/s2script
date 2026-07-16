// Live-gate demo for the Weapon entity object + pawn fire control (CS2). Like items-demo, this
// acts on every live pawn (Pawn.forSlot) rather than SM target resolution, since the client-list
// offsets are stale on the current build (Player.target/allConnected return 0).
import { Commands } from "@s2script/sdk/commands";
import { Pawn } from "@s2script/cs2";
import { Server } from "@s2script/sdk/server";

const MAX_SLOTS = 12;

function livePawns(): Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> {
  const out: Array<{ slot: number; pawn: NonNullable<ReturnType<typeof Pawn.forSlot>> }> = [];
  for (let slot = 0; slot < MAX_SLOTS; slot++) {
    const pawn = Pawn.forSlot(slot);
    if (pawn) out.push({ slot, pawn });
  }
  return out;
}

Commands.register("sm_wpn", (ctx) => {
  for (const { slot, pawn } of livePawns()) {
    const w = pawn.activeWeapon;
    const active = w ? "ref#" + w.ref.index + " clip1=" + w.clip1 + "/" + w.clip2 + " paint=" + w.paintKit : "none";
    ctx.reply("[wpn] slot=" + slot + " active=" + active + " count=" + pawn.weapons.length);
  }
});

Commands.register("sm_refill", (ctx) => {
  let n = 0;
  for (const { pawn } of livePawns()) {
    const w = pawn.activeWeapon;
    if (w && w.setAmmo(90)) n++;
  }
  ctx.reply("[wpn] refilled clip1=90 on " + n + " active weapon(s)");
});

Commands.register("sm_disarm", (ctx) => {
  let n = 0;
  for (const { pawn } of livePawns()) { if (pawn.disarm()) n++; }
  ctx.reply("[wpn] disarmed " + n + " player(s)");
});

Commands.register("sm_nofire", (ctx) => {
  const secs = ctx.args[0] ? Number(ctx.args[0]) : 5;
  const now = Server.gameTime;
  for (const { slot, pawn } of livePawns()) {
    const ok = pawn.blockFiring(secs);
    ctx.reply("[wpn] slot=" + slot + " blockFiring(" + secs + ")=" + ok + " nextAttack=" + pawn.nextAttack + " gameTime=" + now);
  }
});
