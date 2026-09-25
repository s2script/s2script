---
"@s2script/cs2": patch
"@s2script/sdk": patch
---

Relocate CS2-owned gamedata source into the game package and keep generated hook declarations and SDK CS2 bundle tests reading the new source manifest. Package builds now emit deterministic, hashed CS2 artifacts while preserving the deployed paths used by the current runtime.
