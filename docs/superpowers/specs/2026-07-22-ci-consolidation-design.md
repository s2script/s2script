# CI consolidation + one-PR-per-slice — design spec

**Date:** 2026-07-22
**Status:** approved (brainstorming) → ready for planning
**Supersedes:** Workstream A of `2026-07-19-ci-config-disabled-cleanup-design.md` (that spec's §A.4
single-required-check and §A.5 reusable-build decisions are reversed here; §A.1–A.3 stand).

**Scope of THIS spec:** collapse PR CI from seven check runs to at most two, make the gate suite a
single source of truth shared by CI and humans, and retire the Graphite stacked-PR doctrine in
favour of one PR per slice.

---

## 1. Goal

Four concrete outcomes, one per pain:

1. **Fewer boxes.** A PR shows 0, 1, or 2 checks instead of 7.
2. **No stack multiplier.** Graphite stacking is retired; a slice is one branch and one PR, so a
   6-PR stack's 42 check runs becomes 1 PR's 1–2.
3. **Latency where it's payable.** Docs-only and JS-only PRs stop paying the ~4-minute native
   build. The native critical path itself is unchanged — see §6.
4. **One gate suite, not three.** CLAUDE.md's list, `ci.yml`, and reality are three hand-maintained
   copies today. They collapse into two scripts that CI and humans both run.

---

## 2. Background / the gaps today

`.github/workflows/ci.yml` spawns seven check runs per PR. Measured medians over the last 12 CI
runs of the predecessor repo:

| job | median | max | what it actually does |
|---|---:|---:|---|
| `build` (via `_build.yml`) | 119s | 119s | `cargo build`, `cargo test`, boundary check, shim cmake |
| `gates-node` | 104s | 116s | `npm ci`, 4 codegen-freshness checks, plugin typecheck, nameleak |
| `gates-rust` | 56s | 56s | Rust toolchain + cache restore → **one 40-line script** |
| `gates-licenses` | 55s | 55s | Rust toolchain + `cargo fetch --locked` → notice freshness |
| `deps` | 11s | 15s | `npm ci`, nothing else |
| `changes` | 7s | 8s | `dorny/paths-filter` orchestration |
| `ci-ok` | 3s | 4s | fail-closed aggregator |

The `build` figure is the **post-restructure, warm-cache** job (`88859042220`); earlier runs of a
same-named job took 233–292s but predate `Swatinem/rust-cache` landing in the 2026-07-19 spec's
§A.2, so they are not representative. Its internal breakdown matters for §7:

| step | time |
|---|---:|
| `actions/checkout` (`submodules: recursive`) | 21s |
| `dtolnay/rust-toolchain` install | 10s |
| `Swatinem/rust-cache` restore | 13s |
| `cargo build` | 13s |
| `cargo test -p s2script-core` | 17s |
| `hendrikmuhs/ccache-action` setup | 9s |
| shim cmake build | 17s |
| rust-cache save + post steps | 13s |
| **total** | **116s** |

**Compilation is 47s of 116s — 60% of the native job is fixed overhead, not compiling.**

**Both load-bearing justifications for that structure have expired.**

- §A.4 of the 2026-07-19 spec introduced `changes` + `ci-ok` so that "`ci-ok` becomes the **one
  required status check** in branch protection." That flip never happened: this repo is private on
  a free plan, where `GET /branches/main/protection` and `GET /rulesets` both return
  `403 Upgrade to GitHub Pro`. Two of the seven boxes serve a feature that cannot be enabled.
- §A.6 rejected a merge queue *specifically because* merges go through Graphite. Graphite is being
  retired (§3), so that constraint dissolves.

**Redundancy.** On a native PR, `scripts/check-core-boundary.sh` executes **seven times**: once in
`build` (`_build.yml:37`), once as the entire reason `gates-rust` exists, and five more times inside
`test-boundary-nameleak.sh`, which plants and removes probe files and re-invokes the gate at each
step. Every invocation shells out to `cargo metadata` **and** `cargo tree`. `npm ci` runs twice
(`deps` and `gates-node`).

