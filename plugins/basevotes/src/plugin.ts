import { plugin } from "@s2script/sdk/plugin";
import { Vote } from "@s2script/sdk/votes";
import { ADMFLAG } from "@s2script/sdk/admin";
import { Chat } from "@s2script/sdk/chat";
import { config } from "@s2script/sdk/config";
import { Player, pickPlayer } from "@s2script/cs2";

// Parse a command arg string into quoted (or bare) tokens: sm_vote "Kick Rex?" Yes No
function parseTokens(s: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) out.push(m[1] !== undefined ? m[1] : m[2]);
  return out;
}

function startKickVote(userId: number, name: string): boolean {
  return Vote.start({
    question: "Kick " + name + "?",
    options: ["Yes", "No"],
    duration: config.getInt("vote_duration"),
    showLiveTally: config.getBool("show_live_tally"),
    onEnd: (r) => {
      if (r.winner === 0 && r.counts[0] > r.total / 2) {
        const cur = Player.fromUserId(userId);   // re-resolve at end (pick-time slot may be stale)
        if (cur) cur.kick("Vote kicked");
        Chat.toAll("[Vote] " + name + " was vote-kicked.");
      } else {
        Chat.toAll("[Vote] Kick " + name + " failed.");
      }
    },
  });
}

export default plugin((ctx) => {
  ctx.commands.registerAdmin("sm_vote", ADMFLAG.VOTE, (cmd) => {
    const toks = parseTokens(cmd.argString);
    if (toks.length < 3) { cmd.reply('Usage: sm_vote "Question" "Opt1" "Opt2" ...'); return; }
    const question = toks[0], options = toks.slice(1, 10);   // up to 9 options (single-digit chat)
    if (!Vote.start({ question, options, duration: config.getInt("vote_duration"), showLiveTally: config.getBool("show_live_tally"),
                      onEnd: (r) => { Chat.toAll(r.winner === null ? "[Vote] No decision." : "[Vote] Result: " + options[r.winner]); } })) {
      cmd.reply("[SM] A vote is already in progress.");
    }
  });

  ctx.commands.registerAdmin("sm_votekick", ADMFLAG.VOTE, (cmd) => {
    const targetStr = cmd.arg(0);
    if (!targetStr) { cmd.reply("Usage: sm_votekick <target>"); return; }
    const targets = Player.target(targetStr, cmd.callerSlot, true);
    if (targets.length === 0) { cmd.reply("[SM] No matching players."); return; }
    if (targets.length > 1) { cmd.reply("[SM] Ambiguous target."); return; }
    const p = targets[0];
    if (Vote.isActive()) { cmd.reply("[SM] A vote is already in progress."); return; }
    startKickVote(p.userId, p.playerName ?? "player");
  });

  ctx.topmenu.addItem("Voting Commands", { id: "basevotes:votekick", name: "Vote Kick", flags: ADMFLAG.VOTE,
    onSelect: adminSlot => pickPlayer(adminSlot, t => {
      if (!startKickVote(t.userId, t.playerName ?? "player")) Chat.toSlot(adminSlot, "[SM] A vote is already in progress.");
    }) });

  console.log("[basevotes] onLoad — sm_vote/sm_votekick registered");
});
