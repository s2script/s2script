---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

`@s2script/sdk/hud` hides parts of a player's native HUD. `Hud.hide(slot, bits)` / `show` / `set` / `reset` keep a per-plugin mask that the host ORs together on the player's current pawn (`m_iHideHUD`), on top of the game's own bits. The mask follows the player across respawns and spectating, is re-applied if the game resets it, and an unload removes only that plugin's bits. `@s2script/cs2` names CS2's bits as `HideHud` (`Radar`, `Health`, `Crosshair`, `Money`, `RoundTimer`, `Chat`).
