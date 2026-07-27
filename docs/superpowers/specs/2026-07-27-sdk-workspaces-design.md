# SDK workspaces — a monorepo of many plugins that compiles and publishes — design spec

**Date:** 2026-07-27
**Branch:** `sdk/workspaces` (off `main`)
**Status:** design approved, plan pending

## 1. Problem

`examples/monorepo-plugin` demonstrates **one** plugin factored across npm-workspace packages. Its
own README says so explicitly: *"Workspace packages are a build-time factoring of **one** plugin."*
That is a useful lesson, but it is not the thing authors actually ask for — **a repository holding
several plugins that build and publish together.**

Nothing in the tooling supports that shape today:

| Need | Today |
|---|---|
| Build every plugin in a repo | No such command. `s2s build <dir>` builds exactly one. |
| Publish every plugin in a repo | No such command. `s2s deploy <dir>` deploys exactly one. |
| Add a second plugin to a repo | `s2s create` **refuses a non-empty directory**. |
| Plugin B uses sibling plugin A's interface | B must hand-maintain a byte-copy of A's `.d.ts` at `.s2script/types/<dep>/index.d.ts`. |
| Version several plugins together | Nothing. |

The only multi-plugin build that exists anywhere is `scripts/build-base-plugins.sh` — a bash loop
over `plugins/*/` with `VERSION=` stamping. That script *is* the workaround a user would otherwise
have to reinvent, which is the clearest possible evidence the capability belongs in the CLI.

The sharpest edge is the sibling-interface one, because it is exactly why someone co-locates
plugins in the first place. `contracts.ts` requires a consumer to keep a byte-copy of the
producer's published contract, and `build.ts` hashes those bytes into `manifest.compiledAgainst`.
In a monorepo the producer's contract is *right there on disk*, and copying it is pure ceremony —
worse, a copy that can silently go stale.

## 2. Goal

A directory containing many plugins is a first-class thing the SDK understands. One command builds
them all; one command publishes them all; a plugin can depend on a sibling's published interface
with no hand-copied types; shared library packages are consumed by several plugins at once; and
`s2s create` can both create a workspace and add a plugin to one.

The s2script repo's own `plugins/` tree becomes such a workspace, retiring
`scripts/build-base-plugins.sh` — 18 real plugins as the acceptance test.

**Non-goal, stated explicitly because the change makes it newly possible:** the base plugins do
**not** start publishing to the registry. See §9.1.

## 3. Findings (investigated, not assumed)

### 3.1 npm links every workspace package, declared as a dependency or not

Verified against a scratch workspace (`plugins/*`, `packages/*`, no cross-declarations at all):

```
node_modules/@w/a   -> ../../plugins/a
node_modules/@w/b   -> ../../plugins/b
node_modules/@w/lib -> ../../packages/lib
```

The symlink is free. Sibling resolution requires **no** dependency declaration and no new field.

### 3.2 `moduleResolution: "bundler"` resolves a sibling's `types` through that symlink

