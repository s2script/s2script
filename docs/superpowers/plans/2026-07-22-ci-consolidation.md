# CI Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse PR CI from seven check runs to at most two, make the gate suite one source of truth shared by CI and humans, and retire the Graphite stacked-PR doctrine.

**Architecture:** Two GitHub Actions workflows (`ci-native.yml`, `ci-js.yml`), one job each, path-filtered at the *workflow* level so filtering costs zero jobs. Each job's entire gate sequence lives in a shell script (`scripts/ci-native.sh`, `scripts/ci-js.sh`) that humans run via `make ci`. The workflow YAML shrinks to checkout + toolchain + one `bash` line.

**Tech Stack:** GitHub Actions, bash, GNU make, `Swatinem/rust-cache@v2`, `hendrikmuhs/ccache-action@v1`, cargo, cmake, node 22.

**Spec:** `docs/superpowers/specs/2026-07-22-ci-consolidation-design.md`

## Global Constraints

- **Branch:** `ci/consolidation`, already checked out, spec already committed. Do not create sub-branches — this slice is one PR (that is the doctrine this plan installs).
- **Every gate script:** `#!/usr/bin/env bash`, `set -euo pipefail`, and `cd "$(dirname "$0")/.."` as the first action so it runs correctly from any cwd. Exception: `test-boundary-nameleak.sh` already uses `set -uo pipefail` (no `-e`, deliberately — it asserts on non-zero exits); do not add `-e` to it.
- **No gate may run in CI that is not in `scripts/ci-native.sh` or `scripts/ci-js.sh`.** The workflow YAML calls one script and nothing else. This is the whole point of the slice.
- **Node version in CI:** `"22"` with `cache: npm` (must match `changesets.yml`, which pins the same, so the two can never disagree about `npm ci`).
- **`npm ci` is CI-only** — guarded on `${CI:-}` because it deletes `node_modules` and a gate script must not be destructive to a working tree.
- **`check-licenses-generated.sh` belongs to the native suite, never the JS suite** — it needs a Rust toolchain and a populated cargo registry.
- **`gamedata/**` goes in the `ci-js` path filter and NOT `ci-native`'s.** Nothing compiles gamedata in; the shim reads it from disk at runtime. This encodes the CLAUDE.md invariant "layout is data, semantics are code; a field-offset change must never require a code change." See spec §4.2.
- **Commit after every task.** Conventional-commit subject lines. Every commit message ends with the trailer `Claude-Session: https://claude.ai/code/session_013Qp5aRck5qBUFU1MdT54Tz`.

---

## File Structure

| File | Task | Responsibility |
|---|---|---|
| `scripts/check-core-names.sh` | 1 | **Create.** CS2 name-leak + `games/`-embed greps over `core/src`. No cargo. |
| `scripts/check-core-boundary.sh` | 1 | **Modify.** Keeps the cargo dependency-closure walk; delegates the greps. |
| `scripts/test-boundary-nameleak.sh` | 1 | **Modify.** Probes against `check-core-names.sh` (5 greps, not 5 cargo walks). |
| `scripts/ci-js.sh` | 2 | **Create.** The entire JS/TS gate sequence. |
| `scripts/ci-native.sh` | 3 | **Create.** The entire native gate sequence, cheap gates first. |
| `Makefile` | 2, 3 | **Modify.** Adds `ci`, `ci-js`, `ci-native` targets. |
| `.github/workflows/ci-js.yml` | 4 | **Create.** Path-filtered, one job, calls `ci-js.sh`. |
| `.github/workflows/ci-native.yml` | 4 | **Create.** Path-filtered, one job, calls `ci-native.sh`. |
| `.github/workflows/ci.yml` | 4 | **Delete.** Replaced by the two above. |
| `.github/workflows/_build.yml` | 4 | **Delete.** Last consumer removed. |
| `.github/workflows/release.yml` | 4 | **Modify.** Drops the `build` job and `needs: build`. |
| `CLAUDE.md` | 5 | **Modify.** Graphite section replaced; gate-suite block rewritten. |
| `docs/sdk-doc-conventions.md` | 5 | **Modify.** One clause reworded. |

**Task order:** 1 → 2 → 3 → 4 → 5. Task 4 depends on 2 and 3 (the workflows call those scripts). Task 1 is independent but must precede 3 (`ci-native.sh` calls both boundary scripts).

---

### Task 1: Split the boundary script

Today `scripts/check-core-boundary.sh` does two unrelated things: a cargo dependency-closure walk (`cargo metadata` + `cargo tree` — slow) and two greps over `core/src` (instant). `test-boundary-nameleak.sh` calls the whole thing five times to plant and remove probe files, so the closure walk runs five times to test a grep. On a native PR the script executes seven times total.