**Stale filters.** `ci.yml:47` filters on `disabled/**`, a directory that no longer exists — it was
relocated to `plugins/disabled/` by Workstream B of the previous spec. And `package.json` /
`package-lock.json` appear in **no** path filter at all, which is the sole reason `deps` had to be
carved out as a deliberately unfiltered job.

**Orphaned tests.** Four test scripts exist, run in seconds, and are executed by nothing — not CI,
not CLAUDE.md's gate list, not the Makefile:

| script | guards |
|---|---|
| `scripts/check-activity-test.sh` | `games/cs2/js/activity.js` show-activity decision logic |
| `scripts/check-antiflood-test.sh` | `plugins/antiflood/src/flood.ts` token-decay flood model |
| `scripts/test-sigscan.sh` | `shim/src/sigscan.cpp` pure pattern scanner (host g++) |
| `scripts/test-gate.sh` | `scripts/gate.sh` port claiming + `scripts/rcon.py --port` parsing + `docker/pre.sh` invariants |

`scripts/check-activity-test.sh`, `scripts/check-antiflood-test.sh` and `scripts/test-gate.sh` have
zero references anywhere outside themselves. `scripts/test-sigscan.sh` is referenced only by a
completed 2026-07-03 plan document.

**A self-contradicting sentence.** `docs/sdk-doc-conventions.md:5` reads: "Coverage is enforced
per-PR by `scripts/check-doc-coverage.mjs` (a dev tool, not a CI gate)." Both halves are in one
sentence and they contradict each other — "enforced per-PR" describes a gate, and the parenthetical
denies it. The plan for that slice (`2026-07-21-sdk-tsdoc-intellisense.md:17`) is authoritative:
"**The analyzer is a dev tool, not a CI gate.** Do NOT add `check-doc-coverage` to `.github/` or the
gate suite." Only the "enforced per-PR" clause needs rewording.

---

## 3. Workflow doctrine — Graphite is retired

**Decision: one branch → one PR per slice, squash-merged.** Plain `git` + `gh pr create`.

The CLAUDE.md section "Ship work as a stack, not a branch (Graphite)" (lines 70–97) is deleted in
full, including: the `gt` command reference, the "always argue for more PRs, never fewer" mandate,
the "no change is too small" rule, the `terse-stack-name/terse-change` branch convention, the
"plan the stack before writing code" step, and "run the gate suite **per PR**, not once at the top."

It is replaced by a short section stating that a slice is one branch and one PR; that the PR is as
big as the slice is; that a PR body still carries **Why**; and that PR bodies are still written with
the Write tool to a file and passed via `gh pr edit N --body-file` (never a heredoc — shell escaping
mangles tables and code blocks). That last rule is Graphite-independent and survives.

**A pre-merge gate is retained** rather than going trunk-based, because a push to `main` auto-fires
`changesets.yml`, which publishes to npm. There must be a gate between a bad commit and the
registry.

Historical plan and spec documents under `docs/superpowers/` that mention `gt` are records of what
was done at the time and are **not** rewritten.

---

## 4. The new CI topology

Two workflows, one job each, path-filtered **at the workflow level** — so filtering costs zero jobs.

### 4.1 `.github/workflows/ci-native.yml`

Triggers: `pull_request` and `push: branches: [main]`, both path-filtered on:

```
core/**  shim/**  games/**  Cargo.toml  Cargo.lock  third_party/**
licenses/**  LICENSE  packages/*/LICENSE-*
scripts/gen-licenses.sh  scripts/check-licenses-generated.sh
scripts/check-core-boundary.sh  scripts/check-core-names.sh
scripts/test-boundary-nameleak.sh  scripts/test-sigscan.sh  scripts/ci-native.sh
.github/workflows/ci-native.yml
```

