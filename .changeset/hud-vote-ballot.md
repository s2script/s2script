---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Paint CS2 votes as a right-side rail on the `.s2-vote` family already in `s2script_lib.xml` (addon 3790153369). No new workshop layout.

`VoteTally.choice` is this slot's cast (or null). A registered tally renderer always paints; `showLiveTally` is leftover when a renderer exists. Chat is one line. HUD clicks go through `__s2_vote_cast`.
