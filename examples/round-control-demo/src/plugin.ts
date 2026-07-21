// @demo/round-control-demo — live gate for the round-control slice.
//
//   sm_endround [reason] [delay]  — GameRules.terminateRound; the round_end logger below then proves
//                                   BOTH the deferred drain (our own handler fires even though WE
//                                   terminated from a JS dispatch) AND the closed-loop reason
//                                   read-back (engine round_end.reason === the reason we passed).
//   sm_settime <sec>              — setTimeRemaining + read-back (HUD clock repaint = human-visual).
//   sm_addtime <sec>              — addTimeRemaining + read-back.
//   sm_teamscore <team> <score>   — Teams.setScore + read-back (scoreboard visual = human).
//   sm_winpanel [2|3]             — synthetic cs_win_panel_round. NOTE: a JS-fired event never
//                                   re-dispatches to JS subscribers (isolate-borrow rule), so our own
//                                   cs_win_panel_round logger will NOT log this fire — expected; the
//                                   client-visible panel is the check.
//
// round_start logs roundStartTime/timeElapsed sanity (roundStartTime ~= gameTime, elapsed ~= 0);
// a NATURAL round end (mp_ignore_round_win_conditions 0, timer expiry) validates the shipped
// RoundEndReason/WinPanelFinalEvent values against engine-emitted events.

import { plugin } from "@s2script/sdk/plugin";
import { Events } from "@s2script/sdk/events";
import { Server } from "@s2script/sdk/server";
import { GameRules, Teams, RoundEndReason, WinPanelFinalEvent } from "@s2script/cs2";

let lastTerminateReason: number | null = null;

export default plugin((ctx) => {
  ctx.events.on("round_end", (e) => {
    const reason = e.getInt("reason");
    const winner = e.getInt("winner");
    const ours = lastTerminateReason !== null;
    const loop = ours ? (reason === lastTerminateReason ? " [OURS — closed-loop OK]" : ` [OURS — MISMATCH, sent ${lastTerminateReason}]`) : "";
    console.log(`[round-demo] round_end reason=${reason} winner=${winner}${loop}`);
    lastTerminateReason = null;
  });

  ctx.events.on("cs_win_panel_round", (e) => {
    console.log(`[round-demo] cs_win_panel_round final_event=${e.getInt("final_event")} (expect ${WinPanelFinalEvent.CTsWin}=CT / ${WinPanelFinalEvent.TerroristsWin}=T on a natural end)`);
  });

  ctx.events.on("round_start", () => {
    const gr = GameRules.get();
    if (!gr) { console.log("[round-demo] round_start: no gamerules proxy"); return; }
    console.log(`[round-demo] round_start roundTime=${gr.roundTime} roundStartTime=${gr.roundStartTime} gameTime=${Server.gameTime} timeElapsed=${gr.timeElapsed} timeRemaining=${gr.timeRemaining}`);
  });

  ctx.commands.register("sm_endround", (cmd) => {
    const reason = cmd.argInt(0, RoundEndReason.TerroristsWin);
    const delay = cmd.argInt(1, 5);
    lastTerminateReason = reason;
    const ok = GameRules.terminateRound(reason, delay);
    if (!ok) lastTerminateReason = null;
    cmd.reply(`endround reason=${reason} delay=${delay} queued=${ok} (round_end log follows next frame if queued)`);
  });

  ctx.commands.register("sm_settime", (cmd) => {
    const sec = cmd.argInt(0, 60);
    const gr = GameRules.get();
    const ok = gr ? gr.setTimeRemaining(sec) : false;
    if (!gr) { cmd.reply("settime: no gamerules"); return; }
    const rt = gr.roundTime, rst = gr.roundStartTime, now = Server.gameTime;
    const hud = (rt !== null && rst !== null) ? rt - (now - rst) : null;
    cmd.reply(`settime ${sec}: ok=${ok} roundTime=${rt} timeRemaining=${gr.timeRemaining} | freezeTime=${gr.freezeTime} roundStartTime=${rst} gameTime=${now} timeElapsed=${gr.timeElapsed} hud(rt-(now-rst))=${hud}`);
  });

  ctx.commands.register("sm_addtime", (cmd) => {
    const sec = cmd.argInt(0, 30);
    const gr = GameRules.get();
    const ok = gr ? gr.addTimeRemaining(sec) : false;
    cmd.reply(`addtime ${sec}: ok=${ok} roundTime=${gr ? gr.roundTime : null} timeRemaining=${gr ? gr.timeRemaining : null}`);
  });

  ctx.commands.register("sm_teamscore", (cmd) => {
    const team = cmd.argInt(0, 2);
    const score = cmd.argInt(1, 10);
    const ok = Teams.setScore(team, score);
    cmd.reply(`teamscore team=${team} -> ${score}: ok=${ok} readback=${Teams.getScore(team)}`);
  });

  ctx.commands.register("sm_winpanel", (cmd) => {
    const fe = cmd.argInt(0, WinPanelFinalEvent.TerroristsWin);
    const fired = Events.fire("cs_win_panel_round", { final_event: fe }, false);
    cmd.reply(`winpanel final_event=${fe} fired=${fired} (client-visible panel is the check; our own JS logger will NOT fire — expected)`);
  });

  console.log("[round-demo] onLoad — sm_endround / sm_settime / sm_addtime / sm_teamscore / sm_winpanel registered");
});
