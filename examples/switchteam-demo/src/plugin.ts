// @demo/switchteam-demo — live gate for the switchteam primitive (Player.switchTeam), the NON-LETHAL
// T<->CT move (sig-resolved CCSPlayerController::SwitchTeam). Mirrors the TTT port's three consumers:
//
//   sm_switchtest   — role-assignment shape (RoleIconsHandler): the first alive in-game player T<->CT;
//                     asserts the team moved IMMEDIATELY (synchronous read-back), then next-frame:
//                     still alive, weapons survived, and the pawn-respawn probe (EntityRef identity).
//   sm_deadtest     — body-identify shape (BodyPickupListener): a DEAD T/CT controller to the other
//                     team; asserts teamNum moved and pawnIsAlive stayed false (sm_slay a bot first).
//   sm_revealtest   — round-end reveal shape (RoundTimerListener): every T -> CT in bulk, one frame.

import { plugin } from "@s2script/sdk/plugin";
import { delay } from "@s2script/sdk/timers";
import { Player } from "@s2script/cs2";

const TEAM = (t: number | null | undefined): string =>
  t === 1 ? "Spectator" : t === 2 ? "T" : t === 3 ? "CT" : `#${t}`;

export default plugin((ctx) => {
  ctx.commands.register("sm_switchtest", (cmd) => {
    const ps = Player.all(); // in-game (pawn-gated) — alive players
    if (!ps.length) { cmd.reply("switchteam-demo: no in-game players"); return; }
    const p = ps[0];
    const uid = p.userId;
    const before = p.teamNum ?? -1;
    const pawnBefore = p.pawn;
    const refBefore = pawnBefore ? `${pawnBefore.ref.index}:${pawnBefore.ref.id}` : "none";
    const hpBefore = pawnBefore ? (pawnBefore.health ?? -1) : -1;
    const wepBefore = pawnBefore ? pawnBefore.weapons.length : -1;
    const target = before === 2 ? 3 : 2;
    p.switchTeam(target);
    const immediate = p.teamNum ?? -1; // SYNCHRONOUS re-read — the move must be immediate (spec §4)
    cmd.reply(`switchtest slot=${p.slot}: ${TEAM(before)} -> switchTeam(${TEAM(target)}); immediate=${TEAM(immediate)}`);
    console.log(`[switchteam-demo] slot=${p.slot} uid=${uid} before=${TEAM(before)} immediate=${TEAM(immediate)} ` +
                `hpBefore=${hpBefore} wepBefore=${wepBefore} pawnRefBefore=${refBefore}`);
    delay(600).then(() => {
      const after = Player.fromUserId(uid); // the pawn may have been respawned — re-resolve (TTT pattern)
      const pawn = after ? after.pawn : null;
      const refAfter = pawn ? `${pawn.ref.index}:${pawn.ref.id}` : "none";
      console.log(`[switchteam-demo] AFTER slot=${p.slot} team=${TEAM(after ? after.teamNum : null)} ` +
                  `alive=${after ? after.pawnIsAlive : null} hp=${pawn ? pawn.health : null} ` +
                  `weapons=${pawn ? pawn.weapons.length : -1} pawnRef=${refAfter} respawned=${refAfter !== refBefore} ` +
                  `— expect team=${TEAM(target)}, alive=true, weapons kept`);
    });
  });

  ctx.commands.register("sm_deadtest", (cmd) => {
    const dead = Player.allConnected().find(
      (p) => p.pawnIsAlive === false && (p.teamNum === 2 || p.teamNum === 3)
    );
    if (!dead) { cmd.reply("switchteam-demo: no dead T/CT player (sm_slay a bot first)"); return; }
    const uid = dead.userId;
    const before = dead.teamNum ?? -1;
    const target = before === 2 ? 3 : 2;
    dead.switchTeam(target);
    const immediate = dead.teamNum ?? -1;
    cmd.reply(`deadtest slot=${dead.slot}: DEAD ${TEAM(before)} -> ${TEAM(target)}; ` +
              `immediate=${TEAM(immediate)} pawnIsAlive=${dead.pawnIsAlive}`);
    delay(600).then(() => {
      const after = Player.fromUserId(uid);
      console.log(`[switchteam-demo] deadtest AFTER team=${TEAM(after ? after.teamNum : null)} ` +
                  `pawnIsAlive=${after ? after.pawnIsAlive : null} — expect ${TEAM(target)} + false ` +
                  `(the TTT BodyPickup contract; if true, log it — spec §9 documented side effect)`);
    });
  });

  ctx.commands.register("sm_revealtest", (cmd) => {
    let n = 0;
    for (const p of Player.allConnected()) {
      if (p.teamNum === 2) { p.switchTeam(3); n++; }
    }
    cmd.reply(`revealtest: moved ${n} T player(s) -> CT (round-end reveal shape)`);
    console.log(`[switchteam-demo] revealtest moved ${n} players T->CT in one frame`);
  });

  console.log("[switchteam-demo] onLoad — sm_switchtest / sm_deadtest / sm_revealtest registered");
});