**Files:**
- Create: `scripts/check-core-names.sh`
- Modify: `scripts/check-core-boundary.sh`
- Modify: `scripts/test-boundary-nameleak.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `scripts/check-core-names.sh` — exits 0 when `core/src` contains no CS2 identifier and no `include_str!`/`include_bytes!` of a `games/` path; exits 1 with a `BOUNDARY VIOLATION:` line on stderr otherwise. Task 3's `ci-native.sh` calls `check-core-boundary.sh` and `test-boundary-nameleak.sh` (not this script directly).

- [ ] **Step 1: Verify the current behaviour you must preserve**

Run:
```bash
bash scripts/check-core-boundary.sh; echo "exit=$?"
bash scripts/test-boundary-nameleak.sh; echo "exit=$?"
```

Expected: both print `PASS`/`OK` lines and `exit=0`. Record that both pass **before** you change anything — the split must not alter exit codes.

- [ ] **Step 2: Write the failing test**

`test-boundary-nameleak.sh` is already the test for this behaviour. Point it at the script that does not exist yet, so it fails for the right reason.

In `scripts/test-boundary-nameleak.sh`, replace **all five** occurrences of `bash scripts/check-core-boundary.sh` with `bash scripts/check-core-names.sh`. Also update the header comment:

```bash
#!/usr/bin/env bash
# Verifies the name-leak gate FAILS when a CS2 identifier is present in core/ and PASSES when clean.
# Also verifies the include_str!/games/ gate fires on a planted violation and passes when clean.
#
# Probes against check-core-names.sh (pure greps) rather than check-core-boundary.sh, so five
# probe/clean cycles cost five greps instead of five cargo dependency walks.
set -uo pipefail
cd "$(dirname "$0")/.."
```

Leave the five probe/clean cycles, their assertions, and the two final `PASS:` echoes exactly as they are.

- [ ] **Step 3: Run the test to verify it fails**

Run: `bash scripts/test-boundary-nameleak.sh; echo "exit=$?"`

Expected: FAIL — `exit=1` with `FAIL: gate rejected a clean core/`. The first assertion runs `bash scripts/check-core-names.sh`, which does not exist, so bash exits non-zero and the "clean tree must pass" check trips.

- [ ] **Step 4: Create `scripts/check-core-names.sh`**

Move the two grep gates verbatim out of `check-core-boundary.sh`:

```bash
#!/usr/bin/env bash
# CS2 name-leak + games/-embed gates over core/. Pure greps, no cargo — split out of
# check-core-boundary.sh so test-boundary-nameleak.sh can re-run them five times
# without repeating the cargo dependency walk. check-core-boundary.sh calls this.
set -euo pipefail
cd "$(dirname "$0")/.."

# --- CS2 name-leak gate: core/ must contain no CS2 identifier (engine-generic only). ---
# Patterns are CS2 schema/game identifiers that must live only in games/cs2 (JS) or gamedata.
NAME_LEAK_RE='CCSPlayer|CCSPlayerPawn|CCSPlayerController|m_iHealth|m_hPlayerPawn|ProcessUsercmds|CSGOUserCmdPB|CBaseUserCmdPB|subtick_moves|buttons_pb'
if grep -rInE "$NAME_LEAK_RE" core/src 2>/dev/null; then
  echo "BOUNDARY VIOLATION: CS2 identifier found in core/ (must live in games/cs2 or gamedata)" >&2
  exit 1
fi

# --- include_str!/include_bytes! gate: core/ must never embed a file from games/ at compile time. ---
# This closes the gap the Slice-4 regression exploited (core include_str!-ing games/cs2/js/pawn.js).
if grep -rInE 'include_(str|bytes)!\s*\(\s*"[^"]*games/' core/src 2>/dev/null; then
  echo "BOUNDARY VIOLATION: core/ embeds a games/ file via include_str!/include_bytes! (core must stay engine-generic)" >&2
  exit 1
fi

echo "core name gates OK: no CS2 identifier and no games/ embed in core/"
```

Then make it executable:

```bash
chmod +x scripts/check-core-names.sh
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `bash scripts/test-boundary-nameleak.sh; echo "exit=$?"`

Expected: PASS — `exit=0`, ending with:
```
PASS: name-leak gate catches CS2 identifiers and passes when clean
PASS: include_str!/games/ gate catches embedded game files and passes when clean
```

- [ ] **Step 6: Rewire `check-core-boundary.sh` to delegate**

In `scripts/check-core-boundary.sh`, delete both grep blocks (everything from the `# --- CS2 name-leak gate:` comment through the closing `fi` of the `include_(str|bytes)!` block) and put a call in their place. The file's tail becomes:

```bash
if [ "$violation" -ne 0 ]; then exit 1; fi

# Name-leak + games/-embed gates. They live in their own script (pure greps, no cargo) so
# test-boundary-nameleak.sh can re-run them without repeating the dependency walk above.
bash "$(dirname "$0")/check-core-names.sh"

echo "core boundary OK: s2script-core depends on no games/* crate"
```

Everything above `if [ "$violation" -ne 0 ]` — the shebang, `set -euo pipefail`, the `cd`, the `GAME_PKGS` mapfile, the `DEPS` cargo tree, and the violation loop — is unchanged.

- [ ] **Step 7: Verify both halves still gate**

Run all four checks:

```bash
# 1. Clean tree passes, and prints both lines.
bash scripts/check-core-boundary.sh; echo "exit=$?"

# 2. A planted CS2 name still fails THROUGH check-core-boundary.sh.
echo '// CCSPlayerPawn m_iHealth' > core/src/__probe.rs
bash scripts/check-core-boundary.sh; echo "exit=$? (want 1)"
rm -f core/src/__probe.rs

# 3. A planted games/ include_str! still fails through it.
echo 'const _: &str = include_str!("../../games/cs2/js/pawn.js");' > core/src/__probe2.rs
bash scripts/check-core-boundary.sh; echo "exit=$? (want 1)"
rm -f core/src/__probe2.rs

# 4. Clean again.
bash scripts/check-core-boundary.sh; echo "exit=$?"
make check-boundary
```

Expected: check 1 prints `core name gates OK: …` then `core boundary OK: …` with `exit=0`; checks 2 and 3 print a `BOUNDARY VIOLATION:` line with `exit=1`; check 4 is `exit=0` and `make check-boundary` succeeds.

