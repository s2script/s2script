# CI · Config · Disabled-plugins cleanup — design spec

**Date:** 2026-07-19
**Status:** approved (brainstorming) → ready for planning
**Scope of THIS spec:** three independent workstreams that share the theme "release / packaging /
CI hygiene," designed together so they parallelize cleanly onto separate Graphite sub-stacks and
separate Fable-orchestrated implementation workflows:

- **A — CI restructure.** Stop the per-push storm, cache the builds, and actually gate the checks
  that today only run locally.
- **B — `disabled/` relocation + plugin feature work.** Move the opt-in plugins under `plugins/`,
  ship them (loader-skipped) in the release, and bring the four of them to functional SourceMod
  parity.
- **C — Config system (SourceMod-parity, file-backed).** Enrich the declaration format, model the
  framework configs (admin + db) the same way, and generate every default config at package time so
  it ships in the zip instead of being lazily written on first boot.

Each workstream is atomic-per-PR and gate-passing on its own. A is fully independent; B and C both
touch `scripts/package-release.sh` but in different sections (B adds a disabled-copy loop, C adds a
config-gen hook), so they merge without conflict.

---

## 1. Goal

Three concrete outcomes:

1. **PRs stop wasting runner minutes.** One CI run per push (not two), superseded on restack, built
   from a warm cache, and gating the full documented gate suite — not just `deps`.
2. **The release is complete and honest.** The opt-in plugins ship (disabled, per the CLAUDE.md
   contract that is currently only aspirational), and every plugin's + framework's default config
   ships as a commented file, SourceMod-style, rather than being conjured on first server boot.
3. **Config authoring is a first-class DX with a registry-ready contract.** A plugin author declares
   config values (types, defaults, descriptions, ranges, enums, groups, sections); s2script generates
   the operator-editable defaults file; and the same declaration is the schema the future registry
   parses to display "potential config values."

## 2. Background / the gaps today

**CI.** `.github/workflows/ci.yml` uses `on: [push, pull_request]`, so every PR-branch push fires
*both* events and the whole workflow runs twice (observed on PR #94: two `build` jobs at ~4m54s, two
`deps` jobs). There is no `concurrency` group, so a Graphite restack stacks concurrent runs instead
of superseding the in-flight one. There is no Rust or cmake caching, so each `cargo build --release`
recompiles every crate from scratch. The documented gate suite (the 5E.1 typecheck gate, the four
`check-*-generated.sh` freshness gates, `check-core-boundary.sh`, `test-boundary-nameleak.sh`, and
`cargo test -p s2script-core`) is **not in CI at all** — it runs only locally. Branch protection
requires only the `deps` check.

