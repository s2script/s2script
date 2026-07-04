# Slice 5E.1 — the `tsc` typecheck gate (design)

**Goal:** Make `s2script build` **typecheck** each plugin against the shipped engine `.d.ts` contract
(full `strict`) and **fail the build** (produce no `.s2sp`) on any type error — closing the charter's
"typecheck-gate every load and reload against the shipped `.d.ts`" requirement at the build/reload edge.

**Status:** design approved (strictness = full `strict`, user-confirmed).

**Branch base:** `main` (… + 5D.3 merged).
**Cadence:** subagent-driven, merge-to-main-locally. This slice's gate is CI/build-time (node), **not** a
Docker CS2 live gate — the deliverable is a build-tool behaviour, not runtime engine code.

---

## 1. What exists vs. the gap

- **Load-time contract check — DONE (Slice 4):** `core/src/loader.rs::api_version_compatible` refuses a
  plugin whose declared `apiVersion` major ≠ the host's (`WARN` + skip, degrade-never-crash). That is
  the charter's "again at load." **Untouched by this slice.**
- **Build-time typecheck — THE GAP:** `packages/cli/src/build.ts` bundles with **esbuild**, which
  transpiles but never typechecks, and marks `@s2script/*` external. Plugins have **no** `@s2script/*`
  type resolution (no tsconfig, no `node_modules` symlink), so authored `.ts` ships un-typechecked
  against the very `.d.ts` contract the framework semver-governs. This slice fills that.

Because the dev file-watch reload *rebuilds* the `.s2sp`, a failing typecheck → no new `.s2sp` produced
→ the running version is never replaced. So "a failing reload leaves the running version untouched"
falls out of the build gate for free — no separate reload mechanism is needed.

---

## 2. Approach — the TypeScript compiler API, in-process

Add `typescript` as a `packages/cli` dependency and run a programmatic `--noEmit` **semantic +
syntactic** diagnostic pass (no shelled-out `tsc`, no on-disk tsconfig). Rationale: programmatic
diagnostics + custom module resolution, and it's `node:test`-able exactly like the existing
`schemagen`/`eventgen`/`navgen` (pure unit, no child process). A new unit `packages/cli/src/typecheck/`
owns it (mirrors the codegen module layout).

**Interface:** `typecheckPlugin(pluginDir, opts) → { ok: boolean, diagnostics: FormattedDiag[] }` where a
`FormattedDiag` carries `{ file, line, col, code, message }`. `buildPlugin` calls it FIRST (before
esbuild); on `!ok` it throws an error whose message is the formatted diagnostics (`file:line:col — TSxxxx:
message`), so the CLI exits non-zero and writes no `.s2sp`.

---

## 3. Module resolution + the compile set

- The CLI resolves the repo's `packages/` dir relative to its OWN module location (`import.meta.url`),
  so the gate works regardless of the plugin's cwd.
- Synthetic `compilerOptions`: `strict: true`, `noEmit: true`, `moduleResolution: "bundler"` (or
  `"node16"`), `target/lib: es2020`, `types: []` (no ambient `@types/node` — plugins are engine-sandboxed,
  not Node), and
  `paths: { "@s2script/*": ["<repo>/packages/*/index.d.ts"] }` + `baseUrl` so `@s2script/cs2` →
  `packages/cs2/index.d.ts` (which re-exports its `*.generated.d.ts` via normal relative resolution).
- **Root files:** the plugin's entry (`s2script.main`/`main` from its `package.json`) — tsc follows the
  import graph and checks the plugin's own `.ts` against the `@s2script/*` declaration surface. The
  `.d.ts` files themselves are declarations (surface-checked, not re-verified).

### 3.1 The injected-globals ambient `.d.ts` (`console`)

Plugins use `console` as a **global** (the examples call `console.log` without importing; the prelude
injects `globalThis.console`) — but `lib: es2020` + `types: []` does not declare it, and pulling `lib:
dom` would falsely admit `window`/`document`/etc. that the sandbox does NOT provide (typecheck-passing
code that crashes at runtime). So this slice ships a small **plugin-facing ambient globals** declaration
(new `packages/globals/globals.d.ts`, e.g. `declare const console: { log(...a: any[]): void; error(...):
void; warn(...): void; info(...): void };`) that models EXACTLY the globals the engine injects, and the
typecheck always includes it as a root file. It is the home for any future injected global. (The
existing import-form `@s2script/console` stays; its `typeof globalThis.console` is adjusted to not
require a browser/node lib — a hand-written `Console` interface the global decl reuses.)