Confirm `core/src/__probe.rs` and `core/src/__probe2.rs` are both gone: `ls core/src/__probe*` must report no such file.

- [ ] **Step 8: Commit**

```bash
git add scripts/check-core-names.sh scripts/check-core-boundary.sh scripts/test-boundary-nameleak.sh
git commit -m "$(cat <<'EOF'
refactor(gates): split the name-leak greps out of check-core-boundary.sh

check-core-boundary.sh did a cargo dependency-closure walk AND two greps over
core/src. test-boundary-nameleak.sh called the whole thing five times to plant
and remove probes, so the closure walk ran five times to test a grep — seven
cargo walks per native PR in total.

The greps move to scripts/check-core-names.sh (no cargo). check-core-boundary.sh
keeps the closure walk and calls it, so `make check-boundary` and every other
call site behave identically. nameleak now probes the grep script directly.

Cargo dependency walks per native PR: 7 -> 1.

Claude-Session: https://claude.ai/code/session_013Qp5aRck5qBUFU1MdT54Tz
EOF
)"
```

---

### Task 2: `scripts/ci-js.sh` + `make ci-js`

The JS gate sequence currently lives inline in `ci.yml`'s `gates-node` job, plus a separate `deps` job, plus a hand-maintained list in CLAUDE.md — three copies. It also omits three test scripts that exist and run nowhere.

**Files:**
- Create: `scripts/ci-js.sh`
- Modify: `Makefile`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `scripts/ci-js.sh` — exits 0 iff every JS gate passes. Task 4's `ci-js.yml` invokes it as `bash scripts/ci-js.sh` with `CI=true` set by GitHub.

- [ ] **Step 1: Confirm the three orphaned scripts currently pass**

Run:
```bash
bash scripts/check-activity-test.sh; echo "exit=$?"
bash scripts/check-antiflood-test.sh; echo "exit=$?"
bash scripts/test-gate.sh; echo "exit=$?"
```

Expected: all three `exit=0`. These have never been in CI, so confirm they are green *before* wiring them in — if one is red, that is a real pre-existing failure and you must report it rather than silently dropping the script from the suite.

- [ ] **Step 2: Create `scripts/ci-js.sh`**

```bash
#!/usr/bin/env bash
# THE JS/TS gate suite. Single source of truth: .github/workflows/ci-js.yml runs exactly
# this script and nothing else, and so does `make ci-js`. If a gate is not in here, it
# does not run — do not add a gate step to the workflow YAML.
set -euo pipefail
cd "$(dirname "$0")/.."

# The package-lock.json drift guard: the same `npm ci` the changesets release pipeline
# runs on main, so drift fails on the PR instead of after merge. CI-only, because npm ci
# deletes node_modules and a gate script must not be destructive to a working tree.
if [ -n "${CI:-}" ]; then
  echo "== npm ci (package-lock.json in sync) =="
  npm ci
else
  echo "== npm ci SKIPPED (local run — use 'CI=1 make ci-js' to run the lockfile guard) =="
fi

# Codegen freshness. Globbed so a future check-*-generated.sh starts running here with no
# edit. check-licenses-generated.sh is excluded: it needs a Rust toolchain and a populated
# cargo registry, so it lives in ci-native.sh.
for f in scripts/check-*-generated.sh; do
  case "$f" in */check-licenses-generated.sh) continue ;; esac
  echo "== $f =="
  bash "$f"
done

echo "== check-plugins-typecheck.sh (the 5E.1 gate) =="
bash scripts/check-plugins-typecheck.sh

echo "== check-activity-test.sh =="
bash scripts/check-activity-test.sh

echo "== check-antiflood-test.sh =="
bash scripts/check-antiflood-test.sh

echo "== test-gate.sh =="
bash scripts/test-gate.sh

echo "ci-js: all JS gates passed"
```

```bash
chmod +x scripts/ci-js.sh
```

- [ ] **Step 3: Add the `ci-js` make target**

In `Makefile`, extend the `.PHONY` line and append the target. The `.PHONY` line becomes:

```makefile
.PHONY: all core shim package check-boundary docker-test clean ci ci-native ci-js
```

And append at the end of the file:

```makefile
# The gate suite. These two scripts are exactly what CI runs — local green means CI green.
# npm ci is skipped on a local run; use `CI=1 make ci-js` to include the lockfile guard.
ci: ci-native ci-js

ci-js:
	./scripts/ci-js.sh
```

`ci-native` is added in Task 3. `make ci` will fail until then — that is expected and is why `ci-native` comes next.

- [ ] **Step 4: Run it and verify it passes**

Run: `make ci-js`

Expected: PASS — `npm ci SKIPPED`, then a `== scripts/check-*-generated.sh ==` block per freshness script (schema, nav, events, csitem — **not** licenses), then the typecheck gate, the three test scripts, and finally `ci-js: all JS gates passed`. Exit 0.

Verify licenses was really excluded: `make ci-js 2>&1 | grep -c licenses` must print `0`.

- [ ] **Step 5: Verify the lockfile guard actually runs under CI**

Run: `CI=1 make ci-js`

Expected: PASS, and this time the first block is `== npm ci (package-lock.json in sync) ==` followed by npm's install output. This is the only path that exercises `npm ci`, so it must be confirmed once by hand.

- [ ] **Step 6: Commit**

