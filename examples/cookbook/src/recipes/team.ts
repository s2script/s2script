import type { Recipe } from "../recipe.ts";
import { Player } from "@s2script/cs2";
import { delay } from "@s2script/sdk/timers";

// Two different operations, easy to confuse:
//   changeTeam  — the engine's ChangeTeam: jointeam semantics, usually kills the pawn
//   switchTeam  — an immediate move that keeps the player alive and armed
// Use switchTeam for team balancing mid-round; changeTeam for a real join.
const TEAM_NAME = (t: number | null | undefined): string =>
  t === 1 ? "Spectator" : t === 2 ? "T" : t === 3 ? "CT" : `#${t}`;

export const teamRecipe: Recipe = {
  name: "team",
  describe: "changeTeam (cb_changeteam spectest|unspec) vs switchTeam (cb_switchteam switchtest|deadtest|revealtest)",
  register(ctx) {
    // --- changeTeam: jointeam semantics, usually kills the pawn ------------
    ctx.commands.register("cb_changeteam", (cmd) => {
      const sub = cmd.arg(0) || "spectest";

      if (sub === "unspec") {
        const team = cmd.argInt(1, 3); // default CT
        for (const p of Player.allConnected()) {
          if (p.teamNum === 1) {
            p.changeTeam(team);
            cmd.reply(`unspec slot=${p.slot} uid=${p.userId}: Spectator -> ${TEAM_NAME(team)}`);
            console.log(`[cookbook] changeteam moved slot=${p.slot} Spectator -> ${TEAM_NAME(team)}`);
            return;
          }
        }
        cmd.reply("changeteam: no spectators to move");
        return;
      }

      const ps = Player.all(); // in-game (pawn-gated)
      if (!ps.length) { cmd.reply("changeteam: no in-game players"); return; }
      const p = ps[0];
      const uid = p.userId;
      const before = p.teamNum ?? -1;
      p.spectate(); // = changeTeam(1)
      const immediate = p.teamNum ?? -1; // SYNCHRONOUS re-read on the same controller ref
      cmd.reply(`spectest slot=${p.slot} uid=${uid}: ${TEAM_NAME(before)} -> spectate(); immediate=${TEAM_NAME(immediate)}`);
      delay(600).then(() => {
        const after = Player.fromUserId(uid); // pawnless-safe re-resolve
        const t = after ? (after.teamNum ?? -1) : -1;
        console.log(`[cookbook] changeteam slot=${p.slot} uid=${uid} teamNum after=${t} (${TEAM_NAME(t)}) — expect Spectator(1)`);
      });
    });

    // --- switchTeam: non-lethal, keeps the player alive and armed ----------
    ctx.commands.register("cb_switchteam", (cmd) => {
      const sub = cmd.arg(0) || "switchtest";

      if (sub === "deadtest") {
        const dead = Player.allConnected().find(
          (p) => p.pawnIsAlive === false && (p.teamNum === 2 || p.teamNum === 3)
        );
        if (!dead) { cmd.reply("switchteam: no dead T/CT player (slay one first)"); return; }
        const uid = dead.userId;
        const before = dead.teamNum ?? -1;
        const target = before === 2 ? 3 : 2;
        dead.switchTeam(target);
        const immediate = dead.teamNum ?? -1;
        cmd.reply(`deadtest slot=${dead.slot}: DEAD ${TEAM_NAME(before)} -> ${TEAM_NAME(target)}; ` +
                  `immediate=${TEAM_NAME(immediate)} pawnIsAlive=${dead.pawnIsAlive}`);
        delay(600).then(() => {
          const after = Player.fromUserId(uid);
          console.log(`[cookbook] switchteam deadtest AFTER team=${TEAM_NAME(after ? after.teamNum : null)} ` +
                      `pawnIsAlive=${after ? after.pawnIsAlive : null} — expect ${TEAM_NAME(target)} + false`);
        });
        return;
      }

      if (sub === "revealtest") {
        let n = 0;
        for (const p of Player.allConnected()) {
          if (p.teamNum === 2) { p.switchTeam(3); n++; }
        }
        cmd.reply(`revealtest: moved ${n} T player(s) -> CT (round-end reveal shape)`);
        console.log(`[cookbook] switchteam revealtest moved ${n} players T->CT in one frame`);
        return;
      }

      const ps = Player.all(); // in-game (pawn-gated) — alive players
      if (!ps.length) { cmd.reply("switchteam: no in-game players"); return; }
      const p = ps[0];
      const uid = p.userId;
      const before = p.teamNum ?? -1;
      const pawnBefore = p.pawn;
      const refBefore = pawnBefore ? `${pawnBefore.ref.index}:${pawnBefore.ref.id}` : "none";
      const hpBefore = pawnBefore ? (pawnBefore.health ?? -1) : -1;
      const wepBefore = pawnBefore ? pawnBefore.weapons.length : -1;
      const target = before === 2 ? 3 : 2;
      p.switchTeam(target);
      const immediate = p.teamNum ?? -1; // SYNCHRONOUS re-read — the move is immediate
      cmd.reply(`switchtest slot=${p.slot}: ${TEAM_NAME(before)} -> switchTeam(${TEAM_NAME(target)}); immediate=${TEAM_NAME(immediate)}`);
      console.log(`[cookbook] switchteam slot=${p.slot} uid=${uid} before=${TEAM_NAME(before)} immediate=${TEAM_NAME(immediate)} ` +
                  `hpBefore=${hpBefore} wepBefore=${wepBefore} pawnRefBefore=${refBefore}`);
      delay(600).then(() => {
        const after = Player.fromUserId(uid); // the pawn may have been respawned — re-resolve
        const pawn = after ? after.pawn : null;
        const refAfter = pawn ? `${pawn.ref.index}:${pawn.ref.id}` : "none";
        console.log(`[cookbook] switchteam AFTER slot=${p.slot} team=${TEAM_NAME(after ? after.teamNum : null)} ` +
                    `alive=${after ? after.pawnIsAlive : null} hp=${pawn ? pawn.health : null} ` +
                    `weapons=${pawn ? pawn.weapons.length : -1} pawnRef=${refAfter} respawned=${refAfter !== refBefore} ` +
                    `— expect team=${TEAM_NAME(target)}, alive=true, weapons kept`);
      });
    });
  },
};
