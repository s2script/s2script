# examples/ cleanup — curated showcase + monorepo example — design spec

**Date:** 2026-07-22
**Branch:** `examples/cleanup` (off `main`)
**Status:** design approved, plan pending

## 1. Problem

`examples/` is 39 directories, 1,705 lines of example source, and **78 boilerplate files**
(`package.json` + `tsconfig.json`, two per dir). There is more scaffolding than there is code worth
reading. `README.md` references `examples/` **zero times**, so the maintenance cost buys a newcomer
nothing.

The directories are four different kinds of thing wearing the same `*-demo` costume:

| Kind | Members | What it actually is |
|---|---|---|
| Slice live-gate rigs | `liveness-gate`, `demo-plugin`, `entref-*` | one-shot proofs from a shipped slice; comments cite "Slice 5E.3", "pre-E1 SEGV" |
| Dev / treadmill tooling | `schema-dump`, `s2bench`, `crash-test` | not examples — `schema-dump` is *how gamedata is regenerated after a CS2 update* |
| One-module capability demos | ~30 | one dir per shipped module |
| Cross-plugin interface pair | `greeter-plugin` + `greeter-consumer` | the inter-plugin contract demo |

**The constraint that shapes everything:** `scripts/check-plugins-typecheck.sh` globs `examples/*/`,
making the demos the de-facto regression corpus for the shipped `.d.ts`. **Twelve modules are
covered only by `examples/`** — the shipped `plugins/` suite never imports them:

```
entity  http  ws  net  sound  trace  transmit  translations  usercmd  usermessages  cookies  zones
```

`entity` is the largest module in the SDK. A naive "keep five" cut silently guts the typecheck gate
for the entire entity + async-network surface. Any design here must preserve that coverage.

## 2. Goal

`examples/` becomes a **teaching showcase** — a small set of curated, README-linked examples a
newcomer reads to learn the API — **including a monorepo example** for authors who want to
modularise a large plugin across packages. Gate coverage is preserved, not sacrificed.

## 3. Findings (investigated, not assumed)

### 3.1 `s2s build` can bundle an npm-workspace tree today — if siblings use `exports`

Verified end-to-end against a real fixture through the actual `buildPlugin()`: typecheck → lint →
publish gate → esbuild bundle → valid `.s2sp`, with the sibling package's code **inlined into
`plugin.js`**.

The catch is `platform: "neutral"` in `packages/sdk/src/build.ts` (esbuild call, ~line 187). For
`platform: "neutral"` esbuild defaults `mainFields` to **empty**, so a sibling declaring `main`
fails to resolve:

```
✘ Could not resolve "@mono/greet"
  The "main" field here was ignored.
  Main fields must be configured explicitly when using the "neutral" platform.
```

Two independent fixes, **both verified working**:

| Fix | CLI change | Verdict |
|---|---|---|
| Sibling declares `exports: { ".": "./src/index.ts" }` | none | works today — the example adopts this |
| CLI sets `mainFields: ["module", "main"]` | one line | also works; makes `main` resolve too |

Note the typecheck stage **already** resolves workspace siblings — `moduleResolution: "bundler"`
handles it. Only the bundler stage was affected.

**Decision:** the monorepo example uses `exports` (correct modern field, zero CLI risk), **and** we
add `mainFields: ["module", "main"]` to the esbuild call. `main` is what authors naturally write,
and the default failure message is undiagnosable without knowing esbuild's platform semantics. The
fix is one line plus a regression test.

### 3.2 README does not link examples

`README.md` is 67 lines and mentions examples zero times. Curating a showcase nobody can find is
wasted work, so README/docs pointers land **in this slice**, not as a follow-up.

## 4. Design

### 4.1 Target layout

```
examples/
  hello-plugin/       first plugin — plugin(ctx), a command, an event, hot-reload state handoff
  entity-playground/  the centerpiece — create/spawn/EKV/teleport/entity I/O/outputs/listeners/beam
  monorepo-plugin/    npm workspaces → one .s2sp; siblings declare `exports`
  greeter-plugin/     cross-plugin interface: producer
  greeter-consumer/   cross-plugin interface: consumer (absorbs entref-*'s EntityRef-on-the-wire case)
  cookbook/           the long tail — one file per API, all under one plugin
tools/
  schema-dump/        regenerates games/cs2/gamedata/schema-catalog.json after a CS2 update
  s2bench/            op-timing benchmark
  crash-test/         deliberate-crash harness (dev_test-gated)
```

