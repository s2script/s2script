---
"@s2script/sdk": minor
---

Add `EntityRef.target`, reading `CBaseEntity::m_target`

A map entity's target string — which entity it acts on, e.g. which `func_button` opens a
particular door — had no supported read path: it's absent from `schema.generated.d.ts` on both
`CBaseEntity` and `CBaseButton` (the generic schema codegen has no string/symbol accessor), and
plugin gamedata only supports declaring `signatures`/`calls`, not raw field offsets. A plugin that
needed to match entities by target had no way to do it short of hand-rolling a memory read against
a hardcoded, build-specific offset — exactly the kind of thing that breaks silently on the next
game update.

`EntityRef.target` closes that gap the same way `EntityRef.name` closes it for
`CEntityIdentity::m_name`: the offset is resolved live through the schema system and cached, never
baked in. It follows `name`'s exact null-vs-empty contract — `""` when the entity has no target,
`null` only when the ref itself is stale or invalid — so callers can tell "no target" from "could
not read" apart.
