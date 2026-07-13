---
"@s2script/entity": minor
---

Entity lifecycle listeners: `Entity.onCreate` / `onSpawn` / `onDelete(className, handler)` fire when the
engine creates/spawns/deletes an entity of `className` (`"*"` = all), delivering a serial-gated
`EntityRef` (may be null) plus the `className`. Class-keyed, notify-only. Backed by a signature-scanned
`CGameEntitySystem::AddListenerEntity` (the CSSharp/ModSharp `IEntityListener` mechanism).
