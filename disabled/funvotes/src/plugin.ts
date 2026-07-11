// @s2script/funvotes — SourceMod funvotes: admin Yes/No votes that toggle a cvar (AllTalk,
// FriendlyFire), set gravity, or slay a targeted player, on pass.
//
// Task 1 (this file): all 4 commands (sm_votealltalk/sm_voteff/sm_votegravity/sm_voteslay) +
// the shared Yes/No vote helper.

import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Chat } from "@s2script/chat";
import { config } from "@s2script/config";
import { Vote } from "@s2script/votes";
import { Player } from "@s2script/cs2";
import { Server } from "@s2script/server";

/** Start a Yes/No vote; on Yes (winner === 0, options[0] === "Yes"), run `onPass`. Refuses (via
 *  `reply`) if a vote is already active — never queues, SM parity ("one vote at a time"). */
function startYesNo(reply: (m: string) => void, question: string, onPass: () => void): void {
  if (Vote.isActive()) { reply("A vote is already running."); return; }
  Vote.start({
    question,
    options: ["Yes", "No"],
    duration: config.getInt("funvote_duration"),
    showLiveTally: config.getBool("funvote_show_tally"),
    onEnd: (r) => {
      if (r.winner === 0) {
        Chat.toAll("[Vote] Passed: " + question);
        onPass();
      } else {
        Chat.toAll("[Vote] Failed: " + question);
      }
    },
  });
  reply("Vote started.");
}

export function onLoad(): void {
  Commands.registerAdmin("sm_votealltalk", ADMFLAG.VOTE, ctx => {
    const on = ["1", "true"].includes(Server.getCvar("sv_alltalk"));
    startYesNo(ctx.reply, (on ? "Disable" : "Enable") + " AllTalk?", () => Server.setCvar("sv_alltalk", on ? "0" : "1"));
  });

  Commands.registerAdmin("sm_voteff", ADMFLAG.VOTE, ctx => {
    const on = ["1", "true"].includes(Server.getCvar("mp_friendlyfire"));
    startYesNo(ctx.reply, (on ? "Disable" : "Enable") + " Friendly Fire?", () => Server.setCvar("mp_friendlyfire", on ? "0" : "1"));
  });

  Commands.registerAdmin("sm_votegravity", ADMFLAG.VOTE, ctx => {
    const v = ctx.arg(0);
    if (!/^[0-9]+(\.[0-9]+)?$/.test(v)) { ctx.reply("Usage: sm_votegravity <number>"); return; }
    startYesNo(ctx.reply, "Set gravity to " + v + "?", () => Server.setCvar("sv_gravity", v));
  });

  Commands.registerAdmin("sm_voteslay", ADMFLAG.VOTE, ctx => {
    const targets = Player.target(ctx.arg(0), ctx.callerSlot, true);
    if (targets.length === 0) { ctx.reply("No matching players"); return; }
    if (targets.length > 1) { ctx.reply("Multiple players match — be specific"); return; }
    const uid = targets[0].userId;
    const name = targets[0].playerName ?? "player";
    startYesNo(ctx.reply, "Slay " + name + "?", () => {
      const p = Player.fromUserId(uid);   // re-resolve at end (pick-time slot/pawn may be stale)
      if (p && p.pawn) p.pawn.slay();
    });
  });

  console.log("[funvotes] onLoad — votealltalk/voteff/votegravity/voteslay registered");
}

export function onUnload(): void {
  console.log("[funvotes] onUnload");
}
