---
"@s2script/sdk": minor
"@s2script/cs2": minor
---

Generate the `ctx` hook augmentation from gamedata `hooks` descriptors (`s2s gen-hooks`), and gate
its freshness.

`packages/cs2/hooks.generated.d.ts` is new: for every gamedata-declared hook (currently
`onTerminateRound`/`onRespawn`), it emits a typed view interface (mutable params writable,
everything else — including a books-gated `EntityRef` receiver — readonly) and augments
`PluginContext` with one readonly member per `expose.ctx` namespace (`ctx.gameRules`,
`ctx.players`), so a plugin subscribing to a hook that does not exist, or one that has drifted, is a
`tsc` build failure rather than a silent no-op. `@s2script/sdk` gains the `s2s gen-hooks [--check]`
CLI command that produces it, plus a fix to the typecheck gate so a CS2 plugin sees
`@s2script/cs2`'s `ctx` augmentation even when its own source never imports a name from
`@s2script/cs2` directly (mirrors the existing fix for `@s2script/sdk/unsafe`'s plugin-declared
`EngineCalls`).
