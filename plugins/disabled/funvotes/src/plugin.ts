// @s2script/funvotes — SourceMod funvotes: admin Yes/No votes that toggle a cvar (AllTalk,
// FriendlyFire), set gravity, or slay a targeted player, on pass.
//
// Task 1 (this file): all 4 commands (sm_votealltalk/sm_voteff/sm_votegravity/sm_voteslay) +
// the shared Yes/No vote helper.

import {
  command, translations, ADMFLAG, Chat, config, Vote, Server, Translations,
} from "@s2script/sdk";
import type { Command, PhraseKey } from "@s2script/sdk";
import { Player } from "@s2script/cs2";

/** Start a Yes/No vote; on pass, run `onPass`. Refuses (via `cmd.replyT`) if a vote is already
 *  active — never queues, SM parity ("one vote at a time"). `questionKey` is resolved at the
 *  server default language (-1): the question is broadcast to every voter (embedded in the
 *  "Passed"/"Failed" line too), not privately replied to the requesting admin, so there is no
 *  single recipient to translate it for.
 *
 *  Pass semantics (SM parity): NOT plurality. A vote passes when the Yes SHARE of the votes cast is
 *  at least funvote_ratio (default 0.60). With no votes cast (total === 0) the share is 0 → it fails.
 *  options[0] === "Yes", so counts[0] is the Yes tally. */
function startYesNo(cmd: Command, questionKey: PhraseKey, questionArg: string | undefined, onPass: () => void): void {
  if (Vote.isActive()) { cmd.replyT("Vote Already Running"); return; }
  const question = questionArg === undefined
    ? Translations.translate(-1, questionKey)
    : Translations.translate(-1, questionKey, questionArg);
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
        Chat.toAll(Translations.translate(-1, "Vote Passed", pct(share), pct(ratio), question));
        onPass();
      } else {
        Chat.toAll(Translations.translate(-1, "Vote Failed", pct(share), pct(ratio), question));
      }
    },
  });
  cmd.replyT("Vote Started");
}

export function OnPluginStart(): void {
  translations.load("funvotes", "common");

  command.admin("sm_votealltalk", ADMFLAG.VOTE, cmd => {
    const on = ["1", "true"].includes(Server.getCvar("sv_alltalk"));
    startYesNo(cmd, on ? "Disable Alltalk Question" : "Enable Alltalk Question", undefined, () => Server.setCvar("sv_alltalk", on ? "0" : "1"));
  });

  command.admin("sm_voteff", ADMFLAG.VOTE, cmd => {
    const on = ["1", "true"].includes(Server.getCvar("mp_friendlyfire"));
    startYesNo(cmd, on ? "Disable Friendlyfire Question" : "Enable Friendlyfire Question", undefined, () => Server.setCvar("mp_friendlyfire", on ? "0" : "1"));
  });

  // DEVIATION FROM SM: SourceMod's sm_votegravity can present MULTIPLE gravity options in one
  // multi-choice vote (e.g. `sm_votegravity 200 400 800`). We keep it a single-value Yes/No vote
  // (one gravity value → pass/fail), which composes with the shared startYesNo helper. Multi-option
  // funvotes are a future item if demand appears.
  command.admin("sm_votegravity", ADMFLAG.VOTE, cmd => {
    const v = cmd.arg(0);
    if (!/^[0-9]+(\.[0-9]+)?$/.test(v)) { cmd.replyT("Usage Votegravity"); return; }
    startYesNo(cmd, "Set Gravity Question", v, () => Server.setCvar("sv_gravity", v));
  });

  command.admin("sm_voteslay", ADMFLAG.VOTE, cmd => {
    const targets = Player.target(cmd.arg(0), cmd.callerSlot, true);
    // Both replies below are LOCAL keys (colour-free, no "[SM] " prefix) — see phrases.ts.
    if (targets.length === 0) { cmd.replyT("Voteslay No Matching Players"); return; }
    if (targets.length > 1) { cmd.replyT("Voteslay Ambiguous Target"); return; }
    const uid = targets[0].userId;
    const name = targets[0].playerName ?? "player";
    startYesNo(cmd, "Slay Question", name, () => {
      const p = Player.fromUserId(uid);   // re-resolve at end (pick-time slot/pawn may be stale)
      if (p && p.pawn) p.pawn.slay();
    });
  });

  // DESCOPED: SM's sm_voteburn (vote to ignite a player) is intentionally not implemented — it
  // needs a player-ignite primitive that does not exist in the framework yet, and this slice does
  // NOT invent RE work. Revisit once an ignite/entity-fire capability lands (like pawn.slay for
  // sm_voteslay).
  console.log("[funvotes] onLoad — votealltalk/voteff/votegravity/voteslay registered");
}
