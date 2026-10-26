---
"@s2script/cs2": minor
---

`pawn.dropActiveWeapon()` now drops the deployed weapon as a world pickup instead of always
returning `false`, and the new `pawn.dropWeapon(weapon)` drops one held weapon (SourceMod's
`CS_DropWeapon`). Both go through a self-resolved `CCSPlayer_WeaponServices::DropWeapon` binding.
