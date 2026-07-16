# Packaging consolidation, review debt, and the npm name — design spec

**Date:** 2026-07-16
**Status:** design (pending approval) → implementation plan next
**Scope:** finish the packaging story the contract-grammar stack (PRs #36–#41) left open. Three independent parts: **(A)** claim the `s2script` npm name, **(B)** close the six review-debt findings from `/code-review high` on the contract-grammar stack, **(C)** consolidate the 29 `@s2script/*` types-only stubs into a single `s2script` package with subpaths. Amends the distribution spec at `docs/superpowers/specs/2026-07-15-plugin-contract-distribution-design.md` — specifically retires its §10 "stub consolidation" and "risks" debt.

**Depends on:** the contract-grammar stack (#36–#41) being the baseline. This spec assumes those defects (see that stack's plan "Amendments") are already fixed.

## 1. Goal

Three separable goals, one narrative — the packaging layer as it will look when `s2script.com` launches:

- **(A)** `s2script` (unscoped) is unclaimed on npm today. `npx s2script build` 404s cleanly. Publishing anything without a `bin` converts the honest 404 into `could not determine executable` — strictly worse. **Claim it now, with a real forwarding bin**, so the name can't be squatted and `npx s2script build` works immediately.
- **(B)** Six findings surfaced by `/code-review high` on the contract-grammar stack. They are **unrelated to consolidation** and should not wait behind it. One touches the frozen manifest shape and needs care; the rest are mechanical.
- **(C)** The `@s2script/*` npm scope conflates two unlike things that look identical at the import site: always-present **builtins** (`@s2script/entity`, backed by a Rust prelude) and presence-conditional **interfaces** (`@s2script/zones`, a plugin's published API). A consumer cannot tell them apart, and that conflation is what makes one review finding (`typecheck.ts:76`) structurally unfixable. Consolidation splits them into two honest namespaces, and it is the **only** thing that fixes that finding at the root.

**Consolidation is ergonomics, not correctness** — every `@s2script/*` package is types-only (`files: ["index.d.ts"]`), the runtime is Rust preludes on `globalThis.__s2pkg_<name>`, and the CLI marks `@s2script/*` esbuild-external, so **zero package bytes reach a `.s2sp`**. Tree-shaking was never the motivation. The win is DX (one install, one version) and release ergonomics (one package to publish and version). But the distribution spec's §10 is explicit: if consolidation happens, it **must land before the registry launches, never after** — the `.s2sp` bytes and consumer imports it changes are the frozen surface.

## 2. Established facts — verified against the code, do not re-derive

- **Every `@s2script/*` package is types-only.** Runtime resolution is `core/src/v8host.rs:4065 s2require`: strip `@s2script/`, look up `globalThis.__s2pkg_<rest>`. Generic — no hardcoded module list. `@s2script/cs2 → __s2pkg_cs2` rides the same path.
- **`BUILTIN_MODULES` (`core/src/loader.rs:78`) has exactly two call sites** (`loader.rs:121,125`), both in `imports_from_manifest`, both doing nothing but `continue` — its entire job is skipping builtins from the ledger. The list is stale (missing `translations`, `usercmd`, `net`).
- **The staleness is latent, not live.** Nothing in the repo declares `translations`/`usercmd`/`net` as a `pluginDependency`, and if something did, `__s2require` tries the prelude first, so the phantom would never be called; an unresolved `Kind::Hard` dep is lazy (`interfaces.rs:139 call_target_inner` returns `Unavailable` at call time — no load is refused). So the stale list is a hygiene problem, not a bug. **(This corrects the original framing: Fork 1's prize is honesty + a deletable list, not a correctness fix.)**
- **The derived manifest carries only `pluginDependencies` and `optionalPluginDependencies`** (`build.ts:113`). npm `dependencies` never reach it. This is the mechanism that makes Fork 1 work (see §5).
- **`typecheck.ts:76`'s filter is `isBuiltinOnDisk`** — `d.startsWith("@s2script/") && existsSync(packagesDir/<name>/index.d.ts)`. A typo'd builtin (`@s2script/frmae`) is not on disk → falls through to the ambient `declare module` stub → types as `any` instead of TS2307. Verified end-to-end.
- **`s2script` (unscoped) is unclaimed on npm; `@s2script` scope is owned (user `gkh`).** `@s2script/cli@0.2.0` already has `bin: { s2script: "dist/cli.js" }` and depends on `esbuild`, `adm-zip`, `typescript`.
- **Blast radius (re-counted, larger than the earlier ~188/50 estimate):** **230 import sites across 77 files**; **9 `__s2require` string literals** in `games/cs2` (`pawn.js`, `weapon.js`, `nav.generated.js`, `schema.generated.js`) that are compiler-invisible — a miss degrades to `pawn.origin → null` silently at runtime; **11 cross-package `.d.ts` imports** in `packages/*/index.d.ts`; the `tsconfig.base.json:12` `paths` twin (`"@s2script/*": ["*/index.d.ts"]`); the root `package.json` is **already named `"s2script"` (private)** and must be renamed to free the name. `check-core-boundary.sh` and `check-plugins-typecheck.sh` are crate/path-based and need no change.
- **CLAUDE.md's "Never overload npm's `exports`"** governs *plugin manifests*, not a published types package — confirmed. Adding a subpath `exports` map to `s2script` is out of its scope.

## 3. Decisions (forks resolved)

| Fork | Decision | Rationale |
|---|---|---|
| **1 — builtins → npm `dependencies`?** | **Yes.** Move builtins from `s2script.pluginDependencies` to npm `dependencies`. | A consolidated `s2script` *is* an npm build-dep, so CLAUDE.md's "`dependencies` = npm build-deps only" puts it there. Because the derived manifest never carries npm `dependencies` (§2), builtins **vanish from the manifest**, `imports_from_manifest` never sees them, and `BUILTIN_MODULES` becomes genuinely unemployed (deletable). This is also what lets the typecheck filter become honest (§6.4). |
| **2 — does `s2script` ship the CLI bin?** | **Yes — types + CLI** (the `typescript`/`tsc` model). | `npm i -D s2script` gives the subpath types *and* `npx s2script build`. Every plugin author needs the CLI to build anyway; one dep, one version. Kills the npx footgun definitively. `@s2script/cli` is deprecated/aliased. |
| **3 — claim the name now or at consolidation?** | **Now, with a real forwarding bin** (Part A). | A defensive claim prevents squatting; the forwarding bin makes `npx s2script build` work today. A types-only placeholder is the one thing to avoid — it breaks `npx`. Part C later replaces the package contents. |
| Decomposition | **One spec, three independently-stackable parts.** | A/B/C have no build-order dependency on each other except "A frees the name C fills." Each is its own Graphite stack, planned + implemented via its own workflow. |

**Settled going in (not re-litigated):** `@s2script/cs2` stays a **separate scoped package** (game → core, never core → game); **no flat root barrel** (`import { Chat } from "s2script"`) — subpaths only; **no changesets `fixed` lockstep**.

---

## Part A — claim the `s2script` npm name

**One PR. Independent of B and C. Do first.**

Publish `s2script@0.0.x` with a **real forwarding bin** — a tiny `bin: { s2script }` shim that re-execs the installed `@s2script/cli` (or vendors its entry) so `npx s2script build` resolves and runs today. The package is otherwise minimal.

- **Why a bin, not types-only:** a types-only package at `s2script` turns today's honest `npx s2script build` 404 into `could not determine executable` — a worse failure. Any placeholder MUST carry a bin.
- **Squat prevention:** `s2script` is unclaimed; the name is the one irreversible asset here. Claiming it early is cheap insurance.
- **Relationship to C:** Part C replaces this package's contents with the real types + CLI (§Part C). Part A only plants the flag and wires the bin; it does not move any `.d.ts` files.
- **npm trusted-publishing** re-bootstraps per package name — Part A is where `s2script`'s publish provenance is first set up.

**Gate:** `npx s2script build` in a clean directory resolves the published bin and runs (no `could not determine executable`).

---

## Part B — close the six review-debt findings

**Independent of A and C. Land before C** so the two `build.ts` findings don't collide with C's `build.ts` edits. One Graphite stack, roughly one PR per finding.

| # | Site | Finding | Note |
|---|---|---|---|
| B1 | `publishes.ts:99` | `isConcreteVersion` trims for the **check**, but `derivePublishes` stamps the **untrimmed** value — `" 1.0.0 "` reaches the manifest with surrounding space. | Trim at the stamp, not just the check. |
| B2 | `build.ts:132` | A multi-interface `publishes` map gives **every** entry the same `typesSha256` (there is one `types` file), so the hash can't identify which contract belongs to which interface. | **Touches the frozen manifest shape (distribution spec §9).** Isolate in its own PR; the fix corrects a just-frozen shape, so call that out in the PR body. Needs a real per-interface hash source or an explicit single-contract constraint. |
| B3 | `build.ts:118` | The range rejection fires **after** tsc + esbuild, defeating the gate's fail-fast purpose. | Move the check before the expensive steps. |
| B4 | `build.ts:48` | `package.json` is read + parsed twice; `readFileSync` imported twice. | Read once, reuse. |
| B5 | `interfaces.rs:63` | 17 discarded `Result`s from the now-fallible publish path. | Handle or explicitly `let _ =` with reason; a silent drop hides a publish failure. |
| B6 | `v8host.rs:8607` | `shutdown()` doesn't clear `PLUGIN_PUBLISHES` / `UNDECLARED_PUBLISHES`, unlike every other registry thread_local. | Clear them for teardown symmetry. |

**`typecheck.ts:76` is deliberately excluded** — it is Part C's acceptance test (§6.4), not loose cleanup, because consolidation is its structural fix.

**Gate:** existing gate suite green per PR; a regression test where meaningful (B1: `" 1.0.0 "` reaches the manifest trimmed; B2: a two-interface map produces two *distinct* `typesSha256` values).

---

## Part C — consolidate `@s2script/*` into one `s2script` package

The tentpole. Depends on Part A having claimed the name.

### 6.1 The namespace model

Two namespaces, discriminated by **shape**, not by a disk check:

| Import shape | Meaning | Resolves how | Typo behavior |
|---|---|---|---|
| `s2script/<cap>` (unscoped subpath) | a builtin | path-mapping / `exports` → the one `s2script` package's subpath `.d.ts` | **TS2307** (miss = real error) |
| `@s2script/cs2`, `@s2script/<game>` (scoped, first-party) | a game package | real installed package `.d.ts` (kept as today via the `@s2script/*` path) | TS2307 |
| `@scope/name` in `pluginDependencies` | an inter-plugin interface (incl. first-party `@s2script/zones`) | `.s2script/types/<name>/` if fetched, else ambient stub | `any` (unknowable until fetched) |

At runtime, `s2require` resolves `s2script/<cap> → __s2pkg_<cap>` — one additive strip-path alongside the existing `@s2script/` one. `@s2script/cs2` keeps its current path untouched.

### 6.2 The `s2script` package — layout and versioning

**One package, two faces (the `typescript`/`tsc` model):**

```
packages/s2script/
  package.json     name "s2script", exports map (one subpath per capability),
                   bin { s2script → ./dist/cli.js }, deps { esbuild, adm-zip, typescript }
  entity.d.ts      ← moved from packages/entity/index.d.ts
  frame.d.ts       ← moved from packages/frame/index.d.ts
  …                (29 capability .d.ts files, one per remaining stub)
  globals.d.ts     ← moved from packages/globals/globals.d.ts (the ambient globals the gate injects)
  src/…            ← the CLI, absorbed from @s2script/cli
```

- **Physically move** the stub files into one dir (one package = one dir — honest, greppable) rather than assembling at publish. The 11 cross-`.d.ts` imports rewrite `@s2script/math` → `./math` (relative, internal); the `exports` map gates only *external* subpath access, so internal relatives resolve regardless.
- **Two faces of one package:** a plugin does `import { Entity } from "s2script/entity"` (esbuild-external, prelude at runtime) *and* the author runs `npx s2script build` (the bin). Same `npm i -D s2script`.
- **Versioning — the one redundancy, named and kept:** a plugin declares `dependencies: { "s2script": "^1.x" }` (the `.d.ts` contract it compiled against) **and** keeps `s2script.apiVersion` (the host ABI it loads against). Two real axes — types vs runtime — that move together in practice, exactly as `typescript@5.4` is both the compiler and `lib.d.ts`. **Not collapsed.** The per-capability version pins builtins carry today (`@s2script/entity: ^0.2.0`) are lost, but those are fictional — all builtins ship in one runtime zip, versioned by the host — so one `s2script` version is *more* honest.
- **`@s2script/cs2`** stays a separate scoped package and gains `dependencies: { "s2script": "..." }`, since its `.d.ts` imports `s2script/entity`, `s2script/math`, `s2script/trace`, `s2script/events`.

### 6.3 The dual-prefix transition — how a 230-site rename becomes a stack

A hard-cut rename of 77 files cannot be both atomic and small — after the resolution mechanism flips, every un-migrated import breaks. The enabling trick is a **dual-prefix transition**: teach the mechanism both spellings, migrate consumers in batches, then remove the old spelling once nothing uses it. The runtime cooperates for free — `s2require` resolving both `@s2script/entity` and `s2script/entity` to `__s2pkg_entity` is purely additive.

**Phase 1 — publish `s2script` + dual-resolve (one PR).**
- Create `packages/s2script/` (moved `.d.ts` + absorbed CLI), fill the package created in Part A.
- Add `s2script/` stripping to `s2require` (`v8host.rs:4065`), alongside the existing `@s2script/`.
- Teach the typecheck gate, esbuild-external list, and `tsconfig.base.json` paths to accept **both** prefixes.
- No consumer changes yet — fully backward-compatible, CI green.

**Phase 2 — migrate consumers in batches (N small PRs).**
- One PR per plugin (or a few), rewriting `@s2script/<builtin>` → `s2script/<builtin>` and moving builtins from `s2script.pluginDependencies` to npm `dependencies`.
- Each PR atomic because both prefixes still resolve.
- **`games/cs2`'s 9 `__s2require` literals get their own PR**, gated by the live Docker CS2 gate (`pawn.origin != null`) — a missed literal fails silently, so CI alone can't prove it.

**Phase 3 — remove the legacy builtin prefix (one PR).**
- Delete the 29 stub packages and `packages/globals`.
- Delete `BUILTIN_MODULES` from `loader.rs` (now unemployed — see §6.4).
- Narrow the typecheck filter to the honest scope-based rule (§6.4).
- Rename the private root `package.json` off `"s2script"`.
- **Gate:** a grep proves zero `@s2script/<builtin>` imports survive. `@s2script/cs2` and `@s2script/zones` are untouched — they legitimately keep the scope.

### 6.4 Why Fork 1 deletes `BUILTIN_MODULES`, and how the typecheck fix works

**`BUILTIN_MODULES` deletion:** npm `dependencies` never reach the derived manifest (§2). Once builtins move there (Fork 1), they no longer appear in `pluginDependencies`, so `imports_from_manifest` never encounters a builtin name, so the `is_builtin_module` skip has nothing to skip. The list — and its stale-copy hazard, mirrored in the registry branch's `registry/builtins.ts` — is deletable.

**The `typecheck.ts:76` fix (Part C's acceptance test):** today the filter must *guess* whether an `@s2script/*` name is a builtin (resolve) or an interface (stub), and it guesses by disk existence — which types a builtin *typo* as `any`. After consolidation, builtins are `s2script/*`, resolved by real path-mapping against the package's fixed subpath set, so:

- `s2script/frmae` (builtin typo) → path maps to a nonexistent subpath → **TS2307**. Fixed.
- `@community/mapchoser` (interface typo) → stub → `any`. **Not fixed — and correctly so:** an unfetched interface name is genuinely indistinguishable from a typo.

The honest filter keys on **shape**: `s2script/*` never stubs (resolve or error); only entries in `pluginDependencies`/`optionalPluginDependencies` that are not locally resolvable stub to `any`. The disk-existence check disappears entirely. **The claim is scoped precisely: consolidation fixes the reported builtin-typo class, not all typo classes.**

### 6.5 Migration touch-points (the checklist the plan expands)

- `core/src/v8host.rs:4065` — `s2require` gains `s2script/` stripping.
- `core/src/loader.rs:78,121,125` — `BUILTIN_MODULES` + its two call sites deleted (phase 3).
- `packages/cli/src/build.ts` — esbuild `external` accepts both prefixes (phase 1), then `s2script/*` (phase 3).
- `packages/cli/src/typecheck/typecheck.ts:76` — filter rewritten to the shape-based rule.
- `tsconfig.base.json:12` — `paths` twin updated for both prefixes, then narrowed.
- `games/cs2/js/*` — 9 `__s2require` literals (own PR, live gate).
- 77 consumer files — `@s2script/<builtin>` → `s2script/<builtin>` (batched).
- root `package.json` — renamed off `"s2script"`.
- `@s2script/cli` — deprecated/aliased; `@s2script/cs2` — gains `s2script` dep.

## 7. The stack map

Three independent Graphite stacks off `main`, each its own workflow-planned + workflow-implemented unit:

| Stack | PRs (rough) | Gate that proves it |
|---|---|---|
| **A** npm-name | 1 | `npx s2script build` resolves against the published bin |
| **B** review-debt | 4–6 (B2 isolated for its frozen-shape blast radius) | existing gate suite green per PR; targeted regressions on B1/B2 |
| **C** consolidation | 3 phases + N batch PRs | phase 3 grep = zero legacy builtin imports; live gate `pawn.origin != null`; the typo regression test |

Branch naming per CLAUDE.md: `packaging-name/…`, `packaging-debt/…`, `packaging-consolidation/…`. Run the gate suite **per PR**, not once at the top.

## 8. Testing

- **Typo regression (C's acceptance test):** a fixture importing `s2script/frmae` produces **TS2307**, not `any`; a companion asserts an interface typo (`@community/x`, unfetched) still stubs to `any` — proving the right class was fixed and we did not over-error. Add to `packages/cli/test/typecheck.test.mjs`.
- **Dual-prefix parity (phases 1–2):** both `@s2script/entity` and `s2script/entity` typecheck, esbuild-external, and resolve at runtime to `__s2pkg_entity`. Unit test on `s2require`'s two strip paths.
- **`BUILTIN_MODULES` deletion safety:** `check-plugins-typecheck.sh` green across every plugin post-migration; a test that a manifest cannot carry a phantom builtin ledger entry through the normal CLI path.
- **Silent-failure guard (C phase 2, cs2 literals):** the live Docker CS2 gate — load a plugin, assert `pawn.origin` resolves. The one PR that requires the live gate rather than CI alone.
- **Part A:** `npx s2script build` in a clean dir resolves the published bin.
- **Part B:** per-finding regressions (B1 trim, B2 distinct hashes) where meaningful; gate suite green otherwise.

## 9. Risks

| Risk | Mitigation |
|---|---|
| **The 9 `games/cs2` `__s2require` literals are compiler-invisible** — a missed rename degrades to `pawn.origin → null` silently. | Their own PR, dual-prefix so nothing breaks mid-flight, live-gate `pawn.origin != null` as the proof. |
| **A batch PR breaks atomicity** if the mechanism doesn't yet accept both prefixes. | Phase 1 lands dual-resolve first; no consumer PR merges before it. |
| **B2 corrects a just-frozen manifest shape** (distribution spec §9). | Isolate in its own PR, flag the frozen-shape change in the body, decide per-interface-hash vs single-contract-constraint at plan time. |
| **`s2script` name squatted before Part C lands.** | Part A claims it immediately with a real bin — decoupled from C's timeline. |
| **Consolidation lands after the registry.** | It must not (distribution spec §10). This spec sequences C before any registry launch. |
| **Version redundancy (`s2script` npm version vs `apiVersion`) confuses authors.** | Documented as two axes (types vs ABI), same as `typescript`; not collapsed, not hidden. |

## 10. Out of scope

- **Semver unification** (`interfaces.rs:50` major-only matching; `website/.../semver.ts` 0.x caret bug) — the distribution spec's hard follow-on, tracked there. Independent of this spec.
- **Consumer contract resolution (`s2script add`)** — distribution spec §10, plan 2. This spec does not build the interface-fetch path; it only ensures the namespace model leaves room for `.s2script/types/<name>/`.
- **The `/npm/*` facade + `.npmrc` writing** — parked in the distribution spec §10; not revisited here.
- **A flat root barrel** (`import { Chat } from "s2script"`) — explicitly rejected (§3). Subpaths only.
