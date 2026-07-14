---
"@s2script/entity": minor
---

Add `EntityRef.name` — read an entity's targetname (`CEntityIdentity::m_name`, a `CUtlSymbolLarge`). Serial-gated `string | null`: `""` when the entity has no targetname, `null` when the ref is stale/invalid. Unblocks name-based entity/zone discovery (e.g. classifying map triggers by `map_start`/`map_end`).
