---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Add `Ptr` to `@s2script/sdk/unsafe`: a branded raw address with typed reads and writes, pointer-field
`deref`, and `Ptr.from(entity)`. Gamedata calls accept `ptr` arguments and a `ptr` return.
`@s2script/cs2` adds `structView(className)`, which applies a schema class's accessors to a `Ptr`;
generated views now share one prototype per class, so their fields are inherited rather than own
properties.
