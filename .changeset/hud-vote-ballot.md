---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Paint CS2 votes as a right-side rail on the shared `s2script_lib.xml` layout (addon 3790153369), not a second CustomHudLayout.

`VoteTally.choice` is this slot's cast (or null). A registered tally renderer always paints; `showLiveTally` is leftover when a renderer exists. Chat is one line. HUD clicks go through `__s2_vote_cast`.