```bash
git add scripts/ci-js.sh Makefile
git commit -m "$(cat <<'EOF'
ci: scripts/ci-js.sh is the one JS gate suite

The JS gates lived in three hand-maintained copies (ci.yml's gates-node job, a
separate deps job, and a list in CLAUDE.md), which is how check-activity-test.sh,
check-antiflood-test.sh and test-gate.sh ended up running nowhere at all. All
three are wired in here.

npm ci — the package-lock.json drift guard, and the whole reason the `deps` job
existed — is a step in this script, guarded on $CI so a local run does not delete
node_modules.

Claude-Session: https://claude.ai/code/session_013Qp5aRck5qBUFU1MdT54Tz
EOF
)"
```

---

### Task 3: `scripts/ci-native.sh` + `make ci-native`

Cheap gates run first so a boundary violation fails in seconds rather than after a build. `cargo fetch --locked` leads because `check-licenses-generated.sh` reads every locked crate's license text out of the cargo registry, and it warms the registry for the build that follows.

**Files:**
- Create: `scripts/ci-native.sh`
- Modify: `Makefile`

**Interfaces:**
- Consumes: `scripts/check-core-boundary.sh` and `scripts/test-boundary-nameleak.sh` from Task 1.
- Produces: `scripts/ci-native.sh` — exits 0 iff every native gate passes. Task 4's `ci-native.yml` invokes it as `bash scripts/ci-native.sh`.

- [ ] **Step 1: Confirm the orphaned sigscan test currently passes**

Run: `bash scripts/test-sigscan.sh; echo "exit=$?"`

Expected: `exit=0`. It compiles `shim/src/sigscan.{h,cpp}` with the host `g++` — self-contained, no SDK, no container. As in Task 2, confirm green before wiring it in.

- [ ] **Step 2: Create `scripts/ci-native.sh`**

```bash
#!/usr/bin/env bash
# THE native (Rust + C++) gate suite. Single source of truth: .github/workflows/ci-native.yml
# runs exactly this script and nothing else, and so does `make ci-native`. If a gate is not
# in here, it does not run — do not add a gate step to the workflow YAML.
#
# Cheap gates first: a boundary violation should fail in seconds, not after a build.
set -euo pipefail
cd "$(dirname "$0")/.."

# Populates the cargo registry that check-licenses-generated.sh reads every locked crate's
# license text out of, and warms it for the build below.
echo "== cargo fetch --locked =="
cargo fetch --locked

echo "== check-core-boundary.sh (dependency closure + name gates) =="
bash scripts/check-core-boundary.sh

echo "== test-boundary-nameleak.sh =="
bash scripts/test-boundary-nameleak.sh

echo "== test-sigscan.sh =="
bash scripts/test-sigscan.sh

echo "== check-licenses-generated.sh =="
bash scripts/check-licenses-generated.sh

echo "== cargo build =="
cargo build

echo "== cargo test -p s2script-core =="
cargo test -p s2script-core

# ccache is present in CI via hendrikmuhs/ccache-action; on a dev box it may not be.
# Only pass the launcher when it actually exists, so cmake does not fail on a missing binary.
LAUNCHER=()
if command -v ccache >/dev/null 2>&1; then
  LAUNCHER=(-DCMAKE_CXX_COMPILER_LAUNCHER=ccache)
fi

echo "== shim build =="
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release \
  -DS2_CORE_LIB_DIR=debug \
  ${LAUNCHER[@]+"${LAUNCHER[@]}"}
cmake --build build/shim -j

echo "ci-native: all native gates passed"
```

```bash
chmod +x scripts/ci-native.sh
```

Two details that are easy to get wrong:

- `-DS2_CORE_LIB_DIR=debug` — the shim links `libs2script_core.so` out of `target/<dir>`, and this script runs `cargo build` (debug), not `--release`. `release.yml`'s sniper build is the only thing that produces release binaries. Passing `release` here makes the shim link a library that does not exist.
- `${LAUNCHER[@]+"${LAUNCHER[@]}"}` — not the bare `"${LAUNCHER[@]}"`. Under `set -u`, expanding an empty array is an unbound-variable error on bash < 4.4; this form expands to nothing when the array is empty.

- [ ] **Step 3: Add the `ci-native` make target**

Append to `Makefile`, below the `ci-js` target added in Task 2:

```makefile
ci-native:
	./scripts/ci-native.sh
```

The `.PHONY` line already lists `ci-native` from Task 2.

- [ ] **Step 4: Run it and verify it passes**

Run: `make ci-native`

Expected: PASS — the `==` banners in order (cargo fetch, boundary, nameleak, sigscan, licenses, cargo build, cargo test, shim build), ending with `ci-native: all native gates passed`. Exit 0.

This is the slow one: a cold `target/` means a full build of 197 transitive dependencies plus the ~130 MB prebuilt `librusty_v8.a` download. On a warm tree it is roughly a minute.

- [ ] **Step 5: Verify the full suite runs**

Run: `make ci`

Expected: PASS — `ci-native` then `ci-js`, ending with `ci-js: all JS gates passed`. Exit 0. This is the command CLAUDE.md will tell you to run before every PR, so it must work end to end before Task 5 documents it.

- [ ] **Step 6: Verify no gate was lost in the move**

Every gate that `ci.yml` runs today must appear in one of the two new scripts. Check by hand:

```bash
grep -oE 'scripts/[a-z0-9-]+\.(sh|mjs)|cargo (build|test)[^ ]*' .github/workflows/ci.yml .github/workflows/_build.yml | sort -u
grep -oE 'scripts/[a-z0-9-]+\.sh|cargo (build|test)' scripts/ci-native.sh scripts/ci-js.sh | sort -u
```

