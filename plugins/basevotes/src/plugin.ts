import { plugin } from "@s2script/sdk/plugin";
import { Vote } from "@s2script/sdk/votes";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Chat } from "@s2script/sdk/chat";
import { config } from "@s2script/sdk/config";
import { Player, pickPlayer } from "@s2script/cs2";
import { Translations } from "@s2script/sdk/translations";
import { phrases } from "./phrases";

// Parse a command arg string into quoted (or bare) tokens: sm_vote "Kick Rex?" Yes No
function parseTokens(s: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) out.push(m[1] !== undefined ? m[1] : m[2]);
  return out;
}

// The vote QUESTION is shown to every voter (broadcast by the core vote system, not through this
// plugin's own Chat funnel), so — like the pass/fail lines below — it translates at the server
// default language (-1), not the requesting admin's.
function startKickVote(userId: number, name: string): boolean {
  return Vote.start({
    question: Translations.translate(-1, "Kick Vote Question", name),
    options: ["Yes", "No"],
    duration: config.getInt("vote_duration"),
    showLiveTally: config.getBool("show_live_tally"),
    onEnd: (r) => {
      if (r.winner === 0 && r.counts[0] > r.total / 2) {
        const cur = Player.fromUserId(userId);   // re-resolve at end (pick-time slot may be stale)
        // Player.kick's reason is the engine disconnect UI for the TARGET — translate for them.
        if (cur) cur.kick(Translations.translate(cur.slot, "Kick Vote Reason"));
        Chat.toAll(Translations.translate(-1, "Kick Vote Success", name));
      } else {
        Chat.toAll(Translations.translate(-1, "Kick Vote Failed", name));
      }
    },
  });
}

export default plugin((ctx) => {
  // Own set FIRST, common SECOND: translate takes the first hit across sets, so this order is what
  // lets a plugin override a shared phrase.
  Translations.load("basevotes", phrases);
  Translations.load("common");

  ctx.commands.registerAdmin("sm_vote", ADMFLAG.VOTE, (cmd) => {
    const toks = parseTokens(cmd.argString);
    if (toks.length < 3) { cmd.replyT("Usage Vote"); return; }
    const question = toks[0], options = toks.slice(1, 10);   // up to 9 options (single-digit chat)
    if (!Vote.start({ question, options, duration: config.getInt("vote_duration"), showLiveTally: config.getBool("show_live_tally"),
                      onEnd: (r) => {
                        Chat.toAll(r.winner === null
                          ? Translations.translate(-1, "Vote No Decision")
                          : Translations.translate(-1, "Vote Result", options[r.winner]));
                      } })) {
      cmd.replyT("Vote In Progress");
    }
  });

  ctx.commands.registerAdmin("sm_votekick", ADMFLAG.VOTE, (cmd) => {
    const targetStr = cmd.arg(0);
    if (!targetStr) { cmd.replyT("Usage Votekick"); return; }
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.replyT("No matching players"); return; }
    // Reuse common's ambiguous-target phrase (it takes the attempted pattern as {1}) rather than a
    // bare local "Ambiguous target." — same meaning, more informative, no advisory text lost.
    if (targets.length > 1) { cmd.replyT("More than one client matched", targetStr); return; }
    const p = targets[0];
    if (Vote.isActive()) { cmd.replyT("Vote In Progress"); return; }
    startKickVote(p.userId, p.playerName ?? "player");
  });

  // `name` is a static field set once here, before any admin has opened the menu, so — same as
  // basecommands' "Change Map Item" — it can only resolve at the server default language (-1), not
  // per-viewer.
  ctx.topmenu.addItem("Voting Commands", { id: "basevotes:votekick", name: Translations.translate(-1, "Vote Kick Item"), flags: ADMFLAG.VOTE,
    onSelect: adminSlot => pickPlayer(adminSlot, t => {
      if (!startKickVote(t.userId, t.playerName ?? "player")) Chat.toSlot(adminSlot, Translations.translate(adminSlot, "Vote In Progress"));
    }) });

  console.log("[basevotes] onLoad — sm_vote/sm_votekick registered");
});
