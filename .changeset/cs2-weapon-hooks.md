---
"@s2script/cs2": minor
---

`weapons` in `@s2script/cs2`: `weapons.onCanUse`, `weapons.onCanSwitchTo` and `weapons.onDrop` hook
`CCSPlayer_WeaponServices::CanUse`, `CanSwitch` and `DropWeapon`. The pickup and switch gates vote
`allowed` like `chat.onCanHear` (any `false` wins, `Continue` leaves it to the engine), and `Handled`
on a drop keeps the weapon in the player's hands. These are the hooks game modes need to restrict
loadouts, which before could only strip a weapon after the fact.