Expected: every entry in the first list appears in the second, except `scripts/check-core-boundary.sh` appearing once rather than twice (Task 1 removed the duplicate) and the glob `check-*-generated.sh` expanding by name. If anything else is only in the first list, it was dropped — add it before continuing.

- [ ] **Step 7: Commit**

```bash
git add scripts/ci-native.sh Makefile
git commit -m "$(cat <<'EOF'
ci: scripts/ci-native.sh is the one native gate suite

Cheap gates first (boundary, nameleak, sigscan, licenses) so a violation fails in
seconds instead of after a build. Folds in gates-rust (which spun up a whole Rust
toolchain to run one script the build already ran) and gates-licenses, and wires
in test-sigscan.sh, which existed but ran nowhere.

test-boundary-nameleak.sh moves here from the JS job, where it only ever passed
because ubuntu-latest happens to ship a Rust toolchain.

`make ci` now runs both suites — the same scripts CI runs, so local green means
CI green.

Claude-Session: https://claude.ai/code/session_013Qp5aRck5qBUFU1MdT54Tz
EOF
)"
```

---

### Task 4: Replace the workflows

Seven check runs become at most two. This task is atomic: `ci.yml` cannot be deleted before its replacements exist, and `_build.yml` cannot be deleted before `release.yml` stops calling it.

**Files:**
- Create: `.github/workflows/ci-native.yml`
- Create: `.github/workflows/ci-js.yml`
- Delete: `.github/workflows/ci.yml`
- Delete: `.github/workflows/_build.yml`
- Modify: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `scripts/ci-native.sh` (Task 3), `scripts/ci-js.sh` (Task 2).
- Produces: two workflows named `ci-native` and `ci-js`. Nothing downstream consumes them.

- [ ] **Step 1: Create `.github/workflows/ci-native.yml`**

```yaml
name: ci-native

# The native (Rust + C++) gate. Path-filtered at the WORKFLOW level, so filtering costs
# zero jobs — the old `changes` (dorny/paths-filter) and `ci-ok` (fail-closed aggregator)
# jobs are gone. See docs/superpowers/specs/2026-07-22-ci-consolidation-design.md.
#
# NOTE: gamedata/** is deliberately NOT here — it belongs to ci-js. Nothing compiles it in;
# the shim reads gamedata/core.gamedata.jsonc from disk at runtime. That is the CLAUDE.md
# invariant "layout is data, semantics are code" expressed as a path filter.
on:
  pull_request:
    paths:
      - 'core/**'
      - 'shim/**'
      - 'games/**'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'third_party/**'
      - 'licenses/**'
      - 'LICENSE'
      - 'packages/*/LICENSE-*'
      - 'scripts/ci-native.sh'
      - 'scripts/gen-licenses.sh'
      - 'scripts/check-licenses-generated.sh'
      - 'scripts/check-core-boundary.sh'
      - 'scripts/check-core-names.sh'
      - 'scripts/test-boundary-nameleak.sh'
      - 'scripts/test-sigscan.sh'
      - '.github/workflows/ci-native.yml'
  push:
    branches: [main]
    paths:
      - 'core/**'
      - 'shim/**'
      - 'games/**'
      - 'Cargo.toml'
      - 'Cargo.lock'
      - 'third_party/**'
      - 'licenses/**'
      - 'LICENSE'
      - 'packages/*/LICENSE-*'
      - 'scripts/ci-native.sh'
      - 'scripts/gen-licenses.sh'
      - 'scripts/check-licenses-generated.sh'
      - 'scripts/check-core-boundary.sh'
      - 'scripts/check-core-names.sh'
      - 'scripts/test-boundary-nameleak.sh'
      - 'scripts/test-sigscan.sh'
      - '.github/workflows/ci-native.yml'

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

# Only contents:read. The old pull-requests:read existed solely for dorny/paths-filter's
# "list PR files" API call, and paths-filter is gone.
permissions:
  contents: read

jobs:
  native:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with: { submodules: recursive }   # gen-licenses.sh reads the vendored third_party/ submodules
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          # Restore-only on PRs. Returns the ~11s save step, and stops every branch minting
          # its own ~383MB entry — the predecessor repo reached 77 entries / 4.34GB against a
          # 10GB quota, and LRU eviction of a rust-cache entry is exactly what turns a 13s
          # cargo build into a 3-4 minute one. Only main populates the cache.
          # Accepted cost: a PR that changes Cargo.lock misses and rebuilds fully on every
          # push until it merges (9 such PRs in the last 60 days).
          save-if: ${{ github.ref == 'refs/heads/main' }}
      - uses: hendrikmuhs/ccache-action@v1
      - name: native gate suite
        run: bash scripts/ci-native.sh
```

- [ ] **Step 2: Create `.github/workflows/ci-js.yml`**

```yaml
name: ci-js

# The JS/TS gate. Path-filtered at the WORKFLOW level — see ci-native.yml.
#
# docker/** is here because test-gate.sh asserts on docker/pre.sh, docker/docker-compose.yml
# and docker/docker-compose.gate.yml. scripts/** is deliberately broad and overlaps
# ci-native.yml's filter; overlap just means both run, which is correct.
on:
  pull_request:
    paths:
      - 'plugins/**'
      - 'packages/**'
      - 'examples/**'
      - 'games/**'
      - 'gamedata/**'
      - 'scripts/**'
      - 'docker/**'
      - 'package.json'
      - 'package-lock.json'
      - 'tsconfig.base.json'
      - '.github/workflows/ci-js.yml'
  push:
    branches: [main]
    paths:
      - 'plugins/**'
      - 'packages/**'
      - 'examples/**'
      - 'games/**'
      - 'gamedata/**'
      - 'scripts/**'
      - 'docker/**'
      - 'package.json'
      - 'package-lock.json'
      - 'tsconfig.base.json'
      - '.github/workflows/ci-js.yml'

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

permissions:
  contents: read

jobs:
  js:
    runs-on: ubuntu-latest
    steps:
      # No submodules: nothing in the JS gate reads third_party/, so the checkout stays cheap.
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          # Must match changesets.yml so the two can never disagree about `npm ci`.
          node-version: "22"
          cache: npm
      - name: JS gate suite
        run: bash scripts/ci-js.sh
```

