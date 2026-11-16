---
"@s2script/cs2": minor
---

`GameHud.vote(spec)` pops up a vote that does not take the cursor: voters type the option number in chat (the line is swallowed) or press Tab to click. `GameHud.drawer(player, spec)` opens a clickable side drawer with tabs, rows and a Close button. Both use the game-kit panels layout and fall back to chat when the HUD refuses them.
