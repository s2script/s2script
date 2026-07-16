# Packaging consolidation, review debt, and the npm name — design spec

**Date:** 2026-07-16
**Status:** design (pending approval) → implementation plan next
**Scope:** finish the packaging story the contract-grammar stack (PRs #36–#41) left open. Three independent parts: **(A)** claim the `s2script` npm name, **(B)** close the six review-debt findings from `/code-review high` on the contract-grammar stack, **(C)** consolidate the 29 `@s2script/*` types-only stubs into a single `s2script` package with subpaths. Amends the distribution spec at `docs/superpowers/specs/2026-07-15-plugin-contract-distribution-design.md` — specifically retires its §10 "stub consolidation" and "risks" debt.

**Depends on:** the contract-grammar stack (#36–#41) being the baseline. This spec assumes those defects (see that stack's plan "Amendments") are already fixed.

---

## ⚠ Naming pivot (amended 2026-07-16) — READ FIRST, supersedes the unscoped-name decisions below

The unscoped **`s2script` npm name is permanently unobtainable**: npm's new-package name-similarity filter hard-blocks it (`403 Forbidden — Package name too similar to existing package rescript`), for everyone, forever. The design's *mechanics* are unchanged — only the name moves. Everything below that assumes the unscoped name is superseded by these authoritative decisions:

- **The consolidated package is `@s2script/sdk`** (scoped — bypasses the filter), a **types + CLI** package ("SDK" = the kit: types *and* tooling). Subpath imports are **`@s2script/sdk/<cap>`** — e.g. `import { Entity } from "@s2script/sdk/entity"`. Wherever the text below says the unscoped `s2script` package or an `s2script/<cap>` subpath, read `@s2script/sdk` / `@s2script/sdk/<cap>`. The package dir is `packages/sdk/`.
- **The CLI bin is `s2s`** (not `s2script`). Bin names are exempt from npm's package-name filter. Installed usage: `s2s build`. Cold-start: `npx @s2script/sdk build` — **not** `npx s2s`, which resolves the unrelated existing `s2s@0.20.1` "Source To Source" package. Wherever the text says the bin `s2script`, read `s2s`.
- **Part A is CLOSED (void).** Its entire premise was claiming the unscoped name; PR #50 is closed. There is no squat risk (npm blocks the name for everyone) and no "honest 404 → could-not-determine-executable" footgun (the name is unpublishable). An npm-support appeal for the unscoped name is a free lottery ticket — do not plan around it.
- **The root-`package.json` rename is DROPPED** (former Part C PR C4). It existed only to free the unscoped `s2script` name; the root stays `name: "s2script"`, private.
- **Fork 2 stands** (types + CLI in one package), now realized as `@s2script/sdk` binning `s2s`. **Fork 3 is void** (nothing to claim now). **Fork 1 stands** unchanged.
- **`s2require` strip order (correctness):** strip `@s2script/sdk/` **before** `@s2script/` — the shorter prefix also matches `@s2script/sdk/entity` and would yield `__s2pkg_sdk/entity` garbage. Both map to `__s2pkg_<cap>`; `@s2script/cs2` keeps riding the plain `@s2script/` strip.
- **esbuild `external` needs no new pattern** — the existing `@s2script/*` wildcard already covers `@s2script/sdk/entity` (esbuild wildcards cross `/`). The earlier "add `s2script/*` to external" step is dropped.
- **After migration:** `npm deprecate` the 29 `@s2script/*` capability stubs → `@s2script/sdk`, and `@s2script/cli` → `@s2script/sdk`. Keep the `@s2script` scope owned as brand protection.

The authoritative, concrete implementation is `docs/superpowers/plans/2026-07-16-packaging-consolidation.md` (revised to `@s2script/sdk` + bin `s2s`). Parts A and B below keep their original text for history; **Part B (#44–#49) is unaffected**.

---

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
- **Blast radius (re-counted, larger than the earlier ~188/50 estimate):** **230 import sites across 77 files**; **10 `__s2require` string literals** in `games/cs2` (`pawn.js` ×5 — including one embedded in the `__s2pkg_cs2 =` assignment at `pawn.js:830` — `weapon.js` ×1, `nav.generated.js` ×2, `schema.generated.js` ×2) that are compiler-invisible — a miss degrades to `pawn.origin → null` silently at runtime. **Every count in this spec is illustrative; the plan must grep, never hardcode a count** — the off-by-one here (an earlier pass said 9) is exactly the silent-failure class this migration worries about. **11 cross-package `.d.ts` imports** in `packages/*/index.d.ts`; the `tsconfig.base.json:12` `paths` twin (`"@s2script/*": ["*/index.d.ts"]`); the root `package.json` is **already named `"s2script"` (private)** and must be renamed to free the name. `check-core-boundary.sh` and `check-plugins-typecheck.sh` are crate/path-based and need no change.
- **The typecheck gate resolves builtin types at three coupled sites**, all keyed to the current `packages/<name>/index.d.ts` layout: `typecheck.ts:87` (`paths: { "@s2script/*": ["*/index.d.ts"] }`), `typecheck.ts:60` (`isBuiltinOnDisk` = `existsSync`), `typecheck.ts:91` (the hardcoded `globals/globals.d.ts` rootName). A fourth, `packages/cli/src/packages-resolve.ts`, resolves `@s2script/*` types for out-of-monorepo builds via `node_modules/@s2script/` and names `@s2script/globals` in its error text. Any layout move must update all four in lockstep (§6.3).
- **`@s2script/cs2` is declared in `pluginDependencies` by real consumers** (`plugins/zones`, `examples/{demo-plugin,entref-producer,zones-consumer-demo}`) and is itself in `BUILTIN_MODULES` (so skipped from the ledger today). Its own `package.json` pins **exact** versions of soon-deleted stubs (`@s2script/entity: 0.3.0`, `math`, `trace`, `events`) and its `index.d.ts` imports them.
- **CLAUDE.md's "Never overload npm's `exports`"** governs *plugin manifests*, not a published types package — confirmed. Adding a subpath `exports` map to `s2script` is out of its scope.

## 3. Decisions (forks resolved)

| Fork | Decision | Rationale |
|---|---|---|
| **1 — builtins → npm `dependencies`?** | **Yes.** Move builtins from `s2script.pluginDependencies` to npm `dependencies` — **and the game package `@s2script/cs2` with them.** | A consolidated `s2script` *is* an npm build-dep, so CLAUDE.md's "`dependencies` = npm build-deps only" puts it there. Because the derived manifest never carries npm `dependencies` (§2), builtins **vanish from the manifest**, `imports_from_manifest` never sees them, and `BUILTIN_MODULES` becomes genuinely unemployed (deletable). **Game packages must move too:** `@s2script/cs2` is always-present-per-game (like a builtin, not a presence-conditional interface), it lives in `pluginDependencies` today, and it is in `BUILTIN_MODULES`. If its consumer declarations stayed in `pluginDependencies`, the typecheck filter would still need a disk/resolvability check to tell it from an interface — the exact check §6.4 claims disappears — and post-`BUILTIN_MODULES`-deletion it would become a phantom Hard ledger dep. Moving it to npm `dependencies` (it resolves via `node_modules/@s2script/cs2`, still scoped) is what lets the filter be purely shape-based. |
| **2 — does `s2script` ship the CLI bin?** | **Yes — types + CLI** (the `typescript`/`tsc` model). | `npm i -D s2script` gives the subpath types *and* `npx s2script build`. Every plugin author needs the CLI to build anyway; one dep, one version. Kills the npx footgun definitively. `@s2script/cli` is deprecated/aliased. |
| **3 — claim the name now or at consolidation?** | **Now, with a real forwarding bin** (Part A). | A defensive claim prevents squatting; the forwarding bin makes `npx s2script build` work today. A types-only placeholder is the one thing to avoid — it breaks `npx`. Part C later replaces the package contents. |
| Decomposition | **One spec, three independently-stackable parts.** | A/B/C have no build-order dependency on each other except "A frees the name C fills." Each is its own Graphite stack, planned + implemented via its own workflow. |

**Settled going in (not re-litigated):** `@s2script/cs2` stays a **separate scoped package** (game → core, never core → game); **no flat root barrel** (`import { Chat } from "s2script"`) — subpaths only; **no changesets `fixed` lockstep**.

---

## Part A — claim the `s2script` npm name

**One PR. Independent of B and C. Do first.**

Publish `s2script@0.0.x` with a **real forwarding bin** — a tiny `bin: { s2script }` shim that runs the installed `@s2script/cli` so `npx s2script build` resolves and runs today. The package is otherwise minimal.

- **Forward by module path, never by bin name (avoids infinite recursion).** Both `s2script` and `@s2script/cli` declare a bin named `s2script`; a shim that forwards by *spawning the `s2script` bin* (PATH / `.bin`) can resolve to itself and loop. The shim must `require.resolve("@s2script/cli/dist/cli.js")` and execute that file directly. `@s2script/cli` is a real `dependency` of `s2script`, so it is always installed. Users who install both packages will see a transient npm `.bin` collision warning; it disappears in Part C when the CLI is absorbed. (Vendoring the CLI entry is the worse fork — it drags `esbuild`/`adm-zip`/`typescript` in as deps regardless.)
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

| Import shape | Declared in | Meaning | Resolves how | Typo behavior |
|---|---|---|---|---|
| `s2script/<cap>` (unscoped subpath) | npm `dependencies` | a builtin | path-mapping / `exports` → the one `s2script` package's subpath `.d.ts` | **TS2307** (miss = real error) |
| `@s2script/cs2`, `@s2script/<game>` (scoped) | npm `dependencies` | a game package | real installed package `.d.ts` (`node_modules/@s2script/cs2`) | **TS2307** |
| `@scope/name` | `pluginDependencies` | an inter-plugin interface (incl. first-party `@s2script/zones`) | `.s2script/types/<name>/` if fetched, else ambient stub | `any` (unknowable until fetched) |

The discriminator is now **which map declares it**, which the shape mirrors: npm `dependencies` = always-present (builtins + game packages) = resolve-or-error, never stub; `pluginDependencies` = presence-conditional interfaces = stub-until-fetched. That is what makes the typecheck filter honest (§6.4) with no disk check.

**This table is a compile-time contract, not a runtime guarantee.** At runtime `s2require` resolves `s2script/<cap> → __s2pkg_<cap>` — one additive strip-path alongside the existing `@s2script/` one — and is deliberately permissive: `@s2script/cs2` keeps its current path untouched, and dual-stripping means `s2script/cs2` or any `__s2pkg_*` global would also resolve at runtime even though the gate rejects it. The gate, not `s2require`, enforces the namespace split.

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
- **Versioning — the one redundancy, named and kept:** a plugin declares `dependencies: { "s2script": "^0.1.0" }` (the `.d.ts` contract it compiled against — `s2script` starts pre-1.0, not API-frozen) **and** keeps `s2script.apiVersion` (the host ABI it loads against). Two real axes — types vs runtime — that move together in practice, exactly as `typescript@5.4` is both the compiler and `lib.d.ts`. **Not collapsed.** The per-capability version pins builtins carry today (`@s2script/entity: ^0.2.0`) are lost, but those are fictional — all builtins ship in one runtime zip, versioned by the host — so one `s2script` version is *more* honest.
- **Neither axis cross-validates the other, and that hole is pre-existing, not new.** `apiVersion` is major-only at load (`loader.rs:68`); the `s2script` npm version never reaches the manifest at all. Concrete failure the design does **not** fully close: a plugin compiled against `s2script@0.4` types that include a capability added in host runtime 0.4 (say a new `s2script/<cap>`), declaring `apiVersion: "1.x"`, deployed to a host at 0.2 → the apiVersion gate passes (major matches), `__s2pkg_<cap>` is undefined, `s2require` returns null → a runtime `TypeError` on a plugin that typechecked green. This hole exists today (the per-capability pins were equally unenforced), so consolidation does not worsen it. **Cheap mitigation in scope:** the CLI stamps the resolved `s2script` types version into the manifest as an **informational** field, so this exact failure is diagnosable at load. **Full minor-level gating is out of scope** (semver spec, §10).
- **`@s2script/cs2`** stays a separate scoped package and gains `dependencies: { "s2script": "..." }`, since its `.d.ts` imports `s2script/entity`, `s2script/math`, `s2script/trace`, `s2script/events`.

### 6.3 The dual-prefix transition — how a 230-site rename becomes a stack

A hard-cut rename of 77 files cannot be both atomic and small — after the resolution mechanism flips, every un-migrated import breaks. The enabling trick is a **dual-prefix transition**: teach the mechanism both spellings, migrate consumers in batches, then remove the old spelling once nothing uses it. The runtime cooperates for free — `s2require` resolving both `@s2script/entity` and `s2script/entity` to `__s2pkg_entity` is purely additive.

**Phase 1 — publish `s2script` + dual-resolve (one PR).** The `.d.ts` files physically move in this PR, so the gate's resolution sites **must move with them, in the same commit** — otherwise a plugin that declares builtins falls through `isBuiltinOnDisk` into the ambient stub and types as `any`, which typechecks **green**: a silent hollowing of the 5E.1 gate that CI cannot catch. Concretely:
- Fill `packages/s2script/` with the moved `.d.ts` + `exports` map, keeping Part A's forwarding bin (the CLI is physically absorbed later, in a dedicated final phase-3 PR, so the trap PR stays small).
- Add `s2script/` stripping to `s2require` (`v8host.rs:4065`), alongside the existing `@s2script/`.
- Update **all four** type-resolution sites (§2) to find builtins at the new location while still resolving `@s2script/cs2` at the old one: `typecheck.ts:87` paths become an ordered fallback (`"@s2script/*": ["s2script/*.d.ts", "*/index.d.ts"]`) plus a new `"s2script/*"` entry; `isBuiltinOnDisk` checks both locations; the `globals` rootName checks both; `packages-resolve.ts` learns `node_modules/s2script`.
- Add `s2script/*` to the esbuild `external` list (the literal `@s2script/*` wildcard already externalizes the scoped forms; `s2script/*` does not match a bare `s2script` import, which is fine because the flat barrel is rejected, §3).
- No consumer changes yet — fully backward-compatible.
- **Green CI does not prove this PR correct** (green is exactly the silent-failure signature). It must ship a **canary test** (§8): a fixture with a deliberate type error against a builtin still *fails* the gate, proving resolution did not degrade to `any`.

**Phase 2 — migrate consumers in batches (N small PRs).**
- One PR per plugin (or a few), rewriting `@s2script/<builtin>` → `s2script/<builtin>` and moving builtins **and `@s2script/cs2`** from `s2script.pluginDependencies` to npm `dependencies` (Fork 1). The `@s2script/cs2` *import specifier* is unchanged (it stays scoped); only its declaration map moves.
- Each PR atomic because both prefixes still resolve.
- **`games/cs2`'s `__s2require` literals get their own PR** (grep for the exact set — ~10, including the one embedded in the `__s2pkg_cs2 =` assignment), gated by the live Docker CS2 gate (`pawn.origin != null`) — a missed literal fails silently, so CI alone can't prove it.

**Phase 3 — remove the legacy builtin prefix (one PR).**
- Delete the 29 stub packages and `packages/globals`.
- Delete `BUILTIN_MODULES` from `loader.rs` (now unemployed — see §6.4).
- Narrow the typecheck filter to the honest shape-based rule (§6.4); collapse the phase-1 fallback paths to the single `s2script/*` + `@s2script/*`-for-games form.
- Rename the private root `package.json` off `"s2script"`.
- **Republish `@s2script/cs2`** with its npm `dependencies` re-pointed at `s2script` and its `.d.ts` imports rewritten — otherwise the published game package dangles on deprecated stubs (`@s2script/entity@0.3.0` etc.) that no longer exist.
- **Gate:** a grep proves zero `@s2script/<builtin>` imports survive. `@s2script/cs2` and `@s2script/zones` keep the scope — cs2 as an installed package, zones as an interface.

### 6.4 Why Fork 1 deletes `BUILTIN_MODULES`, and how the typecheck fix works

**`BUILTIN_MODULES` deletion:** npm `dependencies` never reach the derived manifest (§2). Once builtins (and `@s2script/cs2`) move there (Fork 1), they no longer appear in `pluginDependencies`, so `imports_from_manifest` never encounters a builtin name, so the `is_builtin_module` skip has nothing to skip. The list — and its stale-copy hazard, mirrored in the registry branch's `registry/builtins.ts` — is deletable.

**Legacy `.s2sp` posture (stated, not hand-waved):** a pre-migration artifact (today's zones `.s2sp` declares 11 builtins in `pluginDependencies`) loaded by a post-deletion core gets those pushed as Hard interface deps with no producer. This is **behaviorally benign** — `call_target_inner` is lazy (`Unavailable` at *call* time, never at load) and `__s2_require` is prelude-first, so the phantom entry is never called — but the imports ledger carries phantoms. Acceptable because this is all pre-registry and the runtime resolves correctly. A **load-side test** pins it: a legacy-shaped manifest (builtins in `pluginDependencies`) still loads and runs post-deletion.

**The `typecheck.ts:76` fix (Part C's acceptance test):** today the filter must *guess* whether an `@s2script/*` name is a builtin (resolve) or an interface (stub), and it guesses by disk existence — which types a builtin *typo* as `any`. After consolidation, builtins are `s2script/*`, resolved by real path-mapping against the package's fixed subpath set, so:

- `s2script/frmae` (builtin typo) → path maps to a nonexistent subpath → **TS2307**. Fixed.
- `@community/mapchoser` (interface typo) → stub → `any`. **Not fixed — and correctly so:** an unfetched interface name is genuinely indistinguishable from a typo.

The honest filter keys on **shape**: `s2script/*` never stubs (resolve or error); only entries in `pluginDependencies`/`optionalPluginDependencies` that are not locally resolvable stub to `any`. The disk-existence check disappears entirely. **The claim is scoped precisely: consolidation fixes the reported builtin-typo class, not all typo classes.**

### 6.5 Migration touch-points (the checklist the plan expands)

- `core/src/v8host.rs:4065` — `s2require` gains `s2script/` stripping.
- `core/src/loader.rs:78,121,125` — `BUILTIN_MODULES` + its two call sites deleted (phase 3).
- `packages/cli/src/build.ts` — esbuild `external` accepts both prefixes (phase 1), then `s2script/*` (phase 3); CLI stamps the informational `s2script` types version into the manifest (§6.2).
- `packages/cli/src/typecheck/typecheck.ts:60,76,87,91` — the four in-file resolution sites (paths, `isBuiltinOnDisk`, globals rootName) moved in phase 1, filter narrowed to the shape-based rule in phase 3.
- `packages/cli/src/packages-resolve.ts` — learns `node_modules/s2script`; error text updated off `@s2script/globals`.
- `tsconfig.base.json:12` — `paths` twin updated for both prefixes, then narrowed.
- `games/cs2/js/*` — ~10 `__s2require` literals, **grep-derived not hardcoded** (own PR, live gate).
- 77 consumer files — `@s2script/<builtin>` → `s2script/<builtin>` (batched); builtins + `@s2script/cs2` move `pluginDependencies` → npm `dependencies`.
- root `package.json` — renamed off `"s2script"`.
- `@s2script/cli` — physically absorbed into `packages/s2script/` in the final phase-3 PR (`src`, `test`, `build.mjs`, `tsconfig.json` git-moved; the ~8 `scripts/*` + `tsconfig.base.json` refs rewritten; `s2script` bin points at its own `dist/cli.js`; the `@s2script/cli` dependency + Part-A forwarding shim dropped; `@s2script/cli` deprecated). `@s2script/cs2` — gains `s2script` dep, `.d.ts` imports rewritten, republished (phase 3).

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
- **Phase-1 no-degrade canary (blocks the silent-hollowing failure):** a fixture with a deliberate type error against a builtin (both `@s2script/entity` and `s2script/entity`) must **fail** the gate after the `.d.ts` move. Green CI is the silent-failure signature, so a passing canary — not overall green — is what proves phase 1 preserved real resolution.
- **Dual-prefix parity (phases 1–2):** both `@s2script/entity` and `s2script/entity` typecheck, esbuild-external, and resolve at runtime to `__s2pkg_entity`. Unit test on `s2require`'s two strip paths.
- **`BUILTIN_MODULES` deletion safety:** `check-plugins-typecheck.sh` green across every plugin post-migration; a **load-side** test that a legacy-shaped manifest (builtins in `pluginDependencies`) still loads and runs post-deletion (phantom-lazy-hard-dep posture, §6.4); and a build-side test that the normal CLI path cannot emit a builtin into the manifest.
- **Silent-failure guard (C phase 2, cs2 literals):** the live Docker CS2 gate — load a plugin, assert `pawn.origin` resolves. The one PR that requires the live gate rather than CI alone.
- **Part A:** `npx s2script build` in a clean dir resolves the published bin.
- **Part B:** per-finding regressions (B1 trim, B2 distinct hashes) where meaningful; gate suite green otherwise.

## 9. Risks

| Risk | Mitigation |
|---|---|
| **The ~10 `games/cs2` `__s2require` literals are compiler-invisible** — a missed rename degrades to `pawn.origin → null` silently. | Their own PR, dual-prefix so nothing breaks mid-flight, grep-derived set (not a hardcoded count — an earlier pass miscounted 9 vs 10), live-gate `pawn.origin != null` as the proof. |
| **Phase 1 hollows the typecheck gate silently** if the `.d.ts` move and the four resolution sites don't land together — builtins fall through to the `any` stub, CI stays green. | Move all four sites in the phase-1 commit; ship the no-degrade canary (§8); treat green CI as insufficient proof for that PR. |
| **Neither version axis cross-validates the other** — types newer than the host minor pass the major-only apiVersion gate, then `TypeError` at runtime. | Pre-existing, not worsened; CLI stamps an informational types version for diagnosis; full minor gating is the semver spec (§10). |
| **Part A's forwarding bin recurses** if it forwards by bin name. | Forward via `require.resolve("@s2script/cli/dist/cli.js")`, never PATH/`.bin`. |
| **Published `@s2script/cs2` dangles** on deleted stub versions after phase 3. | Republish it with `s2script`-pointed deps in/immediately after phase 3. |
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
