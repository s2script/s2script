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
  // Local, not common's "More than one client matched" — this carries no advisory text and no
  // colour/"[SM]" prefix in the original, so reusing common would both add colour and drop nothing
  // (there's nothing to drop) but would still be a wording change beyond what's warranted here;
  // kept local to match the source exactly.
  "Voteslay Ambiguous Target": "Multiple players match — be specific",
};
