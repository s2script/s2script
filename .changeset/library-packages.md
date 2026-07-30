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

`s2s deploy` now publishes a library too: a package declaring `s2script.kind: "library"` builds
via `buildLibrary` instead of `buildPlugin`, and its `.s2lib` is posted as `library.s2lib` instead
of `plugin.s2sp` — the registry's wire format accepts exactly one of the two per upload, never
both, never neither. A library's types ride inside the `.s2lib` itself, so the plugin-only
`publishes`/types-tarball gate never runs for one. The `private: true` refusal applies identically
to both kinds, checked before anything is built or a token is even required. Workspace-mode
`s2s deploy` needed no changes at all: a workspace member declaring `s2script.kind: "library"`
that still matches the `s2script.workspace.plugins` globs flows through the exact same
plan/build/upload path as an ordinary plugin, now that the shared archive-assembly step is
kind-aware.

`s2s add <name>` now vendors a library the same way it already vendored an interface contract:
resolving a package whose `kind` is `"library"`, it downloads the `.s2lib` (not the types tarball
— a library's types live inside it) and extracts `index.js`/`index.d.ts` straight to
`.s2script/libs/<name>/` (plus a small generated `package.json` recording the vendored version and
`apiVersion`, so `s2script.libraries` resolution and the apiVersion gate have something to read),
then records the range under `s2script.libraries` instead of `pluginDependencies` — and writes no
`.npmrc` line, since a library was never an npm-installable artifact to begin with.

Authoring is now symmetric with the plugin path: `s2s create --library` (`kind: "library"` in the
interactive wizard's now two-way choice) scaffolds a library's `package.json`
(`s2script.kind: "library"`, `main`/`types` pointing at `src/index.ts`/`src/index.d.ts`, deliberately
no `pluginDependencies`) plus a real exported function and its matching `.d.ts` — refused inside a
workspace for now, since a workspace library needs to sit outside the `plugins/` glob a workspace
member's scaffold writes into, a case `s2s create` doesn't yet have a slot for. The scaffold no
longer sets `"private": true` — unlike the plugin scaffold, `s2s create --library`'s own next-step
hint points at `s2s deploy`, and `assertDeployable` refuses a private package before login or build
even runs, so the two used to flatly contradict each other.

Final review fix wave, closing the gaps the above left:

- `s2script.libraries` refuses an `@s2script/*`-scoped name outright, in both `buildPlugin` and
  `buildLibrary`. That scope is always resolved as a runtime builtin (never a bundled library) —
  tsc's exact `paths` entry for a declared library used to beat the `@s2script/*` prefix pattern
  and typecheck clean, while esbuild still externalized the name, so the bundle shipped a bare
  `require("@s2script/…")` with none of the library's code inlined and died at load.
- `s2s build` refuses a name declared in BOTH `s2script.libraries` and
  `s2script.pluginDependencies`/`optionalPluginDependencies`. The two used to typecheck clean no
  matter which one tsc's internal `paths` merge happened to pick, so the manifest's
  `compiledAgainst` hash could end up bound to a contract tsc never actually compiled against.
  Now it's a named build error instead.
- `buildLibrary` now runs the same B2 residual-rule lint gate `buildPlugin` does. A library's own
  source used to be linted by nothing at all — `lint/lint.ts`'s directory walk skips
  dot-directories and scopes to the consumer's plugin dir, so neither a vendored copy nor a
  workspace-sibling library was ever in range. Two of the four pinned rules describe hazards that
  fail silently at runtime and apply verbatim to library code once it's bundled into a consumer.
- A `kind: "library"` workspace member matched by the workspace's OWN
  `s2script.workspace.plugins` glob (rather than sitting under a `libs/*` member) is now
  sibling-resolvable by a separate consumer's `s2script.libraries`, not just buildable and
  publishable. `resolveLibrarySibling` used to search `ws.libs` only, reporting such a library
  "missing" with advice pointing at the registry for something sitting two directories away.
