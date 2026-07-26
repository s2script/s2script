---
'@s2script/cs2': minor
'@s2script/sdk': patch
---

Schema codegen: embedded structs, enums, `wrapEntity`, and every entity class.

Catch-up changeset — this work merged in #26, #28 and #29 without one, so neither package was
versioned or published and the new surface is unreachable from npm.

`@s2script/cs2` grows from 62 to 415 interfaces and 793 to 2740 exposed fields:

- **Embedded structs** as nested accessors, at any depth — `pawn.glow.glowing`,
  `pawn.collision.collisionGroup`, and struct-inside-struct via
  `collision.collisionAttribute.collisionGroup`.
- **`wrapEntity(className, ref)`** — schema accessors on an entity you created. `createEntity`
  returns a bare `EntityRef`; this is how it gets fields. Keyed on a generated class map, so a wrong
  name is a compile error rather than an object with silently missing accessors.
- **Enums** as unsigned integers of the width their binding declares. They were skipped wholesale
  because the category names the type but not its width; the width is now dumped from the live
  SchemaSystem. 533 fields catalog-wide. Notably `moveType`, `solidType` and `renderMode`.
- **`Color` as a packed uint32** (R in the low byte), which also exposes `m_clrRender` as `render`.
  Previously skipped as "not a scalar", which is what kept `m_glowColorOverride` unreachable.
- **367 entity classes** instead of 13 — everything deriving from `CEntityInstance` that has fields,
  plus `CCSGameRules`. Costs 3.6 ms once at boot.
- **Transparent value wrappers flattened**: `pawn.deathTime` is a `number`, not `{ value }`.
  `GameTime_t`/`GameTick_t` were the majority of embedded fields on a pawn.

Offsets still resolve by NAME at runtime, so none of this bakes in a layout constant.

`@s2script/sdk` is a patch: only the codegen internals and its tests changed, no published `.d.ts`,
but the shipped `dist/cli.js` differs.
