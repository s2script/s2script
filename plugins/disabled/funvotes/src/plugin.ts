// @s2script/funvotes — SourceMod funvotes: admin Yes/No votes that toggle a cvar (AllTalk,
// FriendlyFire), set gravity, or slay a targeted player, on pass.
//
// Task 1 (this file): all 4 commands (sm_votealltalk/sm_voteff/sm_votegravity/sm_voteslay) +
// the shared Yes/No vote helper.

import { plugin } from "@s2script/sdk/plugin";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Chat } from "@s2script/sdk/chat";
import { config } from "@s2script/sdk/config";
import { Vote } from "@s2script/sdk/votes";
import { Player } from "@s2script/cs2";
import { Server } from "@s2script/sdk/server";

/** Start a Yes/No vote; on pass, run `onPass`. Refuses (via `reply`) if a vote is already active —
 *  never queues, SM parity ("one vote at a time").
 *
 *  Pass semantics (SM parity): NOT plurality. A vote passes when the Yes SHARE of the votes cast is
 *  at least funvote_ratio (default 0.60). With no votes cast (total === 0) the share is 0 → it fails.
 *  options[0] === "Yes", so counts[0] is the Yes tally. */
function startYesNo(reply: (m: string) => void, question: string, onPass: () => void): void {
  if (Vote.isActive()) { reply("A vote is already running."); return; }
  Vote.start({
    question,
    options: ["Yes", "No"],
    duration: config.getInt("funvote_duration"),
    showLiveTally: config.getBool("funvote_show_tally"),
    onEnd: (r) => {
      const yes = r.counts[0] ?? 0;
      const share = r.total > 0 ? yes / r.total : 0;
      const ratio = config.getFloat("funvote_ratio");
      const pct = (x: number) => Math.round(x * 100) + "%";
      if (share >= ratio) {
        Chat.toAll("[Vote] Passed (" + pct(share) + " ≥ " + pct(ratio) + " Yes): " + question);
        onPass();
      } else {
        Chat.toAll("[Vote] Failed (" + pct(share) + " < " + pct(ratio) + " Yes): " + question);
      }
    },
  });
  reply("Vote started.");
}

export default plugin((ctx) => {
  ctx.commands.registerAdmin("sm_votealltalk", ADMFLAG.VOTE, cmd => {
    const on = ["1", "true"].includes(Server.getCvar("sv_alltalk"));
    startYesNo(cmd.reply, (on ? "Disable" : "Enable") + " AllTalk?", () => Server.setCvar("sv_alltalk", on ? "0" : "1"));
  });

  ctx.commands.registerAdmin("sm_voteff", ADMFLAG.VOTE, cmd => {
    const on = ["1", "true"].includes(Server.getCvar("mp_friendlyfire"));
    startYesNo(cmd.reply, (on ? "Disable" : "Enable") + " Friendly Fire?", () => Server.setCvar("mp_friendlyfire", on ? "0" : "1"));
  });

  // DEVIATION FROM SM: SourceMod's sm_votegravity can present MULTIPLE gravity options in one
  // multi-choice vote (e.g. `sm_votegravity 200 400 800`). We keep it a single-value Yes/No vote
  // (one gravity value → pass/fail), which composes with the shared startYesNo helper. Multi-option
  // funvotes are a future item if demand appears.
  ctx.commands.registerAdmin("sm_votegravity", ADMFLAG.VOTE, cmd => {
    const v = cmd.arg(0);
    if (!/^[0-9]+(\.[0-9]+)?$/.test(v)) { cmd.reply("Usage: sm_votegravity <number>"); return; }
    startYesNo(cmd.reply, "Set gravity to " + v + "?", () => Server.setCvar("sv_gravity", v));
  });

  ctx.commands.registerAdmin("sm_voteslay", ADMFLAG.VOTE, cmd => {
    const targets = Player.target(cmd.arg(0), cmd.callerSlot, true);
    if (targets.length === 0) { cmd.reply("No matching players"); return; }
    if (targets.length > 1) { cmd.reply("Multiple players match — be specific"); return; }
    const uid = targets[0].userId;
    const name = targets[0].playerName ?? "player";
    startYesNo(cmd.reply, "Slay " + name + "?", () => {
      const p = Player.fromUserId(uid);   // re-resolve at end (pick-time slot/pawn may be stale)
      if (p && p.pawn) p.pawn.slay();
    });
  });

  // DESCOPED: SM's sm_voteburn (vote to ignite a player) is intentionally not implemented — it
  // needs a player-ignite primitive that does not exist in the framework yet, and this slice does
  // NOT invent RE work. Revisit once an ignite/entity-fire capability lands (like pawn.slay for
  // sm_voteslay).
  console.log("[funvotes] onLoad — votealltalk/voteff/votegravity/voteslay registered");
});
