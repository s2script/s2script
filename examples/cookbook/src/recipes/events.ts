import type { Recipe } from "../recipe.ts";
import { Events } from "@s2script/sdk/events";
import { Server } from "@s2script/sdk/server";
import { GameRules, Teams, RoundEndReason, WinPanelFinalEvent } from "@s2script/cs2";

/**
 * GameRules.terminateRound() queues a round end for the next GameFrame — the
 * round_end logger below proves BOTH the deferred drain (this recipe's own
 * handler fires even though it terminated the round from a JS dispatch) AND
 * the closed-loop reason read-back (the engine's round_end.reason matches
 * the reason passed in). Events.fire lets a plugin synthesize an event for
 * client delivery: note a JS-fired event never re-dispatches to JS
 * subscribers (the isolate-borrow rule), so this recipe's own
 * cs_win_panel_round logger will NOT log a fire it triggered itself — the
 * client-visible panel is the check.
 *
 *   sm_round               end the round (GameRules.terminateRound)
 *   sm_round_settime        set the round clock
 *   sm_round_addtime        add to the round clock
 *   sm_round_teamscore      set a team's scoreboard score
 *   sm_round_winpanel       fire a synthetic cs_win_panel_round
 */
export const eventsRecipe: Recipe = {
  name: "events",
  describe: "control round flow: terminate, adjust the clock, set score, fire an event (sm_round*)",
  register(ctx) {
    let lastTerminateReason: number | null = null;

    ctx.events.on("round_end", (e) => {
      const reason = e.getInt("reason");
      const winner = e.getInt("winner");
      const ours = lastTerminateReason !== null;
      const loop = ours ? (reason === lastTerminateReason ? " [OURS — closed-loop OK]" : ` [OURS — MISMATCH, sent ${lastTerminateReason}]`) : "";
      console.log(`[cookbook] round: round_end reason=${reason} winner=${winner}${loop}`);
      lastTerminateReason = null;
    });

    ctx.events.on("cs_win_panel_round", (e) => {
      console.log(`[cookbook] round: cs_win_panel_round final_event=${e.getInt("final_event")} (expect ${WinPanelFinalEvent.CTsWin}=CT / ${WinPanelFinalEvent.TerroristsWin}=T on a natural end)`);
    });

    ctx.events.on("round_start", () => {
      const gr = GameRules.get();
      if (!gr) { console.log("[cookbook] round: round_start: no gamerules proxy"); return; }
      console.log(`[cookbook] round: round_start roundTime=${gr.roundTime} roundStartTime=${gr.roundStartTime} gameTime=${Server.gameTime} timeElapsed=${gr.timeElapsed} timeRemaining=${gr.timeRemaining}`);
    });

    ctx.commands.register("sm_round", (cmd) => {
      const reason = cmd.argInt(0, RoundEndReason.TerroristsWin);
      const delay = cmd.argInt(1, 5);
      lastTerminateReason = reason;
      const ok = GameRules.terminateRound(reason, delay);
      if (!ok) lastTerminateReason = null;
      cmd.reply(`[cookbook] round: endround reason=${reason} delay=${delay} queued=${ok} (round_end log follows next frame if queued)`);
    });

    ctx.commands.register("sm_round_settime", (cmd) => {
      const sec = cmd.argInt(0, 60);
      const gr = GameRules.get();
      const ok = gr ? gr.setTimeRemaining(sec) : false;
      if (!gr) { cmd.reply("[cookbook] round: settime: no gamerules"); return; }
      const rt = gr.roundTime, rst = gr.roundStartTime, now = Server.gameTime;
      const hud = (rt !== null && rst !== null) ? rt - (now - rst) : null;
      cmd.reply(`[cookbook] round: settime ${sec}: ok=${ok} roundTime=${rt} timeRemaining=${gr.timeRemaining} | freezeTime=${gr.freezeTime} roundStartTime=${rst} gameTime=${now} timeElapsed=${gr.timeElapsed} hud(rt-(now-rst))=${hud}`);
    });

    ctx.commands.register("sm_round_addtime", (cmd) => {
      const sec = cmd.argInt(0, 30);
      const gr = GameRules.get();
      const ok = gr ? gr.addTimeRemaining(sec) : false;
      cmd.reply(`[cookbook] round: addtime ${sec}: ok=${ok} roundTime=${gr ? gr.roundTime : null} timeRemaining=${gr ? gr.timeRemaining : null}`);
    });

    ctx.commands.register("sm_round_teamscore", (cmd) => {
      const team = cmd.argInt(0, 2);
      const score = cmd.argInt(1, 10);
      const ok = Teams.setScore(team, score);
      cmd.reply(`[cookbook] round: teamscore team=${team} -> ${score}: ok=${ok} readback=${Teams.getScore(team)}`);
    });

    ctx.commands.register("sm_round_winpanel", (cmd) => {
      const fe = cmd.argInt(0, WinPanelFinalEvent.TerroristsWin);
      const fired = Events.fire("cs_win_panel_round", { final_event: fe }, false);
      cmd.reply(`[cookbook] round: winpanel final_event=${fe} fired=${fired} (client-visible panel is the check; our own JS logger will NOT fire — expected)`);
    });
  },
};