Verified with the repo's own `tsc` under the exact `sharedCompilerOptionsJson` the gate uses. A
consumer importing `@w/a` resolved to `plugins/a/api.d.ts` (via the sibling's `types` field), and
`@w/lib` resolved through to the lib's TypeScript source. The only diagnostic emitted was a
deliberately planted type error proving the contract's real types were in force:

```
plugins/b/src/plugin.ts(4,14): error TS2322: Type 'string' is not assignable to type 'number'.
```

**Resolve-in-place is therefore real, not hopeful.** No copy, no generated file, no `paths` entry.

### 3.3 The `any`-stub in the typecheck gate is what blocks it

`typecheck.ts` writes `declare module "<dep>";` for any declared plugin dependency lacking a
verified contract copy. That ambient shorthand **shadows** the real node resolution of §3.2. This
one behaviour is the entire blocker; removing it for siblings unblocks the feature.

### 3.4 All 18 base plugins are already `private: true`

Every package under `plugins/` and `plugins/disabled/` carries `private: true`. Meanwhile
`registry/deploy.ts` **never checks `private`** — so `s2s deploy plugins/basechat` would publish a
private package to the registry today. That is a pre-existing hole this slice closes.

### 3.5 Changesets versions private packages but refuses to npm-publish them

This is precisely the posture base plugins need, and it means adding `plugins/*` to npm
`workspaces` will make changesets *see* them — colliding with the repo's stated convention that
*"npm `@s2script/*` packages are independent (Changesets); plugins track the tag."* Resolved in §9.2.

### 3.6 All `@changesets/*` internals are already installed

`@changesets/{get-release-plan,assemble-release-plan,apply-release-plan,config,get-dependents-graph,…}`
are present in root `node_modules` as transitive dependencies of `@changesets/cli`. `s2s version`
can import them from the workspace root rather than adding SDK dependencies.

### 3.7 The one in-repo producer/consumer pair straddles `plugins/` and `examples/`

`plugins/zones` publishes `@s2script/zones`; `examples/cookbook` consumes it and keeps a hand-copy
at `examples/cookbook/.s2script/types/@s2script/zones/index.d.ts`. Making `examples/*` workspace
members to manufacture an in-repo proof is rejected — examples must stay standalone, and
`greeter-consumer`'s hand-copy is the deliberate demonstration of the *registry* path. **The repo
conversion proves build-all; the example proves sibling-resolve.**

### 3.8 `check-plugins-typecheck.sh` will break on a workspace root

It loops `examples/*/` calling `typecheckPlugin(dir)`. A workspace root has no `main`, so it throws
`no entry point`. The gate needs workspace awareness (§12).

## 4. The workspace model

### 4.1 The root marker

```json
{
  "name": "my-server-plugins",
  "private": true,
  "workspaces": ["plugins/*", "packages/*"],
  "s2script": { "workspace": { "plugins": ["plugins/*"] } }
}
```

`s2script.workspace.plugins` is a glob list naming which workspace packages are plugins; everything
else is a shared library. **The presence of `s2script.workspace` is the only thing that makes a
directory a workspace root.** Plugin `package.json` files are untouched — no new marker field.

This follows the standing convention: standard npm fields for what npm models (`workspaces`), the
`s2script` block for engine facts (which of them are plugins).

### 4.2 Discovery

`findWorkspaceRoot(from)` walks **up** from the target directory looking for the marker. If none is
found, every command behaves exactly as it does today — single-plugin mode, unchanged. This is the
backward-compatibility hinge: workspace mode is opt-in by construction.

`s2s build <one-plugin-dir>` stays single-plugin mode even inside a workspace, so per-plugin builds
keep working. `--filter` is the workspace-mode way to narrow.

### 4.3 Three discovery rules, each chosen against a named failure mode

| Situation | Behaviour | Why |
|---|---|---|
| Glob match, **no `package.json`** | Skip silently | It is just a directory — `plugins/disabled/` matches `plugins/*`. Mirrors the existing bash guard `[ -f "$d/package.json" ] \|\| continue`. |
| Glob match, `package.json`, **no entry point** | **Hard error** | Silent-skip is how you ship 14 of 15 plugins and never notice. |
| Plugin dir **not covered by npm `workspaces`** | **Hard error** | Without the symlink there is no sibling resolution, and the failure mode is a *silent degradation to `any`* rather than a diagnostic. |

### 4.4 New module layout

```
packages/sdk/src/workspace/
  workspace.ts   findWorkspaceRoot() · loadWorkspace() -> { root, plugins[], libs[] }
  glob.ts        segment matcher for * and ** (no new dependency, ~20 lines)
  graph.ts       plugin dep graph · order · cycle warn  (mirrors loader.rs::topo_order)
```

`graph.ts` is a deliberate **port of `core/src/loader.rs::topo_order`**, not an independent
implementation: edges from **hard** dependencies only (optional deps impose none), Kahn's algorithm
with a stable lexicographic tie-break, and a cycle warns rather than fails (§11). Build order and
load order must agree, so the algorithm is the same algorithm. Its unit tests mirror
`loader.rs`'s `topo_cycle_falls_back_to_name_order`.

One new bundled dev-dependency: `semver`, for the range gate (§5.2), bundled by `build.mjs` exactly
as `fflate` is, preserving the SDK's zero-runtime-dependency posture.

## 5. Build

### 5.1 Flow

1. Load workspace → build graph → topological sort (producers first) → cycle detection.
2. **Preflight the entire workspace before building anything** (§5.2).
3. Build each plugin through the existing `buildPlugin()`. Output location is unchanged:
   `<plugin>/dist/<sanitized-id>.s2sp`.
4. **Collect-all, not fail-fast.** Every plugin is attempted; the summary names each failure; exit
   non-zero if any failed. This follows *degrade per-descriptor, never crash globally* — and it
   means one broken plugin does not hide the other seventeen.

**Ordering is deliberately not load-bearing, and the spec says so.** A producer's `api.d.ts` is
*authored, not generated*, so a consumer does not need its producer built first. Topological order
buys deterministic output, publish order (§6), and cycle detection — nothing more. Implying a build
dependency that does not exist would be a lie in the design.

### 5.2 Preflight: the sibling range gate

For every plugin B declaring a sibling `pluginDependencies["@me/a"] = R`, assert
`semver.satisfies(A.version, R)`. **All violations are reported at once**, not the first:

```
2 dependency ranges do not match this workspace:
  @me/ranks  pluginDependencies["@me/shop"] = "^2.0.0"  but @me/shop is 3.0.0
  @me/warmup pluginDependencies["@me/shop"] = "^1.0.0"  but @me/shop is 3.0.0
```

This is what makes a monorepo honest: you cannot ship B declaring `^2.0.0` against an A you are
shipping alongside it at `3.0.0`.

### 5.3 Three changes inside the existing build path

1. **`typecheck.ts`** — for a declared dependency resolving to a workspace sibling plugin, suppress
   the ambient `any` stub and add no `paths` entry. Node resolution finds it (§3.2). This is the
   single line that unblocks the feature.
2. **`build.ts`** — `compiledAgainst[dep]` = sha256 of the **sibling's own `types` file**, located
   via `assertPublishesTypes` against the sibling. One copy of the bytes; drift is impossible by
   construction. esbuild `external` is unchanged — siblings are already external by virtue of being
   declared plugin dependencies.

   **Why this is correct at runtime, not merely at build time.** `loader.rs:192` fails a load when a
   consumer's `compiledAgainst[dep]` differs from the producer's published `typesSha256`. In a
   workspace both values are sha256 of *the same file on disk* — the producer hashes its `types` for
   its own `publishes` block, and the consumer hashes that identical path. They are equal by
   construction, so the drift check passes for structural reasons rather than by luck. Note that
   `loader.rs:192`'s remediation text ("refresh `.s2script/types/<dep>/index.d.ts`") names a file a
   workspace-built plugin does not have; the message is unreachable for this case, but it is worth a
   follow-up wording pass once workspaces exist in the wild.

   **Precedence when both exist.** A consumer may be a workspace sibling of its producer *and* still
   carry a stale `.s2script/types/<dep>/index.d.ts` from before the migration. `typecheck.ts` today
   prefers that copy (its `contractPaths` entry beats the `any` stub). **The sibling wins**, and the
   build warns that the copy is now ignored and can be deleted. Leaving the copy authoritative would
   reintroduce exactly the silent drift this design exists to remove.
3. **`lint.ts`** — `hasOwnConfig` checks only the plugin directory, so a workspace-root
   `eslint.config.mjs` is invisible to it and the gate silently falls to the canonical in-memory
   config. Search **upward to the workspace root**, matching how ESLint flat config resolves, which
   preserves the stated editor/build parity guarantee.

### 5.4 New flags

| Flag | Meaning |
|---|---|
| `--filter <pattern>` | Repeatable. Matches against the **package name** (`@me/shop`, `@me/*`); a pattern containing `/` that matches no package name is retried as a **path glob** relative to the workspace root (`plugins/sh*`). No match at all is an error, not an empty build. |
| `--stamp-version <v>` | Rewrite every plugin's `version` to `<v>` **and** rewrite sibling ranges to match, then build. Replaces `VERSION=` in `build-base-plugins.sh`. **`s2s build` only** — `s2s deploy` never rewrites versions. |

`--stamp-version` must rewrite sibling ranges, not just versions: stamping all plugins to `1.5.0`
would otherwise trip the §5.2 gate on every consumer.

## 6. Publish

### 6.1 Flow

1. **Build everything first and require all green.** Any build failure ⇒ publish nothing. A partial
   publish is unrecoverable state; a failed build must not leave 6 of 18 plugins live. With
   `--filter`, both the build set and the publish set are the filtered set — an unrelated broken
   plugin does not block a targeted release, and the all-green requirement applies to what is
   actually being shipped.
2. Compute a per-plugin plan:

| Condition | Plan entry |
|---|---|
| `private: true` | `skip (private)` |
| Version already on the registry | `skip (already published)` |
| Otherwise | `PUBLISH` |

3. Print the plan; confirm interactively. `--yes` / `--ci` skip the prompt; `--dry-run` prints and
   exits.
4. Upload in topological order, so a consumer is never live against an absent producer.

```
$ s2s deploy

  building 4 plugins (dependency order)
    ✓ @me/shared-api  1.2.0    ✓ @me/ranks   0.4.0
    ✓ @me/shop        2.0.1    ✓ @me/warmup  1.0.0

  plan:
    @me/shared-api 1.2.0   skip (already published)
    @me/shop       2.0.1   PUBLISH
    @me/ranks      0.4.0   PUBLISH
    @me/warmup     1.0.0   skip (private)

  Publish 2 plugins to s2script.com? (y/N)
```

### 6.2 The registry is the authority; the client check is a courtesy

The already-published check uses `client.resolve(name, exactVersion)`. It exists **only to render a
legible plan**. A duplicate-version rejection encountered mid-fan-out is reported as `skip`, not
failure — which makes re-running after a partial failure safe by construction. If the registry is
unreachable for the check, the plan degrades to "assume unpublished" and the server decides.

### 6.3 `private` is enforced in single-plugin mode too

`s2s deploy <dir>` on a private package errors with a named reason, closing the §3.4 hole.

## 7. Version — `s2s version`

Changesets cannot see `s2script.pluginDependencies`, so it cannot cascade a bump to a dependent
plugin. Rather than reimplementing its dependents algorithm:

1. Read the real workspace packages.
2. Build an **in-memory mirror** injecting each plugin's sibling `s2script.pluginDependencies` as
   `devDependencies`. Changesets now sees the s2script graph through a field it already
   understands. **Nothing fake is ever written to disk.**
3. Hand the mirror to `assembleReleasePlan` → dependents cascade correctly.
4. Apply that plan against the **real** packages via `applyReleasePlan` (versions + CHANGELOGs; it
   finds no sibling edges in real dependency fields, so it correctly no-ops there).
5. One additional pass rewrites `s2script.pluginDependencies` ranges — `@me/a` 2.0.1 → 3.0.0 means
   B's `^2.0.0` → `^3.0.0` — governed by the existing `updateInternalDependencies` config.

**Coupling and its mitigation.** `@changesets/*` internals are not a stable public API; this was
raised during design and accepted deliberately. Coupling is confined to two entry points, both real
published packages. The modules are imported **dynamically from the workspace root's**
`node_modules` (§3.6), so the SDK gains no runtime dependency and the user's own changesets is what
runs. The resolved version is checked against a supported range and fails with a named reason
otherwise — never a silent drift.

## 8. Scaffolding

`s2s create --workspace <dir>` writes the root: `package.json` (both `workspaces` and
`s2script.workspace`), `tsconfig.base.json` built from the existing `sharedCompilerOptionsJson`
literal, a root `eslint.config.mjs`, `.changeset/config.json`, `.gitignore`, and empty `plugins/`
and `packages/` directories.

`s2s create <name>` run **inside** a workspace detects it and writes `plugins/<name>/` with a
*minimal* `package.json` — no duplicated devDependencies (they live at the root), no per-plugin
eslint config, and a `tsconfig.json` extending the root. The existing "target directory is not
empty" check still applies, now to the new subdirectory. Interactively, the wizard announces the
detected workspace and offers to add a plugin to it.

## 9. Dogfood: converting `plugins/`

### 9.1 Base plugins keep shipping in the runtime zip

**`private: true` = built, never published.** That is npm's own semantics and changesets' (§3.5),
so nothing is invented. All 18 base plugins already carry it (§3.4), therefore:

- `s2s build` at the repo root builds all 18 — the acceptance test that justifies the conversion.
- `s2s deploy` at the repo root publishes **nothing**: the plan comes back all-skip and the command
  is a named no-op, not a surprise. This matters because a push to `main` auto-fires release
  automation.
- `package-release.sh` → runtime zip is entirely unchanged.
- If base plugins later become registry-distributed (CLAUDE.md's eventual intent), it is a
  deliberate one-field change per plugin, decided on its own merits.

The deploy plan **lists private plugins explicitly as skipped**, so "nothing happened" is always
legible.

### 9.2 Resolving the two-version-mechanism conflict

Adding `plugins/*` to npm `workspaces` makes changesets version them (§3.5), colliding with
*"plugins track the tag."* Resolution: add `plugins/*` to `.changeset/config.json` `ignore`, and
keep `--stamp-version` (§5.4). The SDK supports both models; each repo picks one. This repo stamps
its plugins from the release tag and uses changesets for `packages/*`. A user workspace uses
changesets for plugins.

### 9.3 Concrete changes

| File | Change |
|---|---|
| root `package.json` | `workspaces` gains `plugins/*`, `plugins/disabled/*`; add `s2script.workspace.plugins` |
| `.changeset/config.json` | `ignore` the plugin packages |
| `scripts/build-base-plugins.sh` | Becomes a thin shim: `s2s build --stamp-version "$VERSION"` |
| `scripts/check-plugins-typecheck.sh` | Expand a workspace root into its plugins (§3.8) |
| `scripts/package-release.sh` | Unchanged — it globs `plugins/*/dist/*.s2sp`, which is unchanged |

The shim is kept rather than deleted deliberately: it is the smallest possible diff on the
auto-publishing release path, which is the part least worth churning.

**`package-lock.json` is the hazard.** Adding workspaces rewrites it, and `CI=1 make ci-js` runs
`npm ci` as the drift guard. Additive workspace nodes via `npm install --package-lock-only` should
not disturb existing integrity hashes — this gets verified explicitly, and the lockfile is **never**
`rm`'d and regenerated (that strips integrity hashes that cannot be refetched here).

## 10. The example

`examples/monorepo-plugin` is **replaced** by `examples/monorepo/` — a real workspace containing:

- a producer plugin publishing an interface,
- a consumer plugin depending on it **with no `.s2script/types/` copy**,
- a `packages/shared` library bundled into both.

One example carries all three lessons: the old one's "workspace libs are a build-time factoring of
one plugin", the new "several plugins, one repo, one build", and the visible contrast against
`greeter-consumer`, which keeps its hand-copy *because* its producer is a registry dependency
rather than a sibling.

As with today's example, `node_modules` symlinks are committed as git mode `120000` — a nested
workspace receives no `npm install` from the root. The existing Windows `core.symlinks` caveat in
the current README carries over.

## 11. Error handling

Governed by the standing rule — *degrade per-descriptor, never crash globally*:

- Preflight reports **every** violation at once: ranges, missing entry points, uncovered directories.
- Per-plugin build failures are collected, not fail-fast; the summary names each and exits non-zero.
- **A `pluginDependencies` name that is not a workspace sibling keeps today's exact behaviour** —
  verified contract copy, or `any` stub. No error. This is the compatibility hinge that keeps every
  existing plugin building unchanged.
- A sibling declaring `publishes` but no `types` → named error **at the consumer's build**, because
  it is the consumer that cannot compile.
- A sibling declaring **no `publishes` at all**, named as a `pluginDependency` by a consumer →
  named error at the consumer's build: you cannot depend on an interface nobody publishes. Without
  this, resolution would fall through to the sibling's `main` (its *implementation* source) and
  typecheck against internals that never cross the context boundary.
- **A cycle in the interface graph → WARN and fall back to lexicographic order, never an error.**
  This mirrors `core/src/loader.rs::topo_order` exactly, which warns and falls back for precisely
  this case: *"degrade-never-crash — the hard-dep proxy is lazy, so a mis-ordered pair still runs,
  throwing `InterfaceUnavailable` only at call time."* A build that hard-errored here would refuse
  to produce a plugin set the engine is deliberately designed to run.

## 12. Testing and gates

**Unit tests** into the existing `packages/sdk/test/*.test.mjs` suite (already run by `ci-js.sh`),
over fixture workspaces under `packages/sdk/test/fixtures/`:

- glob matcher; workspace discovery walk-up; all three §4.3 rules
- graph topological order and cycle detection
- the §5.2 range gate, including multi-violation aggregation
- plan computation across `private` / `already published` / `PUBLISH`
- the §7 in-memory mirror cascading a bump to a dependent, and asserting **no mirrored
  `devDependencies` are written to disk**

**New gate `scripts/check-workspace-build.sh`**, added to `ci-js.sh` (never to the workflow YAML —
local green means CI green): builds `examples/monorepo/`, asserts the artifact count, **and asserts
the consumer's `manifest.compiledAgainst` equals sha256 of the producer's `api.d.ts`.** Without that
last assertion the feature could silently regress to an `any` stub and every other gate would still
pass.

**Updated gate** `check-plugins-typecheck.sh` — expand a workspace root into its plugins (§3.8).

**Live gate**, per the standing slice cadence: build all 18 base plugins through the new path,
deploy to the Docker CS2 server, confirm they load.

## 13. Decisions

| # | Decision | Rationale |
|---|---|---|
| 1 | Root globs, not a per-package marker | Plugin `package.json` files stay untouched; one place to look; mirrors the repo's own `plugins/` vs `packages/` split |
| 2 | Resolve sibling contracts in place | §3.1–3.2 verified it works with zero config; one copy of the bytes makes drift structurally impossible |
| 3 | Build all, skip already-published | Re-running after a partial failure is safe by construction |
| 4 | `private: true` = built, never published | npm's and changesets' own semantics; keeps base-plugin distribution an explicit future decision |
| 5 | `s2s version` owns the full plan via an in-memory mirror | Correct dependent cascade without reimplementing changesets or polluting the authoring format |
| 6 | Collect-all rather than fail-fast on build | One broken plugin must not hide seventeen others |
| 7 | Ship as one slice / one PR | Chosen deliberately after a two-slice split was proposed and declined |
| 8 | Port `loader.rs::topo_order` rather than reimplement ordering | Build order and load order must agree; a cycle warns in both, so the SDK is never stricter than the engine |
| 9 | Sibling contract beats a stale local copy | The copy is the drift vector this design removes; leaving it authoritative would defeat the point |

## 14. Out of scope

- **Base plugins on the registry** (§9.1) — a separate, deliberate decision.
- **Nested workspaces** — one level only. `examples/monorepo/` is standalone, not a member of the
  root workspace (§3.7).
- **Non-npm workspace managers** (pnpm/yarn/bun workspace protocols) — `workspace:*` specifiers are
  not interpreted. npm workspaces only, matching what the repo uses.
- **Publishing shared libs to npm** — workspace libraries are bundled into each `.s2sp`; they are
  build-time factoring, never runtime dependencies.
- **Workspace modes for the remaining commands.** `add`, `install`, `login`, `config`, and the
  `gen-*` codegen commands keep their exact single-plugin semantics. Run at a workspace root where a
  plugin directory is required, they error naming the workspace and asking for a plugin (or
  `--filter`) — a named refusal, never a silent no-op or an accidental fan-out. Fanning `s2s config`
  out across a workspace is plausible future work; it is not designed here.