**39 dirs → 6 examples + 3 tools.** Per-plugin boilerplate drops from 78 files to 18 (nine
survivors × `package.json` + `tsconfig.json`), plus the monorepo example's two sibling manifests.

### 4.2 Why a cookbook rather than more merged scenarios

Merging every one-module demo into a handful of "realistic" scenario plugins reads well but hurts
the lookup case: someone asking *"how do I play a sound"* should not read a 200-line plugin. The
cookbook keeps each recipe small and greppable while collapsing 30 `package.json`/`tsconfig.json`
pairs into one. Flagships teach **structure**; the cookbook answers **"how do I do X"**.

`cookbook/` is one plugin whose `src/recipes/*.ts` each export a small registration function, all
wired from a single `src/plugin.ts`. One package, one typecheck target, one `.s2sp`.

### 4.3 Disposition of all 39 directories

| Directory | Destination |
|---|---|
| `demo-plugin` | → `hello-plugin` (rewritten; slice archaeology stripped) |
| `beam-demo`, `ekv-demo`, `entityio-demo`, `entity-listeners-demo`, `entity-name-demo` | → `entity-playground` |
| `greeter-plugin`, `greeter-consumer` | kept, consolidated |
| `entref-producer`, `entref-consumer` | → folded into the greeter pair (EntityRef on the wire), dirs deleted |
| `schema-dump`, `s2bench`, `crash-test` | → `tools/` |
| `liveness-gate` | **deleted** — E1 shipped; a finished slice's live-gate rig belongs in git history |
| `http-demo` | → `cookbook/recipes/http.ts` |
| `ws-demo` | → `cookbook/recipes/ws.ts` |
| `net-demo` | → `cookbook/recipes/net.ts` |
| `db-demo`, `db-remote-demo` | → `cookbook/recipes/db.ts` |
| `clientprefs-demo` | → `cookbook/recipes/cookies.ts` |
| `sound-demo` | → `cookbook/recipes/sound.ts` |
| `trace-demo` | → `cookbook/recipes/trace.ts` |
| `zones-consumer-demo` | → `cookbook/recipes/zones.ts` |
| `transmit-demo` | → `cookbook/recipes/transmit.ts` |
| `translations-demo` | → `cookbook/recipes/translations.ts` |
| `usercmd-demo` | → `cookbook/recipes/usercmd.ts` |
| `usermsg-demo` | → `cookbook/recipes/usermessages.ts` |
| `menu-demo` | → `cookbook/recipes/menu.ts` |
| `items-demo`, `weapon-demo` | → `cookbook/recipes/items.ts` |
| `respawn-demo` | → `cookbook/recipes/player-state.ts` |
| `changeteam-demo`, `switchteam-demo` | → `cookbook/recipes/team.ts` |
| `voice-demo`, `clients-demo` | → `cookbook/recipes/clients.ts` |
| `round-control-demo` | → `cookbook/recipes/events.ts` |
| `clientlist-convar-mapstart-demo` | → `cookbook/recipes/server.ts` |
| `gamerules-usermsg-demo` | → `cookbook/recipes/gamerules.ts` |
| `admin-groups-demo` | **deleted** — `admin` is exercised by the shipped `plugins/` suite |

### 4.4 Preserving the typecheck gate

Two changes to `scripts/check-plugins-typecheck.sh`:

1. Add `tools/*/` to the glob, so relocating the three tools loses no coverage.
2. Nothing else — `examples/*/` still matches the six survivors.

All twelve example-only modules keep coverage by construction: `entity` via `entity-playground`;
`http`/`ws`/`net`/`cookies` and `sound`/`trace`/`zones`/`transmit`/`translations`/`usercmd`/
`usermessages` via cookbook recipes.

### 4.5 New gate: module → recipe coverage

