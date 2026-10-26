---
"@s2script/sdk": minor
---

`Entity.findByName(name)` and `Entity.findInSphere(center, radius, className?)` in
`@s2script/sdk/entity`: find entities by targetname (SourceMod's `FindEntityByTargetname` loop) or by
distance from a point. Both return liveness-gated `EntityRef`s like `findByClass`.
