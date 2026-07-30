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

`s2s build` also now refuses a plain npm runtime `dependency` outright. Plugins run in bare V8 —
no `fs`, no `net`, no `process`, no `Buffer` — so a registry-installed package that touches a Node
API used to bundle green and only fail on a live server, far from its cause; that failure now
happens at build time, naming the offending package and pointing at `s2script.libraries`
(`s2s add <pkg>`) instead. `@s2script/*` packages and workspace-linked code (a `file:`/`workspace:`
range, or a package npm symlinked in from a workspace) are unaffected — only a real registry
download is gated.
