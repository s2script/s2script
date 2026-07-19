# Release-Hygiene Implementation Plan — CI · Config · Disabled-plugins

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the CI storm, ship the opt-in plugins and every default config in the release, and give config authoring a SourceMod-parity, registry-ready contract.

**Architecture:** Three independent Graphite sub-stacks run in parallel, each in its own worktree. Within a stack PRs are sequential (each builds on the last); across stacks they are parallel. Stack A (CI) is fully independent. Stacks B (disabled plugins) and C (config) both touch `scripts/package-release.sh` in disjoint sections and one shared enumeration path — the sequencing is called out per task.

**Tech Stack:** GitHub Actions (+ dorny/paths-filter, Swatinem/rust-cache, ccache), Rust (serde, V8-embedding core), TypeScript (`@s2script/sdk` build + `s2s` CLI), Bash packaging scripts, Graphite (`gt`).

This plan **supersedes** the spec where they differ — it incorporates the post-brainstorming Fable adversarial review (blocking findings A-1, A-2, A-3, A-4, B-1, B-2, C-1, C-2, C-3, C-4 and ordering hazards H-1…H-9). The spec is `docs/superpowers/specs/2026-07-19-ci-config-disabled-cleanup-design.md`.

---

## Global Constraints

Every task's requirements implicitly include this section.

- **Graphite atomic PRs.** Each PR passes the gate suite and is mergeable **on its own**. `gt track -p main` first in a fresh worktree, then `gt create <stack>/<change> -m "msg"` per PR. Never `git push` + one giant PR. Run the gate suite **per PR**, not once at stack top.
- **Gate suite (host is fine for gating; only the sniper `.so` + live-gate need Docker):**
  `./scripts/check-core-boundary.sh` · `cargo test -p s2script-core` (single-threaded, forced by `.cargo/config.toml` — do **not** pass `--test-threads`) · `./scripts/check-plugins-typecheck.sh` · `./scripts/check-schema-generated.sh` · `./scripts/check-nav-generated.sh` · `./scripts/check-events-generated.sh` · `./scripts/check-csitem-generated.sh` · `./scripts/test-boundary-nameleak.sh`.
- **Core is engine-generic.** `core/` must never import `games/*` (`check-core-boundary.sh` enforces). Config code stays engine-generic (no CS2 facts).
- **`package.json` is the authoring format.** Config is declared under the `s2script.config` block only. The runtime consumes the derived minimal manifest baked into the `.s2sp`, never the full `package.json`.
- **Contract versioning.** Widening a shipped `.d.ts` (`packages/sdk/config.d.ts`) or the manifest schema is an `apiVersion`-relevant change — note it in the PR body.
- **SDK PRs need a changeset** (`packages/**` triggers `changesets.yml`). Do **not** add `npm test` for the SDK to CI — there are 13 known pre-existing CLI test failures (schema-runtime + player-identity) that would make a new gate red on day one.
- **Do NOT submit or merge PRs, flip branch protection, or run the live gate from an implementation agent.** Build, gate, commit the Graphite stack, and stop. Submission, the branch-protection flip, and the Docker/human live gate are explicit post-implementation checkpoints (see "Checkpoints").

### Decisions resolved (post-Fable-review)