- [ ] **Step 3: Delete the old workflows and rewire `release.yml`**

```bash
git rm .github/workflows/ci.yml .github/workflows/_build.yml
```

Then in `.github/workflows/release.yml`, delete the entire `build` job — these lines:

```yaml
  # Fail-fast host-side release-profile sanity check (shared with ci.yml's cheap
  # debug build) — catches a broken core/shim build in ~minutes, before spinning
  # up the much heavier bullseye Docker sniper build below.
  build:
    uses: ./.github/workflows/_build.yml
    with:
      profile: release
      core_lib_dir: release

  release:
    needs: build
    runs-on: ubuntu-latest
```

and replace them with:

```yaml
  # The sniper build IS the test. The old fail-fast `build` job compiled release-profile on
  # ubuntu-latest — a configuration that is neither PR CI (debug/ubuntu) nor the shipped
  # artifact (release/bullseye/glibc 2.31) — to save ~4 minutes on a tag push where nobody
  # is iterating. Removing it also removed _build.yml's last consumer.
  release:
    runs-on: ubuntu-latest
```

Everything else in `release.yml` — the `on: push: tags: v*` trigger, `permissions: contents: write`, and every step of the `release` job — is unchanged.

- [ ] **Step 4: Verify the YAML parses and says what it should**

Save this as `/tmp/claude-1000/-home-gkh-projects-s2script/faa38fc1-d9a7-42e7-a226-d9cf26798413/scratchpad/verify-workflows.py` and run it with `python3`:

```python
import sys, yaml, pathlib

wf = pathlib.Path(".github/workflows")
fail = []

# Every workflow parses.
files = sorted(p.name for p in wf.glob("*.yml"))
parsed = {}
for p in wf.glob("*.yml"):
    try:
        parsed[p.name] = yaml.safe_load(p.read_text())
    except Exception as e:
        fail.append(f"{p.name}: does not parse: {e}")

# The old workflows are gone, the new ones exist.
assert_files = {"ci-native.yml", "ci-js.yml", "changesets.yml", "release.yml"}
if set(files) != assert_files:
    fail.append(f"workflow set is {files}, want {sorted(assert_files)}")

for name in ("ci-native.yml", "ci-js.yml"):
    d = parsed.get(name)
    if not d:
        continue
    jobs = d.get("jobs", {})
    if len(jobs) != 1:
        fail.append(f"{name}: has {len(jobs)} jobs, want exactly 1 (that is the whole point)")
    # PyYAML parses the bare key `on` as boolean True.
    trig = d.get("on", d.get(True, {}))
    for ev in ("pull_request", "push"):
        if not trig.get(ev, {}).get("paths"):
            fail.append(f"{name}: {ev} has no paths filter")
    # The job must call exactly one gate script and run no gates inline.
    steps = list(jobs.values())[0]["steps"]
    runs = [s["run"] for s in steps if "run" in s]
    want = "scripts/ci-native.sh" if name == "ci-native.yml" else "scripts/ci-js.sh"
    if runs != [f"bash {want}"]:
        fail.append(f"{name}: run steps are {runs}, want exactly ['bash {want}']")

# gamedata belongs to ci-js and NOT ci-native (spec section 4.2).
native = parsed.get("ci-native.yml", {})
nat_paths = native.get("on", native.get(True, {})).get("pull_request", {}).get("paths", [])
if any("gamedata" in p for p in nat_paths):
    fail.append("ci-native.yml: gamedata/** must NOT be in its filter — nothing compiles it in")
js = parsed.get("ci-js.yml", {})
js_paths = js.get("on", js.get(True, {})).get("pull_request", {}).get("paths", [])
if not any("gamedata" in p for p in js_paths):
    fail.append("ci-js.yml: gamedata/** must be in its filter — the codegen checks read it")

# release.yml has exactly one job and no reference to the deleted reusable workflow.
rel = parsed.get("release.yml", {})
if list(rel.get("jobs", {})) != ["release"]:
    fail.append(f"release.yml: jobs are {list(rel.get('jobs', {}))}, want ['release']")
if "_build.yml" in (wf / "release.yml").read_text():
    fail.append("release.yml: still references _build.yml")

if fail:
    print("FAIL")
    for f in fail:
        print("  -", f)
    sys.exit(1)
print("PASS: workflow topology is correct")
```

Run: `python3 /tmp/claude-1000/-home-gkh-projects-s2script/faa38fc1-d9a7-42e7-a226-d9cf26798413/scratchpad/verify-workflows.py`

Expected: `PASS: workflow topology is correct`, exit 0.

- [ ] **Step 5: Verify the path filters classify real PRs correctly**

The filters are the whole mechanism, so check them against the four cases in spec §4.3 by hand:

