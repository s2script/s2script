---
"@s2script/sdk": minor
---

SDKHooks lifecycle virtuals (`Spawn` / `Think` / `Use` / `GetMaxHealth` / `ShouldCollide` / `PreThink` / `PostThink` / `VPhysicsUpdate` / `GroundEntChangedPost` / `CanBeAutobalanced` and matching `*Post`) on the existing `SDKHook` / `SDKUnhook` API, plus `UseType`. Wiki names with no engine backing return `false`; unknown type strings still throw.