**Disabled plugins.** `nominations`, `rockthevote`, `funvotes`, `nextmap` live in repo-root
`disabled/`. They are **not shipped**: `package-release.sh` copies only top-level
`plugins/*/dist/*.s2sp`. Build scripts reference the root path (`build-base-plugins.sh` walks
`plugins/*/ disabled/*/`; `check-plugins-typecheck.sh` walks `for base in examples plugins disabled`).
The CLAUDE.md layout note ("the loader skips top-level `disabled/`; operators move a `.s2sp` up one
level to enable") describes an end state that is not actually wired into packaging.

**Config.** The infrastructure exists and is good, but incomplete on two axes:

- *Not shipped.* Default config files are written **lazily on first server boot**
  (`materialize_for_load` in `core/src/v8host.rs` calls `config_write` only when the override file is
  absent). The release zip ships an empty `configs/` dir. The framework configs
  (`admins.json`, `admin_groups.json`, `admin_overrides.json`, `databases.json`) are the same — their
  defaults are hardcoded **templates embedded in core JS** (`v8host.rs`) and generated on first boot.
- *Format is thin.* `s2script.config` is a flat map of `{ type: "string"|"int"|"float"|"bool",
  default, description? }` (`packages/sdk/src/config-validate.ts`, `core/src/config.rs`). No ranges,
  enums, grouping, or sections — so a registry can't render anything richer than a flat list, and
  nested config isn't expressible.

---

## Workstream A — CI restructure

### A.1 Trigger dedup + concurrency

- PR-gating workflow triggers on `pull_request` only; a `push` trigger runs only on `main` (post-merge
  smoke). This removes the double-run.
- Add to the PR workflow:
  ```yaml
  concurrency:
    group: ${{ github.workflow }}-${{ github.ref }}
    cancel-in-progress: true
  ```
  Never add `cancel-in-progress` to the `main` push or the tag release workflow — those must run to
  completion.

### A.2 Caching + cheaper PR build

- `Swatinem/rust-cache@v2` (caches `~/.cargo/registry`, `~/.cargo/git`, and `target/`). Cache
  `build/shim` and enable ccache for the C++ shim.
- PR CI builds **debug**, not `--release` (release is only needed for the tagged sniper build), and
  adds `cargo test -p s2script-core` (the in-isolate suite, currently run nowhere in CI). Debug shim
  build.

### A.3 Gate suite in CI

Lift the local-only gates into fast, cached jobs:

- Node gates: `check-plugins-typecheck.sh` (5E.1), `check-schema-generated.sh`,
  `check-nav-generated.sh`, `check-events-generated.sh`, `check-csitem-generated.sh`,
  `test-boundary-nameleak.sh`.
- Rust/native gates: `check-core-boundary.sh`, `cargo test -p s2script-core`.

### A.4 Path filtering + single required check

- A `changes` job (`dorny/paths-filter`) classifies the diff: Rust/C++ paths → build + test +
  boundary; `plugins/**` + `packages/**` → JS gates; docs-only → skip build.
- A terminal `ci-ok` job `needs:` every conditional job and succeeds iff each dependency
  passed **or was skipped**. `ci-ok` becomes the **one required status check** in branch protection
  (replacing the current `deps`-only requirement). This is the standard pattern that lets path
  filtering coexist with required checks — a skipped job never blocks merge.

### A.5 Reusable build workflow

Extract the core+shim build into `.github/workflows/_build.yml` (`workflow_call`), consumed by both
`ci.yml` (debug, cached) and `release.yml` (release, sniper). The build recipe lives once.

### A.6 Explicitly out of scope: GitHub native merge queue

"Full restructure" implies a merge queue, but merges happen through **Graphite** (`gt submit`), whose
stack-merge conflicts with GitHub's native merge queue (`merge_group`). Graphite already restacks
against trunk before merge, giving the tested-against-trunk property a queue would provide.
**Decision: no native merge queue.** All other A.1–A.5 items stand.

### A.7 Matrix

Minimal — one `ubuntu-latest` runner. No debug/release matrix on PRs (release is tag-only). Noted
here only to record that we deliberately did not add matrix cost.

---

## Workstream B — `disabled/` relocation + plugin feature work

### B.1 Source move

`disabled/<name>` → `plugins/disabled/<name>`. Source now mirrors the release layout. Root `disabled/`
is removed.

### B.2 Build-script fixes

- `scripts/build-base-plugins.sh`: `plugins/*/ disabled/*/` → walk `plugins/*/` **and**
  `plugins/disabled/*/`, and skip the bare `plugins/disabled/` directory (no `package.json`).
- `scripts/check-plugins-typecheck.sh`: replace `for base in examples plugins disabled` with a walk
  that includes `plugins/disabled/*` (the `[ -f package.json ]` guard already skips the container
  dir).
- Any other `disabled/`-path references (grep-swept as part of the task) updated to `plugins/disabled/`.

### B.3 Ship them, loader-skipped

`scripts/package-release.sh` gains a loop copying `plugins/disabled/*/dist/*.s2sp` →
`$STAGE/addons/s2script/plugins/disabled/`. The runtime watcher is top-level-only, so a `.s2sp` in
that subdirectory does not load until an operator moves it up to `plugins/`. This finally wires the
CLAUDE.md contract. A short `plugins/disabled/README.txt` explains the move-up-to-enable step.

### B.4 Feature work → functional SM parity

Bring each of `nominations`, `rockthevote`, `funvotes`, `nextmap` to functional SourceMod parity,
each **live-gated** on the Docker CS2 server. This is the largest-variance part of the slice: the
**per-plugin parity checklist is pinned in the implementation plan**, derived from the SM default
plugin behavior (the standing parity yardstick). Scope guard: parity behavior + live-gate, not
speculative new features beyond SM.

---

## Workstream C — Config system (SourceMod-parity, file-backed)

### C.1 Value backing — decided

Values stay **file-backed JSONC** read via `@s2script/config` with `onChange` live-reload. **Not**
engine ConVars. This matches the current architecture, keeps live-reload, and avoids a core read-path
rewrite. (Considered and rejected for this slice: a ConVar bridge / per-key cvar mirror.)

### C.2 Enriched declaration format

`package.json` `s2script.config` remains the authoring surface (per the "package.json is the
authoring format" guardrail). Backward-compatible additions:

- Optional per-decl fields: `min`, `max` (numeric bounds), `enum` (allowed values), `label`
  (registry display name), `group` (registry grouping), `sensitive` (registry masks it; e.g. a token).
- **Optional nested sections.** Disambiguation rule: an entry object that has a `type` field is a
  *value declaration*; an entry object without a `type` field is a *section* and is recursed into.
  This is backward-compatible (existing flat blocks have `type` on every entry).

Example:
```jsonc
"config": {
  "flood_time": { "type": "float", "default": 0.75, "min": 0, "description": "..." },
  "voting": {                                   // section (no "type")
    "vote_duration": { "type": "int", "default": 20, "min": 1, "max": 120, "group": "voting" },
    "mode": { "type": "string", "default": "map", "enum": ["map", "kick", "custom"] }
  }
}
```

On disk, sections nest in the generated JSONC. Access is dotted: `config.getInt("voting.vote_duration")`.

### C.3 Core + SDK changes

- `core/src/config.rs`: extend `ConfigDecl` (`min`, `max`, `enum`, `group`, `label`, `sensitive`);
  teach `materialize_config` and `generate_default_jsonc` to recurse sections; add range/enum
  validation. **Degrade rule (consistent with the existing wrong-type path):** an out-of-range or
  not-in-enum override → warn + fall back to the default (never fail the load).
- `@s2script/config` getters accept dotted keys (`section.key`); the injected values object nests.
- `packages/sdk/src/config-validate.ts` `validateConfigBlock`: validate the new optional fields, the
  section-vs-decl disambiguation, and that `default` satisfies `min`/`max`/`enum`.

### C.4 Framework configs modeled the same way — decided scope: per-plugin + admin + db

The admin templates (`admins.json`, `admin_groups.json`, `admin_overrides.json`) and the db template
(`databases.json`) are today hardcoded strings inside `core/src/v8host.rs`. Extract them into a
**single canonical declarative source** consumed by both:

1. a **build-time codegen** that re-embeds them into core (so the first-boot fallback still works
   with no repo access on the server), and
2. the **package-time generator** (C.5).

A `check-*-generated.sh`-style freshness gate (added to Workstream A's gate suite) fails the build if
the embedded copy drifts from the canonical source. `core.cfg`-style top-level framework config is
**out of scope** for this slice (deferred).

### C.5 Generate at package time

A generator step (exposed as `s2s config gen` and invoked from `package-release.sh`) enumerates every
**shipped** plugin manifest — `plugins/*/` **and** `plugins/disabled/*/` — plus the framework configs
from C.4, and writes into the staged zip:

- `configs/<plugin-id>.jsonc` per plugin (commented defaults; sections nested),
- `configs/admins.json`, `configs/admin_groups.json`, `configs/admin_overrides.json`,
  `configs/databases.json`.

First-boot generation in core stays as the fallback for hand-dropped `.s2sp` files (an operator who
drops a third-party plugin still gets its config auto-written). Enabling a shipped disabled plugin
finds its config already present.

### C.6 Registry-ready contract

The enriched manifest `config` block **is** the registry schema. The registry (future) reads it from
the published `.s2sp` / `package.json` and renders types, defaults, descriptions, ranges, enums, and
groups. Locking this format now is what keeps the registry a display layer over an existing model
rather than a retrofit. `sensitive` decls are masked in registry display.

---

## 3. Stack & orchestration

Three parallel Graphite sub-stacks, executed via **Fable-authored `Workflow` scripts** running Opus
and Sonnet subagents:

- **Opus** on the design-sensitive work: the config core-Rust changes (`config.rs` section recursion +
  validation), the framework-config extraction/codegen + drift gate, and the CI gating logic
  (`ci-ok` aggregation, path filter, reusable workflow).
- **Sonnet** on the mechanical work: the `disabled/` relocation + build-script edits, the
  package-time config generator plumbing, and the four disabled-plugin builds/typechecks.

Dependency notes for the workflow author:

- A is fully independent and can land first (it speeds every subsequent PR's CI).
- B and C both edit `package-release.sh` in **disjoint sections** (B: disabled-copy loop; C: config-gen
  hook) — sequence them or merge-resolve trivially.
- C.5 depends on B.1 (source layout) only for the `plugins/disabled/*/` enumeration path; C's core/SDK
  work (C.2–C.4) is independent of B.

Each PR runs the gate suite **per PR** (atomic-PR rule), not once at the top.

## 4. Boundaries (isolation check)

- **CI** touches only `.github/workflows/**`, `scripts/check-*.sh` invocation, and branch-protection
  settings. No product code.
- **Disabled relocation** touches repo layout, three build scripts, `package-release.sh` (one added
  loop), and the four plugins' source. No core/shim.
- **Config** touches `core/src/config.rs`, the config prelude + `materialize_for_load` region of
  `core/src/v8host.rs`, `packages/sdk/src/config-validate.ts` + `build.ts`, the new generator, and
  `package-release.sh` (one added hook). The framework-config extraction is the one place config work
  reaches into `v8host.rs`'s admin/db template strings.

## 5. Decisions locked

- CI: full restructure **minus** native merge queue (Graphite-managed merges). One required `ci-ok`
  check. PR builds are debug + `cargo test`; release stays sniper `--release` on tag.
- Disabled: source at `plugins/disabled/<name>`; shipped loader-skipped; four plugins to functional
  SM parity, live-gated.
- Config: file-backed JSONC (not ConVars); enriched format with sections + ranges + enums + groups +
  `sensitive`; per-plugin **and** admin + db framework configs; generated at package time and shipped;
  first-boot generation retained as fallback; `core.cfg`-style core config deferred.

## 6. Open items for the plan (not blockers)

- Per-plugin SM-parity checklists for the four disabled plugins (B.4).
- Exact canonical-source format for the extracted framework-config templates (C.4) — a `.jsonc`
  fixture set vs a small declarative manifest; either works with the drift gate.
- Whether `s2s config gen` also grows a `--check` mode reused by CI to assert the committed/shipped
  defaults match the manifests (a natural extension of the freshness-gate pattern).