```bash
python3 - <<'PY'
import yaml, pathlib, fnmatch
def paths(name):
    d = yaml.safe_load(pathlib.Path(f".github/workflows/{name}").read_text())
    return d.get("on", d.get(True, {}))["pull_request"]["paths"]
nat, js = paths("ci-native.yml"), paths("ci-js.yml")
def hits(f, pats):
    return any(fnmatch.fnmatch(f, p) or f.startswith(p.replace("**", "")) for p in pats)
for f, want in [
    ("README.md",                        set()),
    ("docs/ARCHITECTURE.md",             set()),
    ("plugins/basechat/src/plugin.ts",   {"js"}),
    ("packages/sdk/src/index.ts",        {"js"}),
    ("core/src/v8host.rs",               {"native"}),
    ("shim/src/sigscan.cpp",             {"native"}),
    ("Cargo.lock",                       {"native"}),
    ("gamedata/core.gamedata.jsonc",     {"js"}),
    ("games/cs2/js/pawn.js",             {"js", "native"}),
    ("scripts/ci-js.sh",                 {"js"}),
]:
    got = ({"native"} if hits(f, nat) else set()) | ({"js"} if hits(f, js) else set())
    print(("ok  " if got == want else "FAIL"), f, "->", sorted(got) or "(no CI)", "want", sorted(want) or "(no CI)")
PY
```

Expected: every line starts `ok`. A docs-only change triggers nothing; a plugin change triggers `ci-js` alone; a `core/` change triggers `ci-native` alone; `games/**` triggers both; `gamedata/**` triggers only `ci-js`.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/
git commit -m "$(cat <<'EOF'
ci: two path-filtered workflows replace the seven-job ci.yml

Per-PR check runs: 7 -> at most 2. Docs-only PRs now show zero checks, a JS or a
core PR shows one, and only a games/ or scripts/ change shows two.

Deleted: ci.yml, _build.yml, and the jobs changes, ci-ok, deps, gates-rust,
gates-licenses. `changes` and `ci-ok` existed to make path filtering safe under a
required-status-check flip that cannot happen — branch protection and rulesets
both 403 on a private free-plan repo. `deps` existed only because package.json
and package-lock.json were in no path filter; they are now in ci-js's.

Filtering moves to the workflow level, so it costs zero jobs. Each workflow runs
exactly one script (scripts/ci-native.sh, scripts/ci-js.sh) and no inline gates.

rust-cache is restore-only on PRs (save-if main): returns ~11s per PR and stops
every branch minting its own ~383MB entry against a 10GB quota.

release.yml drops its fail-fast host build, which compiled a configuration that
was neither PR CI nor the shipped artifact.

Claude-Session: https://claude.ai/code/session_013Qp5aRck5qBUFU1MdT54Tz
EOF
)"
```

---

### Task 5: Retire the Graphite doctrine

**Files:**
- Modify: `CLAUDE.md` (the `## Commands` gate-suite block, and the whole `## Ship work as a stack, not a branch (Graphite)` section)
- Modify: `docs/sdk-doc-conventions.md:5`

**Interfaces:**
- Consumes: `make ci` / `make ci-native` / `make ci-js` from Tasks 2 and 3.
- Produces: nothing programmatic.

- [ ] **Step 1: Replace the gate-suite block in `## Commands`**

In `CLAUDE.md`, replace this block (currently lines 31–41):

````markdown
**Gate suite (run before every PR):**
```bash
make check-boundary                     # core must NOT import games/* (== scripts/check-core-boundary.sh)
./scripts/check-plugins-typecheck.sh    # every plugin + example typechecks vs the shipped .d.ts (the 5E.1 gate)
./scripts/check-schema-generated.sh     # codegen freshness — regenerate + `git diff --exit-code`
./scripts/check-nav-generated.sh
./scripts/check-events-generated.sh
./scripts/check-csitem-generated.sh
./scripts/check-licenses-generated.sh    # third-party notices vs a fresh gen-licenses.sh run
./scripts/test-boundary-nameleak.sh
```
````

with:

````markdown
**Gate suite (run before every PR) — these ARE the CI jobs:**
```bash
make ci           # both suites
make ci-native    # scripts/ci-native.sh — boundary + nameleak + sigscan + licenses, cargo build/test, shim
make ci-js        # scripts/ci-js.sh — codegen freshness, plugin typecheck, activity/antiflood/gate tests
```
`.github/workflows/ci-native.yml` and `ci-js.yml` each run one of those two scripts and nothing
else, so **local green means CI green** and a new gate is added to the script, never to the YAML.
`npm ci` (the `package-lock.json` drift guard) is CI-only — run `CI=1 make ci-js` to include it.
`make check-boundary` still runs the core→games boundary check on its own.
````

- [ ] **Step 2: Replace the Graphite section**

Delete the entire `## Ship work as a stack, not a branch (Graphite)` section (currently lines 70–96, everything from that heading up to but not including `## Repository layout`) and put this in its place:

```markdown
## Ship one PR per slice

**A slice is one branch and one PR.** Plain `git` + `gh pr create`, squash-merged. The PR is as
big as the slice is — don't split a slice into a chain of dependent PRs, and don't batch two
slices into one. Graphite and stacked PRs are retired; there is no `gt`.

Branch naming: `<area>/<terse-change>` — e.g. `ci/consolidation`, `docs/readme-front-door`.

A PR must be **atomic**: it passes `make ci` and is safe to merge on its own. A signature change
that breaks every caller lands WITH its callers.

A pre-merge gate is not optional even for a one-line change: a push to `main` auto-fires
`changesets.yml`, which publishes to npm. There must be a gate between a bad commit and the registry.

PR bodies need **Why** — what prompted this, and how it fits. Write the body with the Write tool to
a file and `gh pr edit N --body-file`; never a heredoc, because shell escaping mangles tables and
code blocks.
```

