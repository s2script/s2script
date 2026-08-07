/** basevotes — English seed. Generated into translations/basevotes.phrases.json by scripts/gen-phrases.mjs. */
export const phrases = {
  "Usage Vote": 'Usage: sm_vote "Question" "Opt1" "Opt2" ...',
  "Usage Votekick": "Usage: sm_votekick <target>",
  // "[SM]"-prefixed replies (cmd.reply / Chat.toSlot, both funnels that expand tags) keep the
  // established {green}[SM]{default} convention.
  "Vote In Progress": "{green}[SM]{default} A vote is already in progress.",
  // "[Vote] ..." broadcasts never had a colour or "[SM]" prefix in the original — stay colour-free.
  "Kick Vote Success": "[Vote] {1} was vote-kicked.",
  "Kick Vote Failed": "[Vote] Kick {1} failed.",
  "Vote No Decision": "[Vote] No decision.",
  "Vote Result": "[Vote] Result: {1}",
};