A cleanup that restores coverage once, then rots as new modules land, has only deferred the
problem. Add `scripts/check-examples-coverage.sh`: enumerate the shipped SDK capability `.d.ts`
subpaths, enumerate the modules imported across `examples/` + `plugins/` + `tools/`, and fail with
a named list if any shipped module has no consumer anywhere. Joins the gate suite in CLAUDE.md.

This is the piece that makes the cleanup durable rather than a one-time tidy.

### 4.6 The monorepo example

`examples/monorepo-plugin/` demonstrates what nothing in the repo shows today — structuring a large
plugin across packages:

```
monorepo-plugin/
  package.json          workspaces: ["packages/*"]; main: src/plugin.ts
  src/plugin.ts         composes the feature packages
  packages/core/        shared types + helpers        (exports: ./src/index.ts)
  packages/commands/    a feature slice importing @mono/core
```

It documents, in comments and in the README section, the two facts an author needs: siblings declare
**`exports`**, and the whole tree bundles into **one** `.s2sp` (the sibling is inlined — it is not a
runtime dependency). It also states the boundary against cross-plugin interfaces: workspace packages
are a *build-time* factoring of one plugin; `greeter-*` is the *runtime* contract between two
separately-loaded plugins. Conflating those is the mistake this example exists to prevent.

### 4.7 Documentation

- `README.md` — a short "Examples" section linking the six, one line each on what it teaches.
- `docs/BUILDING.md` — update the layout block (line ~25) and the build-scripts note (line ~149).
- `CLAUDE.md` — repository-layout block: `examples/` description, new `tools/` entry, and the new
  gate in the gate-suite list.
- `docs/INSTALL.md:57` — the "demos live under `examples/`" aside still holds; verify wording.

Historical plans and specs under `docs/superpowers/` that reference deleted dirs are **left
untouched** — they are a record of what was true then, not live documentation.

## 5. Delivery

**One branch, one PR** — per `2026-07-22-ci-consolidation-design.md` §3, which retires Graphite
stacking in favour of one PR per slice.

Two notes on that, flagged rather than silently resolved:

- `CLAUDE.md` still carries the "Ship work as a stack, not a branch (Graphite)" section; its
  deletion is part of the in-flight `ci/consolidation` work. This slice follows the **new** decided
  doctrine. If `ci/consolidation` has not merged when this lands, the contradiction is cosmetic —
  the decision is recorded and dated.
- This slice is unusually broad for one PR (deletes ~30 dirs, adds three new example plugins). The
  diff is large but overwhelmingly **deletions and moves**; the genuinely new code is
  `entity-playground`, `monorepo-plugin`, the cookbook wiring, and the coverage gate. If review
  friction proves the point, the natural cut line is *(a)* tools relocation + deletions, then
  *(b)* new examples + gate + docs.

Work happens in a dedicated worktree off `main`, not on `ci/consolidation`.

## 6. Success criteria

1. `examples/` contains exactly six directories; `tools/` contains three.
2. `./scripts/check-plugins-typecheck.sh` passes and covers `examples/*/`, `plugins/*/`,
   `plugins/disabled/*/`, `tools/*/`.
3. `./scripts/check-examples-coverage.sh` passes; deleting a recipe makes it fail with a named module.
4. `s2s build examples/monorepo-plugin` produces a `.s2sp` with the sibling package inlined.
5. Every surviving example and tool builds to a `.s2sp`.
6. `README.md` links all six examples.
7. No `core/` → `games/*` boundary change; `make check-boundary` unaffected.

## 7. Risks / open questions

- **Cookbook plugin size at runtime.** Every recipe registers in one plugin, so loading the cookbook
  registers many commands at once. Recipes must be side-effect-light and clearly namespaced
  (`cb_<topic>`), and the cookbook is a demo — never shipped in the release zip.
- **`entity-playground` scope.** May want splitting into two examples if it exceeds ~150 lines;
  settled while writing it, based on how it reads. Not worth pre-deciding.
- **Recipes needing a live server.** Some demos (`transmit`, `usercmd`, `voice`) only do anything
  meaningful with players connected. Recipes stay compile-correct and self-describing; this slice
  does **not** commit to a live gate for every recipe — the typecheck gate is the contract.
- **`mainFields` change is a build-behaviour change.** It widens resolution; it cannot break an
  existing plugin that resolves today. Covered by a regression test.