1. **Config filename contract:** KEEP the runtime's existing scheme. The generator MUST replicate `ConfigPath` in `shim/src/s2script_mm.cpp` exactly: id sanitized (every char not in `[A-Za-z0-9._-]` → `_`) + `.json` extension. `@s2script/funvotes` → `_s2script_funvotes.json`. JSONC content is fine (`strip_line_comments` tolerates `//`). Zero migration for deployed servers. A shared test asserts the TS sanitizer matches `ConfigPath`.
2. **Dot-in-key rule:** BAN `.` in config decl key names (SDK validate error + core WARN-and-skip). Dotted access (`config.getInt("voting.vote_duration")`) is therefore an unambiguous split-walk into sections.
3. **Old-runtime × sectioned-manifest:** A sectioned manifest hard-refuses to load on old core (whole-`Manifest` serde failure, not per-key degrade). Accepted and documented; sections are a new-core capability governed by `apiVersion`. Old flat manifests parse unchanged on new core (untagged fall-through).
4. **The four disabled plugins use FLAT v1 config only** in this slice (no `min`/`enum`/sections). Enriching them is deferred. (Their enriched fields would silently no-op on current core — forbidden here.)
5. **Generator input = the staged `.s2sp` manifest** (post-validation, exactly what ships), not source `package.json`.
6. **`bans.json` (state, managed by `sm_ban`) and `crashreporter.json` (opt-in; shipping a default would arm it) are OUT** of the package-time config set.
7. **`enum` allowed on `string` and `int` decls only; `enum` is mutually exclusive with `min`/`max`** (validate error if both). `sensitive: true` masks the value in registry display (still written to the file).
8. **Main-push (post-merge) job** runs the same debug build + full gate suite as PRs — no release build off `main`; the sniper release build stays tag-only.
9. **Branch-protection flip is a named manual checkpoint** owned by the repo admin (see Checkpoints), with the exact `gh api` command. The A stack keeps a job **named `deps`** until the flip.
10. **funvotes pass semantics:** add a `funvote_ratio` config (float, default `0.60`); a vote passes when yes-share ≥ ratio (SM behavior), replacing the current plurality `winner === 0`.

### Orchestration model

- Three parallel worktree agents, one per stack, model split: **Opus** on Stack C (recursive core serde + prelude) and Stack A (CI gating semantics); **Sonnet** on Stack B mechanical relocation/packaging, with Opus for the four plugin-parity PRs (behavioral).
- Within a stack, PRs are built sequentially in the same worktree so each stacks on the last.
- The whole run is **stop-on-first-failure**, **no PR submit**.

---

## Stack A — `ci-restructure/` (5 PRs + 1 manual checkpoint)

### Task A1: `ci-restructure/cache-debug-build`

**Files:**
- Modify: `.github/workflows/ci.yml` (the `build` job)
- Modify: `shim/CMakeLists.txt` (core-lib path parameterization — **required for atomicity**, finding A-1)

**Constraints / why:** PR build switches release→debug. `cargo build` (debug) emits `target/debug/libs2script_core.so`, but the shim links a **hardcoded** `${CMAKE_SOURCE_DIR}/../target/release/libs2script_core.so`. Both changes must land together or the shim link fails (or silently links a stale release core).

**Key implementation:**
- `shim/CMakeLists.txt`: introduce a cache var, default preserving current behavior so `make shim` / `build-sniper.sh` are untouched:
  ```cmake
  set(S2_CORE_LIB_DIR "release" CACHE STRING "cargo profile dir holding libs2script_core.so")
  # ...link against ${CMAKE_SOURCE_DIR}/../target/${S2_CORE_LIB_DIR}/libs2script_core.so
  ```
- `ci.yml` `build` job:
  - Add before the cargo step: `- uses: Swatinem/rust-cache@v2` (caches `~/.cargo/{registry,git}` + `target/`; also covers the ~130MB prebuilt V8 in `OUT_DIR`).
  - Replace `cargo build --release` with `cargo build` **and** `cargo test -p s2script-core`.
  - cmake configure adds `-DS2_CORE_LIB_DIR=debug`; enable ccache (`-DCMAKE_CXX_COMPILER_LAUNCHER=ccache`, cache `~/.cache/ccache` via `hendrikmuhs/ccache-action` or actions/cache).

**Tests / verify:** On a scratch branch, `cargo build && cargo test -p s2script-core` green; `cmake -S shim -B build/shim -DS2_CORE_LIB_DIR=debug && cmake --build build/shim -j` links against `target/debug`. Confirm `make shim` (no flag) still links `target/release`.

**Commit:** `gt create ci-restructure/cache-debug-build -m "ci: cache + debug PR build; parameterize shim core-lib dir"`

---

### Task A2: `ci-restructure/triggers-concurrency`

**Files:** Modify `.github/workflows/ci.yml` (top-level `on` + new `concurrency`).

**Constraints / why (A-4):** PRs must cancel-in-progress; `main` post-merge must NOT. One workflow, event-conditional cancellation. Do NOT put `paths:` at the workflow level (a non-triggered workflow leaves required checks pending forever — filter at the **job** level in Task A4).