Steps: `actions/checkout@v4` (`submodules: recursive` — `gen-licenses.sh` reads the vendored
`third_party/` submodules), `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`,
`hendrikmuhs/ccache-action@v1`, then `bash scripts/ci-native.sh`.

**rust-cache is restore-only on PRs:**

```yaml
- uses: Swatinem/rust-cache@v2
  with:
    save-if: ${{ github.ref == 'refs/heads/main' }}
```

Two reasons. It returns the 13s save step per PR (§2). More importantly it stops every branch
minting its own ~383 MB cache entry: the predecessor repo reached **77 entries / 4.34 GB** against a
10 GB quota, and LRU eviction of a rust-cache entry is precisely what turns a 13s `cargo build` into
a 3-4 minute one. Only `main` populates the cache; PRs restore from it. Consolidating the three
current per-job Rust caches (`build` 383 MB, `gates-rust` 109 MB, `gates-licenses` 109 MB — one per
job, all keyed separately) into this single job compounds the same win.

Accepted cost: a PR that changes `Cargo.lock` misses the cache and rebuilds fully on every push
until it merges. That is 9 PRs in 60 days (§7).

### 4.2 `.github/workflows/ci-js.yml`

Triggers: `pull_request` and `push: branches: [main]`, both path-filtered on:

```
plugins/**  packages/**  examples/**  games/**  gamedata/**  scripts/**  docker/**
package.json  package-lock.json  tsconfig.base.json
.github/workflows/ci-js.yml
```

Steps: `actions/checkout@v4` (**no** submodules — nothing in the JS gate reads them, so the
checkout gets cheaper), `actions/setup-node@v4` (node 22, `cache: npm`), then
`bash scripts/ci-js.sh`.

`docker/**` is in the filter because `test-gate.sh` asserts on `docker/pre.sh`,
`docker/docker-compose.yml` and `docker/docker-compose.gate.yml`. `scripts/**` is deliberately broad
and overlaps `ci-native`'s filter; overlap means both run, which is correct.

`games/**` appears in **both** filters on purpose: `games/cs2/` holds both a Rust crate
(`games/cs2/Cargo.toml`) and the JS prelude (`games/cs2/js/*.js`).

`gamedata/**` appears in **`ci-js` only**, and this is load-bearing rather than an oversight.
Nothing compiles it in: `core/build.rs` only emits a link arg, no `include_str!`/`include_bytes!`
in `core/src` reads it, and the shim resolves `gamedata/core.gamedata.jsonc` from disk **at
runtime** (`shim/src/gamedata.cpp`, `shim/src/s2script_mm.cpp:2199`). No gate script reads it at
all: the codegen-freshness checks read `games/cs2/gamedata/` (covered by the `games/**` entry),
and the top-level directory's only consumer is `scripts/package-addon.sh` at packaging time. It
stays on `ci-js` rather than `ci-native` because if it must fire something, the cheap suite is the
right one — but the load-bearing half of this decision is its absence from the native filter. So a gamedata edit
must trigger the JS job and must **not** trigger a native rebuild — which is the CLAUDE.md
invariant "layout is data, semantics are code; a field-offset change must never require a code
change", expressed as a path filter. If a future change makes `core/` or `shim/` embed gamedata at
compile time, that invariant has been broken and the filter is the wrong thing to fix.

### 4.3 Resulting box count

| PR touches | checks shown |
|---|---:|
| docs only | **0** |
| `plugins/`, `packages/`, `examples/` | **1** (`ci-js`) |
| `core/`, `shim/`, `Cargo.*` | **1** (`ci-native`) |
| `games/`, `gamedata/`, `scripts/` | **2** |

### 4.4 Concurrency and permissions

Each workflow carries:

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
permissions:
  contents: read
