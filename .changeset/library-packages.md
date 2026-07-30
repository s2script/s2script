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

A package declaring `s2script.kind: "library"` now actually builds: `s2s build` (via the new
`buildPackage` dispatcher) produces `dist/<sanitized-name>.s2lib` instead of a `.s2sp` — a zip of
exactly `manifest.json` + `index.js` + `index.d.ts`, types required because a library with no
published types is unusable to a consumer's typecheck gate. `@s2script/*` imports stay external
through the library's own bundle (they resolve in the *consumer's* context at runtime), but a
library's own declared `s2script.libraries` are inlined into its bundle, so a published `.s2lib` is
one self-contained file with no transitive graph. A library may not declare
`pluginDependencies`/`optionalPluginDependencies`/`publishes` — those are a loaded plugin's runtime
contracts, and build-time code has no ledgered lifecycle to hang them on; `buildPlugin` itself now
refuses a library outright, naming `buildLibrary`/`buildPackage` as the fix, so a direct caller can
never get the wrong artifact. Workspace-mode `s2s build` now also builds every workspace member
declaring `s2script.kind === "library"` (previously invisible to it — such a member sits in the
workspace's structural "everything that isn't a declared plugin" bucket, not its plugin globs) to
its own `.s2lib`, ahead of the plugins for a sensible log; a workspace sibling library is still
resolved from source by its consumer's own build, so this build order is not a dependency.
`--filter` selects across declared libraries too: naming one builds just it, and an unfiltered run
still builds every declared library — a filtered plugin build never needs its sibling library
built (it resolves from source either way), so an unrelated library is never built, and never
allowed to fail, a run meant to isolate one target.
