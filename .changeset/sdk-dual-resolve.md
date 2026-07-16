---
"@s2script/sdk": minor
---

First release: consolidated builtin `.d.ts` + dual-resolve.

`@s2script/sdk` becomes a real (types-only, no bin) package: the 29 builtin capability
`.d.ts` plus `globals.d.ts` move into `packages/sdk/` behind an `exports` subpath map, so
a plugin can import `@s2script/sdk/entity` (…and every other capability). The bin + CLI
absorb into this package in a later PR.

To keep the move atomic, `s2require` and all four CLI type-resolution sites learn the new
layout in the same change. `s2require` strips `@s2script/sdk/<cap>` BEFORE the legacy
`@s2script/<cap>` (order is load-bearing — the shorter prefix also matches the longer
spelling) so both resolve to the same `__s2pkg_<cap>` global. The typecheck gate resolves
builtins at `packages/sdk/<cap>.d.ts` (new) or `packages/<cap>/index.d.ts` (legacy,
now serving only `@s2script/cs2`), under both import spellings. Fully backward-compatible:
existing plugins that import `@s2script/<cap>` still resolve.

Ships the no-degrade canary: a deliberate builtin type error still FAILS the gate under
both spellings (green CI would be the silent-failure signature of resolution degrading
to `any`).
