---
"@s2script/cs2": minor
---

`GameHud.weapon` is a weapon card (bottom right) that replaces the stock weapons HUD: the active weapon's name, clip / reserve in large text with a low-ammo tone at or under 25%, a grenade count, and a compact slot list with the active slot highlighted. `claim()` returns whether this plugin holds it; `auto(audience, { name?, hideSlots? })` fills it from each pawn every frame (painting only changes, hidden while dead or spectating); `set()` and `hide()` give manual control. It is released, and auto stops, when the plugin unloads. `HideHud.Weapons` names bit 6, which hides the weapons HUD, not chat; `HideHud.Chat` stays as a deprecated alias with the same value. `WeaponData.primaryAmmoType` reads the weapon's ammo index. The layout gains new panel ids, so the game-kit workshop addon must be republished.
