// @demo/changeteam-demo — live gate for the changeteam primitive (Player.changeTeam / .spectate),
// sig-resolved CCSPlayerController::SwitchTeam. Bot-provable: a bot is a real controller, so moving it
// to Spectator (team 1) and reading teamNum back proves the SwitchTeam call without a human client.
//
//   sm_spectest        — move the first in-game player (a bot on the gate) to Spectator, then re-read
//                        teamNum via userid after a short delay (spectators are pawnless, so re-resolve
//                        by userid, not the pawn-gated Player.all).
//   sm_unspec [team]   — move the first Spectator back to CT (team 3), or an explicit team number.

import { Commands } from "@s2script/commands";
import { Player } from "@s2script/cs2";
import { delay } from "@s2script/timers";

const TEAM_NAME = (t: number): string =>
  t === 1 ? "Spectator" : t === 2 ? "T" : t === 3 ? "CT" : `#${t}`;

export function onLoad(): void {
  Commands.register("spectest", (ctx) => {
    const ps = Player.all(); // in-game (pawn-gated)
    if (!ps.length) { ctx.reply("changeteam-demo: no in-game players"); return; }
    const p = ps[0];
    const uid = p.userId;
    const before = p.teamNum ?? -1;
    p.spectate(); // = changeTeam(1)
    const immediate = p.teamNum ?? -1; // SYNCHRONOUS re-read on the same controller ref
    ctx.reply(`spectest slot=${p.slot} uid=${uid}: ${TEAM_NAME(before)} -> spectate(); immediate=${TEAM_NAME(immediate)}`);
    console.log(`[changeteam-demo] slot=${p.slot} uid=${uid} before=${before}(${TEAM_NAME(before)}) immediate=${immediate}(${TEAM_NAME(immediate)})`);
    delay(600).then(() => {
      const after = Player.fromUserId(uid); // pawnless-safe re-resolve
      const t = after ? (after.teamNum ?? -1) : -1;
      console.log(`[changeteam-demo] slot=${p.slot} uid=${uid} teamNum after=${t} (${TEAM_NAME(t)}) — expect Spectator(1)`);
    });
  });

  Commands.register("unspec", (ctx) => {
    const team = ctx.argInt(0, 3); // default CT
    for (const p of Player.allConnected()) {
      if (p.teamNum === 1) {
        p.changeTeam(team);
        ctx.reply(`unspec slot=${p.slot} uid=${p.userId}: Spectator -> ${TEAM_NAME(team)}`);
        console.log(`[changeteam-demo] slot=${p.slot} moved Spectator -> ${TEAM_NAME(team)}`);
        return;
      }
    }
    ctx.reply("changeteam-demo: no spectators to move");
  });

  console.log("[changeteam-demo] onLoad — sm_spectest / sm_unspec registered");
}

export function onUnload(): void {
  console.log("[changeteam-demo] onUnload");
}
