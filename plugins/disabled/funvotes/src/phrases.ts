/** funvotes — English seed. Generated into translations/funvotes.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  // None of these ever carried a colour or an "[SM] " prefix — none gain a tag now.
  "Vote Already Running": "A vote is already running.",
  "Vote Started": "Vote started.",

  // The vote QUESTION and the "[Vote] Passed/Failed" broadcast — both shown to every voter via
  // Chat.toAll, so both resolve at the server default language (-1), not the requesting admin's.
  "Disable Alltalk Question": "Disable AllTalk?",
  "Enable Alltalk Question": "Enable AllTalk?",
  "Disable Friendlyfire Question": "Disable Friendly Fire?",
  "Enable Friendlyfire Question": "Enable Friendly Fire?",
  "Set Gravity Question": "Set gravity to {1}?",
  "Slay Question": "Slay {1}?",

  "Vote Passed": "[Vote] Passed ({1} ≥ {2} Yes): {3}",
  "Vote Failed": "[Vote] Failed ({1} < {2} Yes): {3}",

  "Usage Votegravity": "Usage: sm_votegravity <number>",
  // Both of sm_voteslay's failure replies below are LOCAL, not common's colour/"[SM]"-prefixed
  // equivalents — this command's failure messages never carried a colour or an "[SM] " prefix in
  // the original (matching every other phrase in this file), so reusing common's "No matching
  // players" / "More than one client matched" here would add colour/prefix to only ONE of the two
  // adjacent replies, or would be a wording change beyond what's warranted; kept local, both
  // colour-free, to match the source exactly and to match each other.
  "Voteslay No Matching Players": "No matching players",
  "Voteslay Ambiguous Target": "Multiple players match — be specific",
};
