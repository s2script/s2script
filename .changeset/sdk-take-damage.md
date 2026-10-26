---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Deal damage and heal. `entity.takeDamage(damage, { attacker, inflictor, ability, damageType, force, position })`
in `@s2script/sdk/entity`, and SourceMod's `SDKHooks_TakeDamage` in `@s2script/sdk/sdkhooks`, deal damage
through the game's own damage path, so `OnTakeDamage` hooks and the game's rules (god mode, friendly
fire, armor) still apply. `pawn.heal(amount, { max })` in `@s2script/cs2` adds health up to the pawn's max
health, or past it to `max`.
