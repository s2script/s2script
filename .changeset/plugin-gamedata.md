---
'@s2script/sdk': minor
---

Add plugin-shippable gamedata and declared engine calls (`@s2script/sdk/unsafe`).

A plugin can now declare, in its own regenerable gamedata, an engine function the framework does not
natively wrap, and call it from TypeScript with generated types.

`s2s build` gains three behaviours when `s2script.gamedata` is set: it validates the gamedata, writes
`.s2script/gamedata.d.ts` (which augments `EngineCalls`, so arity and argument types are enforced by
the existing typecheck gate), and packs `gamedata.json` into the `.s2sp`. Declaring a `calls` section
requires `s2script.permissions: ["engine:calls"]`, which is recorded in the manifest — and is
necessary but not sufficient, since an operator must also allow-list the plugin id.

New export subpath `@s2script/sdk/unsafe`, exposing `Engine.call(name)` (a plain callable, or `null`
when the descriptor failed a load-time gate) and `Engine.status(name)` (the named reason).

Validation is deliberately strict in two places that are easy to get wrong: a `vtable` target must
carry a `validate.prologue` (a bare borrowed index is never trusted, because a wrong-but-in-range slot
silently misbehaves rather than crashing), and call names must be plain identifiers, since they are
interpolated into the generated `.d.ts` and a crafted name could otherwise inject an index signature
that defeats the type gate entirely.
