/** basevotes — English seed. Generated into translations/basevotes.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  "Usage Vote": 'Usage: sm_vote "Question" "Opt1" "Opt2" ...',
  "Usage Votekick": "Usage: sm_votekick <target>",
  // "[SM]"-prefixed replies (cmd.reply / Chat.toSlot, both funnels that expand tags) keep the
  // established {green}[SM]{default} convention.
  "Vote In Progress": "{green}[SM]{default} A vote is already in progress.",
  // "[Vote] ..." broadcasts never had a colour or "[SM]" prefix in the original — stay colour-free.
  "Kick Vote Question": "Kick {1}?",
  "Kick Vote Success": "[Vote] {1} was vote-kicked.",
  "Kick Vote Failed": "[Vote] Kick {1} failed.",
  "Vote No Decision": "[Vote] No decision.",
  "Vote Result": "[Vote] Result: {1}",
  // Player.kick's reason argument is the ENGINE disconnect UI, not the chat/console funnel — it
  // never expands colour tags, so no {tag} here (same treatment as basecommands' "Kick Reason
  // Default" and reservedslots' "Kick Reserved Slot").
  "Kick Vote Reason": "Vote kicked",
  // MenuStyle.Center topmenu item name — resolved once at registration (no viewer yet), same
  // limitation/pattern as basecommands' "Change Map Item".
  "Vote Kick Item": "Vote Kick",
};
