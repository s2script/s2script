**Game kit workshop layouts (2026-11-15). Generated. Still needs a Workshop Tools compile, a publish and a live check.** This is the second slice of the gamemode HUD work. `scripts/gen-gamehud-layout.mjs` writes two layouts, one shared stylesheet (the hand rules in `scripts/gamehud/base.css` plus the generated class families), a manifest and `games/cs2/js/gamekit.generated.js` into the hudkit workshop addon source. The JS file is the descriptor the server runtime reads, so the panel ids, class counts and meter names come from one source.

- **`s2script_gamehud.xml`** (87 ids, 44 dialog variables) holds the passive in-round pieces:
  - a top rail with two team sides (score, label, 10 alive pips), a clock and a sub line
  - a six-row kill feed that is only as wide as its text
  - a status card with a role, a hint and two bars
  - a toast
- **`s2script_gamepanels.xml`** (470 ids, 314 dialog variables, 97 buttons, 9 meters) holds the clickable surfaces:
  - a pop-up vote with 8 options and fill meters
  - a side drawer with 5 tabs and 8 rows
  - a prompt with 4 answers
  - a sheet with 16 side tabs, a pager and every body type (list, paragraphs, toggles, item grid, action table)
  - the intro/spectate hub with a profile card, a player picker and a camera shot label

Both layouts spawn with `observable: false`, so each player sees their own HUD and spectators never see the watched player's panels. Each layout has its own 1024-entry intern budget. Accent colours come from seven colour-named themes. Text, fill and edge colours use the chat colour tags (`gk-t-green`, `gk-e-lightred`, ...), so a phrase colours the same in chat and on the HUD; a label takes the colour of its leading tag. Bars and meters move in 10% steps (`s2-w0..s2-w10`). The stock radar stays, and the custom radar and compass from the first draft are gone. `.HUD_BUYMENU_VISIBLE` and `.HUD_SCOREBOARD_VISIBLE` hide the passive pieces. `check-gamehud-layout-generated.sh` keeps the committed files in step with the generator. Live check (workshop README): both layouts load with no client error dialog, buttons click through, and the meter steps animate.