**Key implementation:**
```yaml
on:
  pull_request:
  push:
    branches: [main]
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

**Verify:** `actionlint .github/workflows/ci.yml` clean; open a draft PR and confirm exactly one `build` run (not two).

**Commit:** `gt create ci-restructure/triggers-concurrency -m "ci: dedup push/pull_request; PR-only cancel-in-progress"`

---

### Task A3: `ci-restructure/gate-jobs`

**Files:** Modify `.github/workflows/ci.yml` (add `gates-node`, `gates-rust` jobs).

**Pre-flight (mandatory):** Run every gate on current `main` FIRST. If any is already red, STOP and report — this PR must not be the thing that discovers a pre-broken gate.
```bash
./scripts/check-core-boundary.sh && ./scripts/check-plugins-typecheck.sh && \
./scripts/check-schema-generated.sh && ./scripts/check-nav-generated.sh && \
./scripts/check-events-generated.sh && ./scripts/check-csitem-generated.sh && \
./scripts/test-boundary-nameleak.sh
```

**Constraints / why (H-1):** The gates job runs the freshness scripts by **glob**, so Stack C can add a new `check-*-generated.sh` with ZERO edits to `ci.yml`:
```yaml
- run: |
    set -e
    for f in scripts/check-*-generated.sh; do echo "== $f =="; bash "$f"; done
```
`gates-node` job: `actions/setup-node@22` + `npm ci` → the glob loop → `check-plugins-typecheck.sh` → `test-boundary-nameleak.sh`. `gates-rust` job: `rust-cache` + `check-core-boundary.sh` (needs only `cargo metadata`, no full build). These run parallel to `build`.

**Verify:** Both jobs green on a draft PR.

**Commit:** `gt create ci-restructure/gate-jobs -m "ci: run the full gate suite (typecheck + freshness + boundary + nameleak)"`

---

### Task A4: `ci-restructure/paths-filter-ci-ok`

**Files:** Modify `.github/workflows/ci.yml` (add `changes` job, job-level `if`s, `ci-ok` aggregator; **keep a job named `deps`**).

**Constraints / why (A-2, A-3):** `ci-ok` must fail CLOSED. A required check that reports `skipped` satisfies branch protection, so a naive aggregator lets red PRs merge. Exact shape:
```yaml
changes:
  runs-on: ubuntu-latest
  outputs:
    native: ${{ steps.f.outputs.native }}
    js: ${{ steps.f.outputs.js }}
  steps:
    - uses: actions/checkout@v4
    - uses: dorny/paths-filter@v3
      id: f
      with:
        filters: |
          native: ['core/**','shim/**','Cargo.*','third_party/**','gamedata/**','games/**']
          js: ['plugins/**','packages/**','examples/**','disabled/**','plugins/disabled/**']
# build: needs: changes; if: needs.changes.outputs.native == 'true'
# gates-node: needs: changes; if: needs.changes.outputs.js == 'true' || needs.changes.outputs.native == 'true'
# deps: (unchanged, ALWAYS runs — lockfile-drift guard for changesets, H-6)
ci-ok:
  needs: [changes, build, gates-node, gates-rust, deps]
  if: always()
  runs-on: ubuntu-latest
  steps:
    - name: gate
      run: |
        results='${{ join(needs.*.result, ',') }}'
        echo "results: $results"
        case "$results" in
          *failure*|*cancelled*) echo "a required job failed/cancelled"; exit 1 ;;
        esac
        echo "ok"
