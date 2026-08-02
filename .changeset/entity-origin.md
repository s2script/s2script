---
"@s2script/sdk": minor
---

Add `EntityRef.origin`, reading `CGameSceneNode::m_vecAbsOrigin`

A plugin had no supported way to read an arbitrary entity's world position. `EntityRef` had
`teleport` (a write) but no positional read; `@s2script/cs2` exposed `origin` only on `Pawn` (a
player body); and `schema.generated.d.ts` declares no origin field on `CBaseEntity`,
`CBaseModelEntity`, `CBaseButton`, or any other generic entity class (the schema codegen has no
pointer-chain accessor). Code that needed to compare non-pawn entities by position — e.g. picking
the nearest of several matching `func_button`s to a reference point when more than one candidate
opens a door — had no way to do it short of hand-rolling a memory read against a hardcoded,
build-specific offset chain.

`EntityRef.origin` closes that gap the same way `EntityRef.target` closes it for `m_target`: every
offset it needs — `CBaseEntity::m_CBodyComponent`, `CBodyComponent::m_pSceneNode`,
`CGameSceneNode::m_vecAbsOrigin` — is resolved live through the schema system and cached, never
baked in. It reads the absolute (world-hierarchy-resolved) origin rather than the parent-relative,
cell-quantized `m_vecOrigin`, matching what `Pawn.origin` already returns for player pawns, and
returns a `Vector` for consistency with every other 3-component field read in the schema (e.g.
`CBaseEntity.absVelocity`). There is no meaningful "empty" position, so unlike `name`/`target`'s
null-vs-`""` contract, `null` means exactly one thing here: could not read (stale/invalid ref, or
the offset chain didn't resolve).
