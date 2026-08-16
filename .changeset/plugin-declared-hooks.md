---
"@s2script/sdk": minor
---

Plugins can declare inbound engine hooks in their own gamedata.

`s2s build` accepts a `hooks` section and the `engine:hooks` permission (separate from
`engine:calls`). Types are generated into `.s2script/hooks.d.ts` as an `EngineHooks`
augmentation of `@s2script/sdk/unsafe`. Subscribe with `Engine.hook(name)` — the inbound
sibling of `Engine.call`. The owner is always the calling plugin; you cannot attach to
another plugin's detour through this factory. Game-package hooks still hang off `ctx.*`.

The thunk vocabulary is unchanged and still closed (`this_void`, `this_f32_i32_i32_i32`,
`this_f32_i32_i64_i64`). A hook target still requires a non-empty `validate`.