```
(`success` and `skipped` both pass; `failure`/`cancelled` fail.) `deps` stays a distinct job so branch protection can require it until the checkpoint flip.

**Verify:** Draft PR touching only `docs/**` → `build`/`gates-*` skipped, `deps` + `ci-ok` pass. Draft PR with a deliberately failing gate → `ci-ok` FAILS (does not skip).

**Commit:** `gt create ci-restructure/paths-filter-ci-ok -m "ci: path-filtered jobs + fail-closed ci-ok aggregator"`

---

### Task A5: `ci-restructure/reusable-build`

**Files:** Create `.github/workflows/_build.yml` (`workflow_call`); modify `ci.yml` (call it) and `release.yml` (call it for the sniper path, keeping the Docker/bullseye step).

**Constraints:** Keep the diff minimal; the `release.yml` side is only fully testable on a tag. `_build.yml` inputs: `profile` (`debug`/`release`), `core_lib_dir`.

**Verify:** `actionlint` clean; ci.yml build parity with A1.

**Commit:** `gt create ci-restructure/reusable-build -m "ci: extract reusable _build.yml (workflow_call) shared by ci + release"`

---

### Checkpoint A-M (manual, NOT a PR — repo admin):
After A1–A5 merge and `ci-ok` is observed reporting on a live PR:
```bash
gh api -X PUT repos/GabeHirakawa/s2script/branches/main/protection/required_status_checks \
  -f 'checks[][context]=ci-ok'   # replace deps→ci-ok (keep CodeRabbit if desired)
```
Then `gt restack && gt submit` any open B/C stacks so they report `ci-ok`.

---

## Stack B — `disabled-plugins/` (6 PRs)

### Task B1: `disabled-plugins/relocate`

**Files:**
- Move: `disabled/{nominations,rockthevote,funvotes,nextmap}` → `plugins/disabled/…` (`git mv`)
- Modify: `scripts/build-base-plugins.sh` (BOTH loops — version-stamp AND build, finding B-1), `scripts/check-plugins-typecheck.sh`
- Modify: `CLAUDE.md` (repo-layout `disabled/` line)
- Grep-sweep: any other `disabled/` path references

**Constraints / why (B-1):** This is atomic — moving files without the script edits breaks the gate suite mid-stack. Critically, `build-base-plugins.sh`'s **build loop** (`for d in plugins/*/`) has never built disabled plugins; add `plugins/disabled/*/`. Both loops must skip the bare `plugins/disabled/` container dir (`[ -f "$d/package.json" ]` guard already does).

**Key implementation:** stamp loop `for d in plugins/*/ disabled/*/` → `for d in plugins/*/ plugins/disabled/*/`; build loop likewise. `check-plugins-typecheck.sh`: change `for base in examples plugins disabled` to also walk `plugins/disabled/*` (drop the bare `disabled`).

**Verify:** `./scripts/build-base-plugins.sh` builds 4 disabled `.s2sp` under `plugins/disabled/*/dist/`; `./scripts/check-plugins-typecheck.sh` passes for all incl. `plugins/disabled/*`.

**Commit:** `gt create disabled-plugins/relocate -m "chore(plugins): move disabled/ under plugins/disabled/; fix build+typecheck walks"`

---

### Task B2: `disabled-plugins/ship-loader-skipped`

**Files:** Modify `scripts/package-release.sh`; create `plugins/disabled/README.txt` content (written by the script).

**Constraints / why (H-2):** Insert BETWEEN the plugin-copy loop (≈lines 87–97) and the zip step; C5 also inserts here — sequence B2 before C5. Loader-skip is already correct (`loader.rs` `read_dir` is non-recursive — no runtime change).

**Key implementation:** after the enabled-plugin loop, add:
```bash
mkdir -p "$STAGE/addons/s2script/plugins/disabled"
disabled_count=0
shopt -s nullglob
for s2sp in plugins/disabled/*/dist/*.s2sp; do
    cp "$s2sp" "$STAGE/addons/s2script/plugins/disabled/"; disabled_count=$((disabled_count+1))
done
shopt -u nullglob
[ "$disabled_count" -gt 0 ] || { echo "ERROR: no disabled .s2sp — run build-base-plugins.sh" >&2; exit 1; }
cat > "$STAGE/addons/s2script/plugins/disabled/README.txt" <<EOF
Opt-in plugins ($disabled_count). Move a .s2sp up one level (into plugins/) to enable it.
EOF
```

**Verify:** `bash scripts/package-release.sh 0.0.0-dev` → zip contains `addons/s2script/plugins/disabled/*.s2sp` (4 files) + README.

**Commit:** `gt create disabled-plugins/ship-loader-skipped -m "release: ship opt-in plugins under plugins/disabled/ (loader-skipped)"`

---

### Tasks B3–B6: plugin parity (one file each; independent of each other; each passes the typecheck gate alone; ONE live-gate at stack top covers all four)

Each: `plugins/disabled/<name>/src/plugin.ts`. **Flat v1 config only** (Decision 4). Per-plugin parity checklist:

- **B3 `disabled-plugins/rtv-parity` (rockthevote):** FIX standalone breakage (B-2) — rtv must `CREATE TABLE IF NOT EXISTS` its own `mapvote` schema (idempotent alongside nominations) or treat missing tables as empty, so `startVote`→`buildBallot` can't reject on a fresh install. Add `rtv_initialdelay` (int secs; refuse rtv in a map's first N secs). Keep: threshold/minplayers/cooldown/ballot-cap/Don't-Change/round_end-apply/force-rtv/disconnect-cleanup/tie-keeps-map. Document the deliberate deviation (SM delegates to mapchooser; ours inlines the vote).
- **B4 `disabled-plugins/nominations-parity`:** Make current-map exclusion explicit (not merely implied by `map_cooldown ≥ 1`). Pin the exact menu exclusion set (cooldown + already-nominated + current map). `sm_nominate_addmap` (admin) → descope explicitly.
- **B5 `disabled-plugins/funvotes-parity`:** Replace plurality pass with `funvote_ratio` (Decision 10). Keep votealltalk/voteff/votegravity/voteslay. `sm_voteburn` → **descope explicitly** (needs an ignite primitive that doesn't exist; do not invent RE work). votegravity multi-value menu → single-value, note deviation.
- **B6 `disabled-plugins/nextmap-parity`:** Fix maplist ownership — nextmap must not depend on nominations generating `maplist.txt`; either own an idempotent list source or degrade to empty gracefully. `sm_maphistory` → **descope** (would couple to nominations' DB); do not quietly couple the plugins.

**Verify (each):** `cd plugins/disabled/<name> && npx s2script build` + `./scripts/check-plugins-typecheck.sh`. Behavioral verification is the live gate (Checkpoint B-M) — chat `rtv`/nominate menu/vote casting need a **human client** (bots are filtered; `rcon say` is server console). rcon-drivable: `sm_forcertv`, `sm_setnextmap`, `mp_maxrounds 1` (round_end), fractional `mp_timelimit`, `Server.mapName`.

**Commits:** `gt create disabled-plugins/<name>-parity -m "feat(<name>): SM parity — <one-line>"`

---

### Checkpoint B-M (live gate, human-in-the-loop): one Docker session covering B3–B6 — see Checkpoints.

---

## Stack C — `config-system/` (6 PRs)

### Task C1: `config-system/core-sections-validation`

**Files:** Modify `core/src/config.rs`, `core/src/loader.rs` (the `Manifest.config` field), `core/src/v8host.rs` (the `@s2script/config` prelude getters ≈1186–1189 + `store_config_decls`/`materialize_for_load`/`re_materialize_config`).

**Constraints / why (C-2, C-3):** Sections require a **recursive type change at the Manifest boundary**, not just recursion in one function. A flat `HashMap<String, ConfigDecl>` with required `type: String` refuses a whole sectioned Manifest at serde.

**Key implementation:**
```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ConfigEntry {
    Decl(ConfigDecl),                       // matched first: requires `type`
    Section(std::collections::HashMap<String, ConfigEntry>),
}
// ConfigDecl gains: #[serde(default)] min/max: Option<f64>, enum_: Option<Vec<Value>> (rename "enum"),
//                   group/label: Option<String>, sensitive: #[serde(default)] bool
```
Thread `ConfigEntry` through `loader::Manifest.config`, `PluginInstance.config_decls`, `materialize_config`, `generate_default_jsonc` (recurse sections → nested JSONC), `re_materialize_config`. Degrade rule (matches existing wrong-type path): out-of-range / not-in-enum override → WARN + fall back to default; never fail load. Ban `.` in a decl key → WARN-and-skip. Prelude getters: split dotted key, walk nested `__s2pkg_config_values`.

**Tests (`cargo test -p s2script-core`):** flat manifest parses unchanged; sectioned manifest materializes nested; **section whose child decl is literally named `type` classifies as a section** (the untagged edge case); out-of-range override → default + 1 warning; dotted getter reads a nested value; dotted key with a banned literal-dot decl is skipped.

**Verify:** `cargo test -p s2script-core` green; `./scripts/check-core-boundary.sh` green.

**Commit:** `gt create config-system/core-sections-validation -m "core(config): recursive ConfigEntry (sections) + range/enum + dotted getters"`

---

### Task C2: `config-system/sdk-validate-types`

**Files:** Modify `packages/sdk/src/config-validate.ts`, `packages/sdk/config.d.ts`; add a changeset.

**Constraints / why (C-2, C-3):** The disambiguation + dot-ban + field rules MUST match C1 exactly (agents share this rule text). Refined rule: an entry is a **decl** iff it has a `type` key whose value ∈ `{string,int,float,bool}`; otherwise a **section** (recurse). Validate: `default` satisfies `min`/`max`/`enum`; `enum` only on `string`/`int` and mutually exclusive with `min`/`max` (Decision 7); ban `.` in key names; `sensitive` boolean.

**Key implementation:** `config.d.ts` becomes recursive:
```ts
export type ConfigValue = string | number | boolean | { [k: string]: ConfigValue };
export type Config = Record<string, ConfigValue>;
```
(widening a shipped `.d.ts` — note apiVersion in the PR body.)

**Tests:** existing flat blocks still valid; a section validates + recurses; `enum` + `min` together → error; dotted key → error; default out of `enum` → error.

**Verify:** `./scripts/check-plugins-typecheck.sh` green (all existing plugins still validate).

**Commit:** `gt create config-system/sdk-validate-types -m "sdk(config): enriched validation (sections/min/max/enum/sensitive) + recursive Config type"` (+ changeset)

---

### Task C3: `config-system/framework-templates`

**Files:** Create `core/config-templates/{admins,admin_groups,admin_overrides,databases}.json`; modify `core/src/v8host.rs` (delete the inline template string literals ~1887–1889, 2176; inject via `include_str!`).

**Constraints / why (C-4):** Replaces the spec's fragile codegen+drift-gate. `include_str!` the canonical files into Rust statics, build a `globalThis.__s2_TEMPLATES` object via V8 string creation (no JS-escaping of the raw prelude), and point `__s2_admin_readOrTemplate` / the db loader at it. ONE source; NO generated copy; NO drift gate. First-boot behavior is preserved (template written only when the operator file is absent).

**Tests:** `cargo test -p s2script-core` — a test that `__s2_TEMPLATES.admins` parses as JSON and matches the file; admin cache still resolves from the template. Existing admin/db in-isolate tests stay green.

**Verify:** `cargo test -p s2script-core` green.

**Commit:** `gt create config-system/framework-templates -m "core(config): canonical framework templates via include_str! (drops inline literals + drift gate)"`

---

### Task C4: `config-system/config-gen-cli`

**Files:** Create `packages/sdk/src/config/gen.ts` + a shared sanitizer `packages/sdk/src/config/config-path.ts`; wire `s2s config gen` in the CLI; add a changeset.

**Constraints / why (C-1, Decision 1 & 5):** Input = **staged `.s2sp` manifests** (post-validation). Output filenames MUST match `ConfigPath`: sanitize id (`[^A-Za-z0-9._-]` → `_`) + `.json`. Emit commented JSONC (defaults + `// type — description`, sections nested) via the same logic as core's `generate_default_jsonc` (keep them behaviorally identical). CLI stays **plugin-scoped** — it does NOT know about framework templates (third-party `s2s` users have none).

**Tests (node:test):** a **sanitizer-parity test** asserting the TS `configPath()` equals the C++ `ConfigPath` for a table of ids (`@s2script/funvotes` → `_s2script_funvotes.json`, `a/b c` → `a_b_c.json`); a gen test producing nested JSONC for a sectioned manifest.

**Verify:** `s2s config gen` over a built plugin dir emits the correctly-named file.

**Commit:** `gt create config-system/config-gen-cli -m "sdk(config): s2s config gen — ConfigPath-matching default files"` (+ changeset)

---

### Task C5: `config-system/package-time-gen`

**Files:** Modify `scripts/package-release.sh` (add a config-gen hook after staging plugins).

**Constraints / why (H-2, H-3):** Sequence AFTER B2 (shared file) and after B1's build-loop fix (so disabled manifests exist to enumerate). Two steps: (a) run `s2s config gen` over every staged `.s2sp` (enabled AND `plugins/disabled/*`) → `$STAGE/addons/s2script/configs/`; (b) `cp` the C3 canonical framework files (`admins.json`, `admin_groups.json`, `admin_overrides.json`, `databases.json`) into the same dir. The framework `cp` is a **shell step here**, not baked into the published CLI. Explicitly exclude `bans.json`/`crashreporter.json` (Decision 6).

**Verify:** `bash scripts/package-release.sh 0.0.0-dev` → `configs/` contains `_s2script_*.json` for every shipped plugin (incl. disabled) + the 4 framework files; first-boot on a live server writes NO new config (files already present + correctly named).

**Commit:** `gt create config-system/package-time-gen -m "release: generate + ship all default configs (plugins + admin + db) at package time"`

---

### Task C6 (optional, spec §6): `config-system/gen-check`

**Files:** Add `s2s config gen --check` + `scripts/check-configs-generated.sh` (glob-picked-up by A3's job automatically, H-1).

**Commit:** `gt create config-system/gen-check -m "sdk(config): --check mode + freshness gate"`

---

## Checkpoints (human / out-of-band — NOT done by implementation agents)

- **A-M — branch-protection flip:** after Stack A merges, run the `gh api` command in Task A4's checkpoint; restack open stacks.
- **B-M — live gate:** one Docker CS2 session (`make docker-test`; `docker exec … /patch-gameinfo.sh`; `docker compose … restart cs2`; `python3 scripts/rcon.py`). Human client required for chat-`rtv` / nominate-menu / vote-casting; rcon covers `sm_forcertv`/`sm_setnextmap`/round_end/timelimit/`Server.mapName`. Remember: `docker compose restart` (NOT `--force-recreate`, which wipes `gameinfo.gi`); a plain restart may not cycle the process (verify `StartedAt` + `/proc/maps`).
- **C live check:** deploy the packaged zip to the Docker server; confirm shipped `configs/*.json` are read (no first-boot regeneration) and a value change hot-reloads via `onChange`.

---

## Self-Review

**Spec coverage:** A.1–A.7 → Tasks A1–A5 + A-M (merge queue explicitly excluded, Decision/A.6). B.1–B.4 → B1–B6 + B-M. C.1–C.6 → C1–C6 + C-live. Framework-config extraction (C.4) → Task C3 (via the better `include_str!` design). Generate-at-package-time (C.5) → Task C5. Registry-ready (C.6) → the enriched manifest schema locked in C1/C2.

**Fable blocking findings mapped:** A-1→A1 (CMake param) · A-2→A4 (fail-closed ci-ok) · A-3→A4/A-M (keep `deps`, sequenced flip) · A-4→A2 (event-conditional cancel) · B-1→B1 (build loop) · B-2→B3 (rtv standalone) · C-1→C4 (ConfigPath parity + test) · C-2→C1 (ConfigEntry enum) · C-3→C1/C2 (dot ban + recursive `.d.ts`) · C-4→C3 (include_str!). Ordering hazards H-1→A3 (glob gate), H-2/H-3→B2/C5 sequencing, H-4→Decision 4, H-6→A4 (`deps` always-runs), H-7→Global Constraints (changeset, no SDK npm test), H-8→docs edits in each stack's last PR.

**Placeholder scan:** none — every descope (sm_voteburn, sm_maphistory, sm_nominate_addmap) is an explicit decision, not a TODO.

**Type consistency:** `ConfigEntry`/`ConfigDecl` (C1) ↔ `config-validate.ts` rule (C2) ↔ `configPath()` (C4, matches `ConfigPath`) ↔ `generate_default_jsonc` behavior shared by C1 core + C4 CLI.