## 4. Inter-plugin dependencies (a plugin's `pluginDependencies`)

A plugin importing another plugin's published interface (e.g. `@demo/greeter`) has no shippable `.d.ts`
yet. To keep the gate usable for such plugins WITHOUT full inter-plugin typing: the typecheck injects an
in-memory **ambient stub** — the empty-body form `declare module "@demo/greeter";` (per declared dep +
the `optional*` deps) — which makes ANY import shape from that module resolve to `any` (named/default/
`require`). So the import resolves and the plugin's *engine* usage is still fully checked; the
inter-plugin call surface is `any`. **Full inter-plugin interface typing is
deferred** (it requires the interface *producer* to emit a consumer `.d.ts` — a separate feature).

## 5. Strictness (confirmed: full `strict`)

`compilerOptions.strict = true` — the strongest guardrail (`strictNullChecks`, `noImplicitAny`,
`strictFunctionTypes`, …). The shipped `.d.ts` is `T | null` everywhere by design (entity/field/player
accessors), so strict null-checking is exactly the value: it forces authors to handle the `null` the
safety model produces. The baseline is FIXED by the gate (a plugin cannot loosen it) — it is the shipped
contract, not a per-plugin preference. (No per-plugin tsconfig customization this slice — YAGNI.)

## 6. The examples are the forcing function

Every plugin under `examples/*` (incl. the demo) MUST now typecheck cleanly under full strict, or its
build breaks. This **validates the shipped `.d.ts` surface is correct + complete** — any example that
fails means the example OR a `.d.ts` is wrong, and fixing it is part of this slice. Expect small,
honest fixes (null-guards the `T|null` API already requires; a stray `any`). This is a feature, not
friction: the gate's first job is to prove the API types are usable.

## 7. A CI gate

`scripts/check-examples-typecheck.sh` runs the typecheck over every `examples/*` plugin (build or a
direct `typecheckPlugin` call) and fails if any has type errors — joining the existing `check-*.sh`
gates so a `.d.ts` regression that breaks the examples is caught in CI, not just at a manual build.

## 8. Testing

- **Unit (`node:test`, `packages/cli/test/typecheck.test.mjs`):** a fixture plugin dir with a deliberate
  type error (e.g. `const x: number = pawn.health` where `health` is `number | null`) → `ok:false` +
  a diagnostic at the right line; a clean fixture → `ok:true`, no diagnostics; an `@s2script/*` import
  resolves (no "cannot find module"); an inter-plugin dep import resolves via the ambient stub.
- **Integration:** `s2script build <clean-example>` succeeds + emits `.s2sp`; `s2script build
  <broken-fixture>` exits non-zero + emits NO `.s2sp`.
- **The examples build** (the CI gate) is green after any example fixes.

## 9. Rough task decomposition (~4)

1. The injected-globals `packages/globals/globals.d.ts` (`console`) + adjust `@s2script/console` to a
   lib-free `Console` interface. Then the `typecheck/` unit: `typecheckPlugin` (compiler API, full
   strict, `@s2script/*` paths, the globals root file, ambient inter-plugin stubs, formatted
   diagnostics) + `typescript` dep + unit tests (clean/broken/`@s2script/*`-resolves/inter-plugin-dep-
   resolves/`console`-global-resolves fixtures).
2. Wire into `buildPlugin` (typecheck first; throw formatted diagnostics; no `.s2sp` on failure) +
   integration tests (clean → emits, broken → no emit + non-zero).
3. Make every `examples/*` plugin pass full strict (the forcing function — fix examples and/or `.d.ts`)
   + `scripts/check-examples-typecheck.sh` CI gate.
4. Docs (README "the typecheck gate" + CLAUDE Current state) + the full gate sweep.

## 10. Explicitly out of scope (do not build ahead)

Full inter-plugin interface typing (producer-emitted consumer `.d.ts`); a runtime/load-time tsc (the
load gate is the existing apiVersion check — a version check, not tsc); per-plugin tsconfig
customization / loosening the baseline; incremental/watch-mode typecheck caching; `.d.ts` API-docs
generation; the `tsc` version pin treadmill (note as a TODO). Note later needs as TODOs and stop.
