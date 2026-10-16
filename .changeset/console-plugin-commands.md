---
"@s2script/sdk": minor
---

`Plugins.install(names, { with })`, `Plugins.update(names?)`, `Plugins.remove(names)` and
`Plugins.outdated()` in `@s2script/sdk/plugins` manage the server's `plugins.json` from inside the
runtime (no Node.js needed). They back the new `sm plugins install|update|remove|outdated` console
commands in basecommands. Gated by the `plugins:manage` permission, which defaults to
`@s2script/basecommands` only.