- [ ] **Step 3: Fix the self-contradicting sentence in the doc conventions**

In `docs/sdk-doc-conventions.md`, replace lines 5–6:

```
consistent. Coverage is enforced per-PR by `scripts/check-doc-coverage.mjs`
(a dev tool, not a CI gate).
```

with:

```
consistent. Check coverage as you write stubs with `scripts/check-doc-coverage.mjs`
— a local dev tool, deliberately not a CI gate.
```

The old sentence contradicted itself: "enforced per-PR" describes a gate and the parenthetical denies it. The plan for that slice (`docs/superpowers/plans/2026-07-21-sdk-tsdoc-intellisense.md:17`) is authoritative — "Do NOT add `check-doc-coverage` to `.github/` or the gate suite."

- [ ] **Step 4: Verify the doctrine is really gone and the new commands are real**

```bash
# No Graphite left in the governing doc (historical plans/specs are records — leave them).
grep -nE 'gt submit|gt create|gt restack|gt modify|gt track|Graphite|stacked PR' CLAUDE.md \
  && { echo "FAIL: Graphite still referenced in CLAUDE.md"; exit 1; } || echo "ok: no Graphite in CLAUDE.md"

# Every make target CLAUDE.md now advertises actually exists.
for t in ci ci-native ci-js check-boundary; do
  grep -qE "^$t:" Makefile && echo "ok: make $t exists" || echo "FAIL: make $t missing"
done

# No stale reference to a deleted workflow anywhere outside the historical docs.
grep -rn '_build.yml\|workflows/ci.yml' --include='*.md' --include='*.yml' . \
  | grep -v 'docs/superpowers/' | grep -v node_modules \
  && { echo "FAIL: stale workflow reference"; exit 1; } || echo "ok: no stale workflow references"

# The doc conventions no longer claim enforcement.
grep -n 'enforced per-PR' docs/sdk-doc-conventions.md \
  && { echo "FAIL: still claims enforcement"; exit 1; } || echo "ok: doc conventions corrected"
```

Expected: five `ok:` lines, no `FAIL`.

- [ ] **Step 5: Run the full suite one last time**

Run: `make ci`

Expected: PASS, exit 0. This is the last chance to catch a gate that the doc now promises but that does not actually run.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/sdk-doc-conventions.md
git commit -m "$(cat <<'EOF'
docs: retire the Graphite stacked-PR doctrine; one PR per slice

Replaces the stacking section with plain git + gh pr create, squash-merged, one
branch and one PR per slice. Drops the "always argue for more PRs, never fewer"
mandate, the per-PR-in-a-stack gate rule, the gt command reference, and the
terse-stack-name/terse-change branch convention. The gh pr edit --body-file rule
survives — it was never Graphite-specific.

The gate-suite block now points at `make ci`, which runs the same two scripts the
workflows run, instead of a hand-maintained list of eight commands that had
already drifted from what CI actually enforced.

Also fixes a sentence in sdk-doc-conventions.md that contradicted itself inside
one clause ("enforced per-PR ... (a dev tool, not a CI gate)").

Claude-Session: https://claude.ai/code/session_013Qp5aRck5qBUFU1MdT54Tz
EOF
)"
```

---

## Verification against the spec's success criteria

Spec §9, checked after Task 5:

| criterion | how |
|---|---|
| docs-only PR → 0 checks; plugin PR → 1; `core/` PR → 1 | Task 4 Step 5 |
| workflows contain no gate step outside the two scripts | Task 4 Step 4 (asserts `run` steps are exactly one `bash scripts/ci-*.sh`) |
| `grep -rn 'gt submit\|gt create\|gt restack' CLAUDE.md` returns nothing | Task 5 Step 4 |
| each formerly-orphaned script appears in exactly one gate script | Task 2 Step 4, Task 3 Steps 4 and 6 |
| `make check-boundary` keeps its exit-code behaviour on both halves | Task 1 Step 7 |
| a planted CS2 identifier still fails `ci-native` | Task 1 Step 7 (checks 2 and 3) |

## Opening the PR

After Task 5, this slice is one PR — the doctrine it installs:

Write the body with the Write tool to `/tmp/claude-1000/-home-gkh-projects-s2script/faa38fc1-d9a7-42e7-a226-d9cf26798413/scratchpad/pr-body.md` — never a heredoc, because shell escaping mangles tables and code blocks. Then:

```bash
git push -u origin ci/consolidation
gh pr create --title "ci: 7 check runs per PR down to at most 2" \
  --body-file /tmp/claude-1000/-home-gkh-projects-s2script/faa38fc1-d9a7-42e7-a226-d9cf26798413/scratchpad/pr-body.md
```

The body needs **Why**: seven check runs per PR; `check-core-boundary.sh` executing seven times on a native PR; four test scripts that ran nowhere; and two orchestration jobs (`changes`, `ci-ok`) serving a required-status-check flip that cannot happen, because branch protection and rulesets both 403 on a private free-plan repo.

Note the PR itself is the first live test of the path filters — the diff touches `scripts/**`, `.github/workflows/**`, `CLAUDE.md`, and `docs/`, so it must show **`ci-js` only**. `ci-native` should not run: no `core/`, `shim/`, `games/`, or `Cargo.*` file changes, and the native-side gate scripts it does touch (`check-core-boundary.sh`, `check-core-names.sh`, `test-boundary-nameleak.sh`) are in `ci-native`'s filter — so in fact **both** run. Confirm that is what appears, and that it is two boxes rather than seven.