```

`pull-requests: read` is dropped — it existed only for `dorny/paths-filter`'s "list PR files" API
call, and `paths-filter` is gone. `cancel-in-progress` stays false on `main` pushes: those must run
to completion, because a green `main` is what gates the changesets publish.

### 4.5 Deletions

| deleted | why it's safe |
|---|---|
| `.github/workflows/ci.yml` | replaced by the two above |
| `.github/workflows/_build.yml` | its only remaining consumer is removed (§4.6) |
| job `changes` | workflow-level `paths:` replaces it |
| job `ci-ok` | aggregates for a required-check flip that cannot be enabled (§2, §7) |
| job `deps` | its `npm ci` is a step in `ci-js.sh`; `package*.json` now enters a path filter |
| job `gates-rust` | ran only `check-core-boundary.sh`, which `ci-native.sh` runs |
| job `gates-licenses` | folds into `ci-native.sh`, whose cargo registry is already warm |

### 4.6 `release.yml`

The `build` job (a `_build.yml` call at `profile: release`) and the `needs: build` on the `release`
job are both removed; `release` becomes the sole job and goes straight to the bullseye sniper build.

Rationale: that job builds release-profile on `ubuntu-latest`, a configuration that is neither PR CI
(debug, ubuntu) nor the shipped artifact (release, bullseye, glibc 2.31). It buys roughly four
minutes of earlier failure on a tag push, where nobody is iterating, and the sniper build is itself
the real test. Removing it also removes `_build.yml`'s last consumer, so the reusable-workflow
indirection disappears entirely.

---

## 5. The gate suite becomes two scripts

`scripts/ci-native.sh` and `scripts/ci-js.sh` hold the gate sequence. The workflow files call them;
so do humans. There is exactly one copy.

**`scripts/ci-native.sh`** — cheap gates first so a boundary violation fails in seconds rather than
after a four-minute build:

1. `cargo fetch --locked` (needed by the license gate; warms the registry for the build)
2. `scripts/check-core-boundary.sh`
3. `scripts/test-boundary-nameleak.sh`
4. `scripts/test-sigscan.sh`
5. `scripts/check-licenses-generated.sh`
6. `cargo build`
7. `cargo test -p s2script-core`
8. `cmake -S shim -B build/shim … && cmake --build build/shim -j`

**`scripts/ci-js.sh`:**

1. `npm ci` — the lockfile-drift guard, **guarded on `$CI`**. On a local run it is skipped with a
   printed note ("set `CI=1` to run the lockfile guard"), because `npm ci` deletes `node_modules`
   and a gate script must not be destructive to a working tree.
2. every `scripts/check-*-generated.sh` **except** `check-licenses-generated.sh` (which needs the
   Rust toolchain and runs in `ci-native.sh`). Kept as a glob, preserving the existing property that
   a future freshness check needs no CI edit to start running.
3. `scripts/check-plugins-typecheck.sh` (the 5E.1 gate)
4. `scripts/check-activity-test.sh`
5. `scripts/check-antiflood-test.sh`
6. `scripts/test-gate.sh`

Note `test-boundary-nameleak.sh` **moves out of the JS job** into `ci-native.sh`, where a Rust
toolchain and a warm cargo cache actually exist. It was only ever passing in `gates-node` because
`ubuntu-latest` happens to ship a Rust toolchain.

**Makefile targets:** `make ci-native`, `make ci-js`, and `make ci` (both). Local green now *means*
CI green, because it is the same script.

---

## 6. Boundary script split

Today `check-core-boundary.sh` does two unrelated things in one file: a cargo dependency-closure
walk (`cargo metadata` + `cargo tree`, the slow part) and two greps over `core/src` (the CS2
name-leak patterns and the `include_str!`/`games/` gate, both instant). `test-boundary-nameleak.sh`
calls the whole thing five times to plant and remove probe files — so the closure walk runs five
times to test a grep.

| file | change |
|---|---|
| `scripts/check-core-names.sh` | **new.** The two greps only. No cargo. |
| `scripts/check-core-boundary.sh` | **modified.** Keeps the closure walk, then calls `check-core-names.sh`. External behaviour identical, so `make check-boundary` and every existing call site are unchanged. |
| `scripts/test-boundary-nameleak.sh` | **modified.** Calls `check-core-names.sh` directly instead of `check-core-boundary.sh`. Same five probe/clean cycles, same assertions. |

Cargo dependency walks on a native PR: **7 → 1**.

---

## 7. Explicitly out of scope

- **Native critical-path latency.** A native PR takes ~2 minutes (§2), of which only 47s is
  compilation. Consolidating jobs does not shrink that; the latency win in this spec is that docs
  and JS-only PRs stop paying it at all, plus the `save-if` change in §4.1. Splitting the shim into
  a parallel job would trim ~17s at the cost of a check box — rejected, against the primary goal.

- **`sccache`. Evaluated and rejected**, with the numbers, so this isn't relitigated:
  1. On a warm `target/` cargo never invokes `rustc`, so the `RUSTC_WRAPPER` is never consulted.
     No effect on ~99% of PRs.
  2. The case it would help is a `Cargo.lock` change (which misses rust-cache's key and forces a
     full rebuild of all 197 transitive deps). `Cargo.lock` changed **9 times in 60 days** against
     **1006 commits** — under 1%.
  3. It worsens the real constraint. GitHub's cache quota is 10 GB/repo with LRU eviction, and
     eviction is what *causes* cold builds. The predecessor repo held **4.34 GB across 77 entries**.
     sccache would add a second GHA-backed cache population competing for the same quota.
  4. It cannot cache the expensive part of a cold build anyway: the `v8 = "149.4.0"` build script
     downloads a ~130 MB prebuilt `librusty_v8.a`, and sccache caches `rustc` invocations, not
     build-script execution or downloaded artifacts. It would be an addition to rust-cache, not a
     replacement.
- **Merge queue.** No branch protection exists to queue against.
- **Gating `check-doc-coverage.mjs`.** Honours the 2026-07-21 plan's explicit decision. Instead,
  `docs/sdk-doc-conventions.md:5` is corrected to describe it as a local tool run while authoring
  stubs, not a per-PR gate.
- **Rewriting historical plan/spec docs** that reference `gt` (§3).

### Recorded tripwire

If this repo goes public and rulesets are enabled, a workflow skipped by a `paths:` filter **never
reports a status**, so any required check pointing at it hangs forever. `ci-ok` was insurance
against exactly this. The fix at that point is to re-add a small `ci-ok`-style aggregator job — a
~10-line addition, not a restructure. This is a deliberate trade: pay nothing now for a feature that
cannot currently be turned on.

---

## 8. Decisions locked

1. Graphite stacking retired; one branch → one PR per slice, squash-merged. Pre-merge gate retained
   because `main` pushes publish to npm.
2. Two path-filtered workflows, one job each. Max two check boxes per PR.
3. The four orphaned test scripts are wired in, not deleted.
4. `_build.yml` deleted; `release.yml` goes straight to the sniper build.
5. `docs/sdk-doc-conventions.md` corrected; `check-doc-coverage.mjs` stays a dev tool.
6. `check-core-boundary.sh` split, preserving external behaviour at every existing call site.
7. Gate suite lives in `scripts/ci-native.sh` + `scripts/ci-js.sh`, shared by CI and humans.

## 9. Success criteria

- A docs-only PR shows **zero** checks; a plugin PR shows **one**; a `core/` PR shows **one**.
- `scripts/ci-js.sh` and `scripts/ci-native.sh` pass locally on a clean tree, and the workflow files
  contain no gate step that is not in those scripts.
- `grep -rn 'gt submit\|gt create\|gt restack' CLAUDE.md` returns nothing.
- Each of the four formerly-orphaned scripts appears in exactly one of the two gate scripts.
- `make check-boundary` keeps its exit-code behaviour across the split: 0 on a clean tree, non-zero
  on a planted violation of **either** half (a `games/*` dependency edge, or a CS2 identifier /
  `include_str!("…games/…")` in `core/src`). Its stdout gains one line from the extracted script;
  no caller parses it.
- A deliberately planted CS2 identifier in `core/src` still fails `ci-native`.
