---
"@s2script/cs2": minor
---

`weapon.data` in `@s2script/cs2`: a weapon's static VData (max clip, reserve max, price, kill award,
damage, cycle time, move speed, armor ratio, range, weapon type, gear slot, ...), the same data
CounterStrikeSharp exposes as `VData` and ModSharp as `GetWeaponData`. It is read-only and reads null on
a stale weapon. `Weapon.dataUnavailableReason` names why it is null when a CS2 update moves the
entity layout it is derived from.
