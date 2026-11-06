---
"@s2script/cs2": minor
---

Regenerate the schema types for the Nov 5 2026 CS2 build (26445667). New classes include CCSCustomHudLayout, CCSCustomPlayerCamera and CGenericLogicPlayerProxy. CBaseMoveBehavior, CEnvWindController, CEnvWindVolume and CLogicPlayerProxy are gone from the game, so their interfaces are removed; logic_playerproxy is now CGenericLogicPlayerProxy (fields on CLogicPlayerProxyBase).
