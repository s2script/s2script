---
"@s2script/sdk": minor
---

Resolve `s2script.libraries` — build-time packages a plugin bundles into its own `.s2sp`

A declared library resolves from two sources, tried in order: a vendored copy at
`.s2script/libs/<name>/` (the registry-distributed case, pulled by `s2s add` and committed so
builds are reproducible offline), then a workspace sibling whose own `package.json` opts in with
`s2script.kind === "library"` (the monorepo case — re-vendoring a sibling's copy after every edit
is untenable, so it resolves straight to its own `main`/`types` on disk, no copy anywhere).

Both the typecheck gate (`s2s build`'s tsc pass) and the esbuild bundle step now resolve declared
libraries to real types and real code — never an ambient `any` stub. A declared library that
resolves to neither source is a hard build error naming the fix (`s2s add <name>`), the same
doctrine the gate already applies to a missing builtin.
