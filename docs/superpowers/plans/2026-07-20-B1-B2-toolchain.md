# B1+B2 Toolchain — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the safety-by-construction re-architecture's toolchain slices: **B1** makes every load-time contract check exist at build time or be deleted outright (`apiVersion` derived at build, `typesSha256` verified at load, `publishes` derived from code, warn-once loader refusals), and **B2** ships `@s2script/eslint-plugin` — the four residual local rules — running as one pinned engine in the editor AND in-process inside `s2s build`.

**Architecture:** B1 hardens the two ends of the existing pipeline: the TS side (`packages/sdk/src/build.ts` + `typecheck/typecheck.ts`) stamps/derives/hashes what used to be author input, and the Rust side (`core/src/loader.rs` + `interfaces.rs` + `v8host.rs`) verifies the previously-inert `typesSha256` and remembers refusals through the existing `WATCH_STATE`/`FAILED_PLUGINS` phase machinery. B2 adds one new **tooling** package (`packages/eslint-plugin`, published as `@s2script/eslint-plugin`) whose rules are consumed two ways from one implementation: the scaffolded `eslint.config.mjs` (editor, `projectService`) and `s2s build`'s in-process run (reusing the tsc gate's already-built `ts.Program`, so build lint resolution is byte-identical to the typecheck gate).

**Tech Stack:** TypeScript (SDK CLI, node:test via `--experimental-strip-types`), Rust (core, `cargo test`), ESLint ≥9 flat config + `@typescript-eslint/{parser,utils,rule-tester}` (typed rules), esbuild (both package bundles).

**Authoritative design:** `docs/superpowers/specs/2026-07-20-safety-by-construction-north-star-design.md` §5 (+§8 locked decisions #3, #6). Typed-artifact surface consumed here is frozen by `docs/superpowers/specs/2026-07-20-L1-lifecycle-v2-design.md` (`plugin(ctx)`, `PluginContext`, `packages/sdk/plugin.d.ts`).

## Global Constraints

- **Repo root (work here):** `/home/gkh/projects/s2script/.claude/worktrees/rearch+north-star`, branch `rearch/north-star`. Commit each task directly to this branch (E1/L1 precedent on this worktree). No `gt` stack, no PR submission — the human decides integration at the end.
- **Rebuild the CLI after every `packages/sdk/src` change:** `cd packages/sdk && node build.mjs` (→ `dist/cli.js`). Anything invoking `node packages/sdk/dist/cli.js` tests the *bundle*, not `src/`.
- **Build a plugin:** `node packages/sdk/dist/cli.js build <dir>` (from repo root).
- **SDK CLI tests:** `cd packages/sdk && npm test` (runs `node --experimental-strip-types --no-warnings --test test/*.test.mjs`).
  **KNOWN-FAILING BASELINE — do NOT fix, do NOT add to:** exactly these 13 tests fail before this plan and must be the ONLY failures after every task (all in `schema-runtime.test.mjs` + `player-identity.test.mjs`):
  1. `generated Vector/QAngle accessor: reads a value object, degrades to null (offline vm)`
  2. `nav.generated.js + pawn.js compose: sceneNode/weaponServices wrappers, null-hop guard (offline vm)`
  3. `pawn.origin / pawn.angles: pointer-chain accessors read a value, degrade to null (offline vm)`
  4. `Pawn.setVelocity (offline vm): writes 3 floats + notifyStateChanged`
  5. `Player.allConnected + userId (offline vm): connected slots regardless of pawn`
  6. `Player.fromSlot degrades to null when the controller is invalid (offline vm)`
  7. `Player.fromSlot excludes a valid controller with no pawn (occupancy filter, offline vm)`
  8. `Player.fromUserId (offline vm): round-trips to the right slot, null on miss`
  9. `Player.kick (offline vm): calls __s2_client_kick with slot + reason`
  10. `Player model: fromSlot/all, generated accessors, .pawn + .controller nav (offline vm)`
  11. `Player.target name match (offline vm): exact wins over partial, partial returns all, no-match empty`
  12. `Player.target (offline vm): #userid / @all / @me / name / no-match`
  13. `schema.generated.js + pawn.js compose: Pawn.prototype has generated accessors`
- **Core tests:** `cargo test -p s2script-core` — forced single-threaded via `.cargo/config.toml`; **never pass `--test-threads`**.
- **Gate suite (run per task where marked, and in full at Task 11):**
  ```bash
  make check-boundary                     # core must NOT import games/*
  ./scripts/check-plugins-typecheck.sh    # every plugin + example typechecks vs the shipped .d.ts
  ./scripts/check-schema-generated.sh
  ./scripts/check-nav-generated.sh
  ./scripts/check-events-generated.sh
  ./scripts/check-csitem-generated.sh
  ./scripts/test-boundary-nameleak.sh
  ```
- **Base plugins build:** `./scripts/build-base-plugins.sh` (drives `s2s build` per shipped plugin — after Task 9 this also exercises the lint gate).
- **Lockfile discipline (memory: lockfile-reconcile-integrity):** `npm install` at the repo root may ADD entries for new deps; **NEVER** delete + regenerate `package-lock.json` (a from-scratch regen strips the 133 integrity hashes and cannot be recovered in this environment). After any install, `git diff package-lock.json` must show only additions/modifications for the new packages.
- **Packaging convention (locked #10):** no new *runtime* `@s2script/*` packages. `@s2script/eslint-plugin` is **dev-time tooling** (SDK-pinned), never importable by plugin runtime code, and is the ONE new package this plan creates (see Open Questions #1).
- **Core stays engine-generic** — nothing in this plan touches `games/` or the shim.
- **Naming:** PascalCase types (`ImportSpec`, `PublishScan`), camelCase functions (`lintPlugin`, `scanPluginProgram`).
- **`HOST_API_VERSION_MAJOR` is 2** (`core/src/loader.rs:56`) — L1 bumped it; every stamp/test in this plan uses major 2.
- **The loader keeps its apiVersion major gate** (`core/src/loader.rs` `api_version_compatible`) as the runtime backstop — B1 never removes a load-side check, it only adds build-side derivation and load-side memory.

## Resolved decisions (2026-07-21 — human-reviewed; these settle the "Open questions" tail)

1. **The ESLint plugin is a separate scoped tooling package `@s2script/eslint-plugin` (`packages/eslint-plugin`)** — CONFIRMED. It is dev-time tooling (an eslint-resolvable JS module), NOT a runtime capability; locked decision #10 ("no new packages") governs the runtime surface, and tooling is a different category. Not unscoped, not an `@s2script/sdk` subpath.
2. Authored `s2script.apiVersion` → **warn-and-ignore** (not a build error) — don't break out-of-tree plugin builds for a now-derived field.
3. `publishes`-from-code → **name-set derivation only** (auto-`"self"` + exact reconciliation + literal names); versions are data, conditional `ctx.publish` is statically ambiguous → runtime `reconcile_publishes` keeps the residual.
4. `compiledAgainst` → **opt-in** via a local `.s2script/types/<iface>/index.d.ts` contract copy (a `cp`); the `s2s add <iface>` fetch is deferred to the registry slice.
5. Late producer-hash drift → **poison per-call** (`InterfaceTypesMismatch`), NOT consumer unload — matches the existing lazy hard-dep proxy model (the L1 decision).
6. ESLint version pair as Task 6 pins (`eslint ^10.7.0` + `typescript-eslint 8.65`, verified fallback `^9.39.0`), identical in the plugin, the SDK, and the `s2s create` template.
7. `no-ctx-escape` does **not** chase `ctx`-passed-as-argument — accepted residual (the runtime seal backstops it), documented in the rule.
8. `state()`-returning-BigInt is **not** covered by `no-bigint-in-interface-payloads` — a candidate 5th rule, deferred until the footgun shows in practice.
9. Task 9 gates on **errors only** (all four rules ship at `error` severity, so this is moot now).

## Parallelization map

Three independent lanes until the merge point at Task 9. **`packages/sdk/src/build.ts` is the coordination hotspot** — it is modified by Tasks 1, 4, 5, and 9, which MUST run in that order (each task's diff assumes the previous task's version of the file). `core/src/loader.rs` is modified by Tasks 2 and 3, in that order. `packages/sdk/src/create/create.ts` is modified by Tasks 1 and 10, in that order.

```
Lane TS (B1)              Lane Rust (B1)             Lane ESLint (B2)
────────────              ──────────────             ────────────────
T1 apiVersion stamp       T2 warn-once refusals      T6 pkg + no-ctx-escape
      │                         │                          │
      │                   T3 typesSha256 verify       T7 typed rules (floating-
      │                         │                        promise, raw-view)
T4 compiledAgainst  ◄── wire format "compiledAgainst"      │
   at build              is FIXED IN THIS PLAN;       T8 no-bigint rule
      │                  T3 ∥ T4 are independent           │
T5 publishes-from-code          │                          │
      └────────────┬────────────┴──────────┬───────────────┘
                   ▼                       ▼
             T9 s2s build in-process lint (needs T5's program return + T6-T8's rules)
                   │
             T10 scaffold + editor/build parity + base-suite fix pass
                   │
             T11 full gates + changesets + PROGRESS entry
```

- **Genuinely parallel:** {T1}, {T2→T3}, {T6→T7→T8} can run as three concurrent workers.
- **T4 does not depend on T3** (only on the manifest key `compiledAgainst`, whose exact shape both tasks take from this plan): `{"compiledAgainst": {"<interface-name>": "<sha256-hex-of-contract-bytes>"}}` — sha256 of the RAW bytes of the consumer's local contract copy, same hashing as `publishes.typesSha256` (`hashContract`).
- **T9 blocks on T5 AND T8.** T10 blocks on T9. T11 last.

---

## B1 — build ⊇ load

### Task 1: Derive `apiVersion` at build (the SDK stamps the host major)

**Files:**
- Create: `packages/sdk/src/api-version.ts`
- Create: `packages/sdk/test/api-version.test.mjs`
- Modify: `packages/sdk/src/build.ts` (the `apiVersion` derivation around the current `const apiVersion = s2.apiVersion ?? "";`)
- Modify: `packages/sdk/test/build.test.mjs` (the `manifest.apiVersion` assertions, currently expecting `"1.x"`)
- Modify: `packages/sdk/src/create/create.ts` (`packageJsonContent` — drop the scaffolded `s2script.apiVersion`)
- Modify: every in-repo plugin/example `package.json` that carries `s2script.apiVersion` (mechanical strip; test fixtures under `packages/sdk/test/fixtures/` are deliberately KEPT as-is)

**Interfaces:**
- Consumes: `core/src/loader.rs` `pub(crate) const HOST_API_VERSION_MAJOR: u32 = 2;` (read-only, as the drift-gate oracle).
- Produces: `export const HOST_API_VERSION_MAJOR = 2` and `export const STAMPED_API_VERSION = "2.x"` from `packages/sdk/src/api-version.ts` — Task 11's changeset notes and any future task needing the host major import from here, never re-declare.

- [ ] **Step 1: Write the failing tests**

Create `packages/sdk/test/api-version.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { HOST_API_VERSION_MAJOR, STAMPED_API_VERSION } from "../src/api-version.ts";

test("SDK host-major equals core's HOST_API_VERSION_MAJOR (drift gate)", () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const loaderRs = readFileSync(
    join(here, "..", "..", "..", "core", "src", "loader.rs"),
    "utf8",
  );
  const m = loaderRs.match(/HOST_API_VERSION_MAJOR:\s*u32\s*=\s*(\d+)/);
  assert.ok(m, "HOST_API_VERSION_MAJOR not found in core/src/loader.rs");
  assert.equal(
    Number(m[1]),
    HOST_API_VERSION_MAJOR,
    "core and SDK disagree on the host apiVersion major — bump BOTH in one commit",
  );
});

test("stamped form carries the major in loader-parseable form", () => {
  // loader parse_api_major reads the leading integer: "2.x" -> 2.
  assert.match(STAMPED_API_VERSION, /^\d+\.x$/);
  assert.equal(parseInt(STAMPED_API_VERSION, 10), HOST_API_VERSION_MAJOR);
});
```

In `packages/sdk/test/build.test.mjs`, find the existing assertions (lines ~29-30):

```js
  assert.ok(manifest.apiVersion, "manifest.apiVersion should be truthy");
  assert.equal(manifest.apiVersion, "1.x", "manifest.apiVersion should match s2script.apiVersion");
```

and replace with (add the import at the top of the file):

```js
import { STAMPED_API_VERSION } from "../src/api-version.ts";
```

```js
  assert.ok(manifest.apiVersion, "manifest.apiVersion should be truthy");
  // B1: apiVersion is DERIVED — the fixture's authored s2script.apiVersion ("1.x") is IGNORED
  // and the SDK's own host major is stamped. The drift class is deleted, not detected.
  assert.equal(manifest.apiVersion, STAMPED_API_VERSION,
    "manifest.apiVersion is stamped from the SDK host major, not copied from package.json");
```

(The `hello` fixture keeps its authored `"apiVersion": "1.x"` — it now proves the ignore+stamp path.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/api-version.test.mjs test/build.test.mjs`
Expected: FAIL — `api-version.test.mjs` cannot resolve `../src/api-version.ts` (module not found); `build.test.mjs` fails the new `STAMPED_API_VERSION` assertion (manifest still carries `"1.x"`).

- [ ] **Step 3: Create `packages/sdk/src/api-version.ts`**

```ts
/**
 * The host apiVersion major THIS SDK types — the TS-side single source of truth.
 *
 * `s2s build` STAMPS the manifest `apiVersion` from this constant (north-star §5.2, locked
 * decision #6): the field is derived, never author-input, so "green build / refused load"
 * apiVersion drift is impossible to author. The loader's major gate (core/src/loader.rs
 * `HOST_API_VERSION_MAJOR` / `api_version_compatible`) stays as the runtime backstop.
 *
 * MUST equal core/src/loader.rs `HOST_API_VERSION_MAJOR` — test/api-version.test.mjs fails
 * the suite when they drift. Bump BOTH in the same commit.
 */
export const HOST_API_VERSION_MAJOR = 2;

/** Exactly what `s2s build` writes into manifest.apiVersion ("2.x": major-pinned, minor-open). */
export const STAMPED_API_VERSION = `${HOST_API_VERSION_MAJOR}.x`;
```

- [ ] **Step 4: Stamp in `packages/sdk/src/build.ts`**

Add the import at the top (next to the `derivePublishes` import):

```ts
import { STAMPED_API_VERSION } from "./api-version.ts";
```

Replace the line

```ts
  const apiVersion = s2.apiVersion ?? "";
```

with:

```ts
  // --- apiVersion is DERIVED at build (north-star §5.2, locked decision #6). The SDK stamps the
  // host major it types; an authored s2script.apiVersion is vestigial and ignored (warn so authors
  // delete it). The loader's major gate stays as the runtime backstop for stale .s2sp files.
  if (s2.apiVersion !== undefined) {
    console.warn(
      `WARN: ${pkgPath}: s2script.apiVersion is ignored — s2s build derives apiVersion from the ` +
        `SDK (stamping ${JSON.stringify(STAMPED_API_VERSION)}). Remove the field.`,
    );
  }
  const apiVersion = STAMPED_API_VERSION;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/api-version.test.mjs test/build.test.mjs`
Expected: PASS (all tests in both files; the build test's stderr shows the new WARN for the `hello` fixture — that is the ignore path working).

- [ ] **Step 6: Drop `apiVersion` from the `s2s create` template**

In `packages/sdk/src/create/create.ts`, `packageJsonContent`, delete the `s2script` block:

```ts
        devDependencies,
        s2script: {
          apiVersion: "2.x",
        },
```

becomes:

```ts
        devDependencies,
```

(The scaffolded plugin now has NO `s2script` block; `buildPlugin` treats a missing block as `{}`.)

- [ ] **Step 7: Strip `s2script.apiVersion` from every in-repo plugin/example**

Run from the repo root:

```bash
for f in plugins/*/package.json plugins/disabled/*/package.json examples/*/package.json; do
  [ -f "$f" ] || continue
  node -e '
    const fs = require("fs");
    const p = process.argv[1];
    const j = JSON.parse(fs.readFileSync(p, "utf8"));
    if (j.s2script && "apiVersion" in j.s2script) {
      delete j.s2script.apiVersion;
      if (Object.keys(j.s2script).length === 0) delete j.s2script;
      fs.writeFileSync(p, JSON.stringify(j, null, 2) + "\n");
    }' "$f"
done
git diff --stat -- plugins examples | tail -3
```

Expected: ~50 package.json files changed, each losing exactly the `apiVersion` line (and the `s2script` block where it was the only key). Do NOT touch `packages/sdk/test/fixtures/**` (they keep authored values to test the ignore path). Verify: `grep -rl '"apiVersion"' plugins examples || echo CLEAN` → `CLEAN`.

- [ ] **Step 8: Rebuild the CLI, run the full SDK suite + typecheck gate**

```bash
cd packages/sdk && node build.mjs && npm test
cd ../.. && ./scripts/check-plugins-typecheck.sh
```

Expected: only the 13 known-failing tests fail; gate prints `PASS: all plugins and examples typecheck`.

- [ ] **Step 9: Commit**

```bash
git add packages/sdk/src/api-version.ts packages/sdk/src/build.ts packages/sdk/src/create/create.ts \
        packages/sdk/test/api-version.test.mjs packages/sdk/test/build.test.mjs plugins examples
git commit -m "sdk: derive manifest apiVersion at build from the SDK host-major constant (B1)

s2s build stamps apiVersion from api-version.ts (single TS source of truth,
drift-gated against core's HOST_API_VERSION_MAJOR by test). The authored
s2script.apiVersion field is ignored-with-WARN and removed from the create
template and every in-repo plugin/example. Loader major gate unchanged
(runtime backstop)."
```

---

### Task 2: Loader refusal memory — WARN once per file version, `failed` state visible

**Files:**
- Modify: `core/src/v8host.rs` (two tiny pub(crate) helpers next to `is_failed`, currently ~line 10905)
- Modify: `core/src/loader.rs` (`poll_plugins` — the two apiVersion refusal sites; the `Action::Unload` arm)
- Test: `core/src/loader.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: existing `FAILED_PLUGINS` thread_local + `is_failed(id)` in `v8host.rs`; `WATCH_STATE`/`WatchedPlugin` in `loader.rs`.
- Produces: `pub(crate) fn set_failed(id: &str, reason: &str)` and `pub(crate) fn clear_failed(id: &str)` in `core/src/v8host.rs` — **Task 3 calls `set_failed` for typesSha256 refusals**; the refusal-memory convention "refused NEW file ⇒ insert its `WATCH_STATE` row + `set_failed`" is what Task 3's `begin_load` refusal reuses.

The bug: `poll_plugins` refuses an apiVersion-incompatible `.s2sp` with `continue` and never records the path, so every scan re-diffs the file as NEW and re-WARNs (~once/second). Fix: the refusal is remembered exactly the way every other seen file is — a `WATCH_STATE` row keyed by path+mtime — plus a `FAILED_PLUGINS` entry so `sm plugins list` shows `failed` instead of silence. A refused *reload* keeps the old version running and just bumps the stored mtime (the same "failing reload leaves the running version untouched" doctrine as the typecheck gate).

- [ ] **Step 1: Write the failing test**

Append to `core/src/loader.rs` `mod tests` (uses the existing `make_test_s2sp` helper; V8 is never initialized — `set_failed`/`is_failed` are plain thread_locals):

```rust
    /// B1: a refused (apiVersion-incompatible) .s2sp is remembered by path+mtime — one WARN,
    /// a `failed` state, and NO re-processing on later scans until the file changes.
    #[test]
    fn refused_load_is_remembered_by_path_and_mtime() {
        let dir = std::env::temp_dir().join(format!("s2s-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk tempdir");
        let bytes = make_test_s2sp(
            r#"{"id":"@old/one","version":"0.1.0","apiVersion":"1.x"}"#,
            "module.exports={};",
        );
        let p = dir.join("old.s2sp");
        std::fs::write(&p, &bytes).expect("write s2sp");
        PLUGINS_DIR.with(|d| *d.borrow_mut() = Some(dir.clone()));

        // At least one real scan regardless of the throttle counter's current phase.
        for _ in 0..(POLL_THROTTLE as usize + 1) { poll_plugins(); }

        assert!(
            WATCH_STATE.with(|ws| ws.borrow().contains_key(&p)),
            "refusal must be remembered as a WATCH_STATE row (path+mtime) — no rescan-and-rewarn"
        );
        assert!(crate::v8host::is_failed("@old/one"), "refusal is operator-visible as `failed`");

        // Unchanged file ⇒ the diff yields NO action (this IS warn-once, structurally).
        let mtime_before = WATCH_STATE.with(|ws| ws.borrow().get(&p).map(|w| w.mtime));
        for _ in 0..(POLL_THROTTLE as usize + 1) { poll_plugins(); }
        let mtime_after = WATCH_STATE.with(|ws| ws.borrow().get(&p).map(|w| w.mtime));
        assert_eq!(mtime_before, mtime_after, "second scan must not re-process the refused file");

        // Cleanup so later tests on this (single) test thread see no leftovers.
        WATCH_STATE.with(|ws| { ws.borrow_mut().remove(&p); });
        crate::v8host::clear_failed("@old/one");
        PLUGINS_DIR.with(|d| *d.borrow_mut() = None);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p s2script-core refused_load_is_remembered`
Expected: compile FAIL — `crate::v8host::clear_failed` (and `is_failed` visibility from the loader test is fine, it's `pub(crate)`) does not exist yet; after adding stubs, the assertion `WATCH_STATE ... contains_key` fails because today's code `continue`s without inserting.

- [ ] **Step 3: Add the two helpers in `core/src/v8host.rs`**

Directly below `pub(crate) fn is_failed(...)` (~line 10905):

```rust
/// Mark a plugin FAILED without it ever loading (loader refusals: apiVersion major, and — B1 —
/// a `compiledAgainst` typesSha256 mismatch). Shows as `failed` in `sm plugins list`; cleared by
/// the next successful load (load_plugin_js clears on fresh load) or by `clear_failed`.
pub(crate) fn set_failed(id: &str, reason: &str) {
    FAILED_PLUGINS.with(|f| { f.borrow_mut().insert(id.to_string(), reason.to_string()); });
}

/// Drop a FAILED entry (loader: the file vanished — a removed plugin is not `failed`, it is gone).
pub(crate) fn clear_failed(id: &str) {
    FAILED_PLUGINS.with(|f| { f.borrow_mut().remove(id); });
}
```

- [ ] **Step 4: Rework the two refusal sites in `core/src/loader.rs` `poll_plugins`**

The `Action::Load` arm — replace:

```rust
                    if !api_version_compatible(&manifest.api_version) {
                        crate::v8host::log_warn(&format!(
                            "WARN: poll_plugins: refusing {:?}: apiVersion {:?} incompatible with host major {}",
                            path, manifest.api_version, HOST_API_VERSION_MAJOR
                        ));
                        continue;
                    }
```

with:

```rust
                    if !api_version_compatible(&manifest.api_version) {
                        let reason = format!(
                            "apiVersion {:?} incompatible with host major {} (rebuild with a matching @s2script/sdk)",
                            manifest.api_version, HOST_API_VERSION_MAJOR
                        );
                        crate::v8host::log_warn(&format!("WARN: poll_plugins: refusing {:?}: {}", path, reason));
                        crate::v8host::set_failed(&manifest.id, &reason);
                        // Remember the refusal by path+mtime: with a WATCH_STATE row the next scan
                        // diffs this file as UNCHANGED (no action, no re-WARN). A rebuilt file has a
                        // new mtime -> Reload action -> re-evaluated. This is the re-warn-bug fix.
                        WATCH_STATE.with(|ws| {
                            ws.borrow_mut().insert(path.clone(), WatchedPlugin { mtime, id: manifest.id.clone() });
                        });
                        continue;
                    }
```

The `Action::Reload` arm — replace:

```rust
                        if !api_version_compatible(&manifest.api_version) {
                            crate::v8host::log_warn(&format!(
                                "WARN: poll_plugins: refusing reload of {:?}: apiVersion {:?} incompatible with host major {}",
                                path, manifest.api_version, HOST_API_VERSION_MAJOR
                            ));
                            continue;
                        }
```

with:

```rust
                        if !api_version_compatible(&manifest.api_version) {
                            crate::v8host::log_warn(&format!(
                                "WARN: poll_plugins: refusing reload of {:?}: apiVersion {:?} incompatible with host major {} - keeping the running version",
                                path, manifest.api_version, HOST_API_VERSION_MAJOR
                            ));
                            // Failed-reload doctrine (same as the typecheck gate): the RUNNING version
                            // stays untouched. Bump the stored mtime so this WARN fires once per
                            // file version instead of every scan.
                            WATCH_STATE.with(|ws| {
                                if let Some(wp) = ws.borrow_mut().get_mut(&path) { wp.mtime = mtime; }
                            });
                            continue;
                        }
```

The `Action::Unload` arm — a vanished refused file must also drop its `failed` state. Replace:

```rust
            Action::Unload { path, id } => {
                // A parked (never-started) plugin just drops from WAITING; a loaded one gets teardown.
                let was_waiting = WAITING.with(|w| w.borrow_mut().remove(&id).is_some());
                if !was_waiting { crate::v8host::unload_plugin(&id); }
                crate::v8host::clear_pending_handoff(&id);   // Slice 5E.3: a final removal discards any captured handoff
                removes.push(path);
            }
```

with:

```rust
            Action::Unload { path, id } => {
                // A parked (never-started) plugin just drops from WAITING; a loaded one gets teardown.
                let was_waiting = WAITING.with(|w| w.borrow_mut().remove(&id).is_some());
                if !was_waiting { crate::v8host::unload_plugin(&id); }
                crate::v8host::clear_pending_handoff(&id);   // Slice 5E.3: a final removal discards any captured handoff
                crate::v8host::clear_failed(&id);            // B1: a removed refused/failed file is gone, not `failed`
                removes.push(path);
            }
```

- [ ] **Step 5: Run the test + the whole core suite**

Run: `cargo test -p s2script-core refused_load_is_remembered` → PASS.
Run: `cargo test -p s2script-core` → all green (single-threaded; no `--test-threads`).

- [ ] **Step 6: Commit**

```bash
git add core/src/loader.rs core/src/v8host.rs
git commit -m "loader: remember refused .s2sp by path+mtime - WARN once, failed state visible (B1)

A refused (apiVersion-incompatible) load gets a WATCH_STATE row + FAILED_PLUGINS
entry instead of a bare continue, killing the every-poll re-WARN; a refused
reload keeps the running version and bumps the stored mtime. Vanished files
clear their failed state."
```

---

### Task 3: `typesSha256` load-verify — contract drift fails fast at load AND per-call

**Files:**
- Modify: `core/src/interfaces.rs` (`ImportSpec` new, `ImportDecl` grows the hash, `InterfaceEntry.types_sha256`, `CallTarget::TypesMismatch`, `publish`/`set_imports` signatures, `call_target_inner`)
- Modify: `core/src/v8host.rs` (`set_plugin_imports` signature, `s2_iface_publish` passes the hash, `s2_iface_call` maps the new target, new `iface_published_types_sha256`)
- Modify: `core/src/loader.rs` (`Manifest.compiled_against`, `imports_from_manifest`, `verify_compiled_against` + refusal in `begin_load`, dedupe the `PendingOp::Reload` body into `begin_load`)
- Test: unit tests in `core/src/interfaces.rs` + `core/src/loader.rs`, one in-isolate test in `core/src/v8host.rs`

**Interfaces:**
- Consumes: Task 2's `crate::v8host::set_failed(id, reason)` + the "refused ⇒ WATCH_STATE row + set_failed" convention.
- Consumes (wire, from this plan; produced concretely by Task 4): manifest key `"compiledAgainst": {"<interface-name>": "<sha256-hex>"}` — sha256 of the raw bytes of the consumer's local copy of the producer contract, hex-lowercase, same as `publishes[*].typesSha256`.
- Produces:
  - `pub struct ImportSpec { pub name: String, pub range: String, pub kind: Kind, pub compiled_types_sha256: Option<String> }` + `ImportSpec::new(name, range, kind) -> Self` (hash `None`) in `core/src/interfaces.rs`.
  - `CallTarget::TypesMismatch` variant; `InterfaceRegistry::publish(&mut self, name, version, types_sha256: &str, producer_id, producer_gen, method_names)`.
  - `pub fn set_imports(&mut self, plugin_id: &str, decls: Vec<ImportSpec>)`; `pub fn set_plugin_imports(id: &str, decls: Vec<crate::interfaces::ImportSpec>)` in v8host.
  - `pub(crate) fn iface_published_types_sha256(name: &str) -> Option<String>` in v8host.
  - JS-visible error name: **`InterfaceTypesMismatch`** (sibling of `InterfaceVersionMismatch`).

- [ ] **Step 1: Write the failing pure-registry tests**

In `core/src/interfaces.rs`'s test module (create `#[cfg(test)] mod tests` at the bottom if the file has none — check first; if tests for this registry live in `v8host.rs`, put these there instead, same content):

```rust
    #[test]
    fn call_target_types_mismatch_when_compiled_hash_differs() {
        let mut r = InterfaceRegistry::new();
        r.publish("@x/if", "1.0.0", "aaa111", "prod", 1, vec!["m".into()]).expect("publish");
        r.set_imports("cons", vec![ImportSpec {
            name: "@x/if".into(), range: "^1.0.0".into(), kind: Kind::Hard,
            compiled_types_sha256: Some("bbb222".into()),
        }]);
        assert_eq!(r.call_target("cons", "@x/if", "m"), CallTarget::TypesMismatch);
    }

    #[test]
    fn call_target_ok_when_hash_matches_absent_or_unpublished() {
        let mut r = InterfaceRegistry::new();
        r.publish("@x/if", "1.0.0", "aaa111", "prod", 1, vec!["m".into()]).expect("publish");
        // Matching hash -> Ok.
        r.set_imports("c1", vec![ImportSpec {
            name: "@x/if".into(), range: "^1.0.0".into(), kind: Kind::Hard,
            compiled_types_sha256: Some("aaa111".into()),
        }]);
        assert_eq!(r.call_target("c1", "@x/if", "m"), CallTarget::Ok);
        // Consumer shipped no hash (no local contract copy) -> unverified, Ok (today's contract).
        r.set_imports("c2", vec![ImportSpec::new("@x/if", "^1.0.0", Kind::Hard)]);
        assert_eq!(r.call_target("c2", "@x/if", "m"), CallTarget::Ok);
        // Producer published no hash (empty string) -> nothing to verify against, Ok.
        let mut r2 = InterfaceRegistry::new();
        r2.publish("@y/if", "1.0.0", "", "prod", 1, vec!["m".into()]).expect("publish");
        r2.set_imports("c3", vec![ImportSpec {
            name: "@y/if".into(), range: "^1.0.0".into(), kind: Kind::Hard,
            compiled_types_sha256: Some("bbb222".into()),
        }]);
        assert_eq!(r2.call_target("c3", "@y/if", "m"), CallTarget::Ok);
    }
```

And in `core/src/loader.rs` `mod tests`:

```rust
    #[test]
    fn manifest_parses_compiled_against_and_flows_into_imports() {
        let json = r#"{"id":"@demo/c","version":"0.1.0","apiVersion":"2.x",
            "pluginDependencies":{"@x/if":"^1.0.0","@x/other":"^1.0.0"},
            "compiledAgainst":{"@x/if":"deadbeef"}}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert_eq!(m.compiled_against.get("@x/if").map(String::as_str), Some("deadbeef"));
        let imports = imports_from_manifest(&m);
        let with = imports.iter().find(|i| i.name == "@x/if").expect("present");
        assert_eq!(with.compiled_types_sha256.as_deref(), Some("deadbeef"));
        let without = imports.iter().find(|i| i.name == "@x/other").expect("present");
        assert_eq!(without.compiled_types_sha256, None);
    }

    #[test]
    fn manifest_without_compiled_against_defaults_empty() {
        let json = r#"{"id":"@demo/x","version":"0.1.0","apiVersion":"2.x"}"#;
        let m: Manifest = serde_json::from_str(json).expect("parse");
        assert!(m.compiled_against.is_empty());
    }
```

- [ ] **Step 2: Run to verify compile failure**

Run: `cargo test -p s2script-core call_target_types_mismatch`
Expected: compile FAIL (`ImportSpec` / `TypesMismatch` / `publish` arity / `compiled_against` don't exist).

- [ ] **Step 3: Implement `core/src/interfaces.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallTarget { Ok, Unavailable, VersionMismatch, TypesMismatch }
```

Replace `struct ImportDecl { range: String, kind: Kind }` and add the public spec type:

```rust
/// One declared inter-plugin import, as the loader hands it over from the manifest.
#[derive(Debug, Clone)]
pub struct ImportSpec {
    pub name: String,
    pub range: String,
    pub kind: Kind,
    /// sha256 (hex) of the producer-contract bytes this consumer COMPILED against
    /// (manifest `compiledAgainst`). None when the consumer shipped no verified copy
    /// — unverified imports keep the pre-B1 contract (no hash check).
    pub compiled_types_sha256: Option<String>,
}

impl ImportSpec {
    /// Hash-less spec (the dominant/test case).
    pub fn new(name: &str, range: &str, kind: Kind) -> Self {
        Self { name: name.into(), range: range.into(), kind, compiled_types_sha256: None }
    }
}

struct ImportDecl { range: String, kind: Kind, compiled_types_sha256: Option<String> }
```

`InterfaceEntry` gains the published hash:

```rust
#[derive(Debug, Clone)]
pub struct InterfaceEntry {
    pub version: String,
    /// sha256 of the contract .d.ts the producer published (manifest publishes[*].typesSha256;
    /// "" when the producer predates B1 or ships no hash — then nothing is verified).
    pub types_sha256: String,
    pub producer_id: String,
    pub producer_gen: u64,
    pub method_names: Vec<String>,
    pub subscribers: Vec<Subscriber>,
}
```

`publish` takes the hash (insert `types_sha256` after `version` in both the signature and the `InterfaceEntry` construction):

```rust
    pub fn publish(
        &mut self,
        name: &str,
        version: &str,
        types_sha256: &str,
        producer_id: &str,
        producer_gen: u64,
        method_names: Vec<String>,
    ) -> Result<(), String> {
        // ... unchanged duplicate-producer refusal + subscriber preservation ...
        self.ifaces.insert(name.to_string(), InterfaceEntry {
            version: version.to_string(),
            types_sha256: types_sha256.to_string(),
            producer_id: producer_id.to_string(),
            producer_gen,
            method_names,
            subscribers,
        });
        Ok(())
    }
```

`set_imports` consumes specs:

```rust
    pub fn set_imports(&mut self, plugin_id: &str, decls: Vec<ImportSpec>) {
        let map = decls.into_iter()
            .map(|s| (s.name, ImportDecl {
                range: s.range, kind: s.kind, compiled_types_sha256: s.compiled_types_sha256,
            }))
            .collect();
        self.imports.insert(plugin_id.to_string(), map);
    }
```

`call_target_inner` — after the version check, before the method check:

```rust
    fn call_target_inner(&self, plugin_id: &str, name: &str, method: Option<&str>) -> CallTarget {
        let Some(decl) = self.imports.get(plugin_id).and_then(|m| m.get(name)) else {
            return CallTarget::Unavailable;
        };
        let Some(entry) = self.ifaces.get(name) else { return CallTarget::Unavailable };
        if !version_satisfies(&decl.range, &entry.version) { return CallTarget::VersionMismatch; }
        // B1 typesSha256 verify (north-star §5.2): the consumer compiled against a specific
        // contract; if the producer's published hash differs, every call is unsound — refuse
        // loudly rather than marshal across a drifted contract. Verified only when BOTH sides
        // carry a hash (fail-open for pre-B1 artifacts; the doctrine gate is at load, this is
        // the always-on backstop for late-arriving producers).
        if let (Some(expected), false) = (&decl.compiled_types_sha256, entry.types_sha256.is_empty()) {
            if !expected.is_empty() && *expected != entry.types_sha256 {
                return CallTarget::TypesMismatch;
            }
        }
        if let Some(m) = method {
            if !entry.method_names.iter().any(|n| n == m) { return CallTarget::Unavailable; }
        }
        CallTarget::Ok
    }
```

(Note `import_range` is subsumed — delete it and use `decl.range` directly, or keep it delegating; prefer delete.)

- [ ] **Step 4: Ripple through `core/src/v8host.rs`**

1. `set_plugin_imports` (~line 4674):

```rust
pub fn set_plugin_imports(id: &str, decls: Vec<crate::interfaces::ImportSpec>) {
    IFACES.with(|r| r.borrow_mut().set_imports(id, decls));
}
```

2. `s2_iface_publish` — the `IFACES.publish` call site gains the hash (the `decl` in scope is the manifest `PublishDecl`):

```rust
        if let Err(e) = IFACES.with(|r| {
            r.borrow_mut().publish(&name, &decl.version, &decl.types_sha256, &owner, generation, method_names)
        }) {
```

3. `s2_iface_call` — the `match target` arms (~line 5038) gain:

```rust
            crate::interfaces::CallTarget::TypesMismatch => { throw_named(scope, "InterfaceTypesMismatch", &name); return; }
```

Grep for every other exhaustive `match` on `CallTarget` (`rg 'CallTarget::' core/src`) and give each a `TypesMismatch` arm with the same semantics as `VersionMismatch` at that site (e.g. a subscribe path that treats non-`Ok` as unavailable keeps doing so).

4. New accessor next to `iface_published` (~line 10917):

```rust
/// The published contract hash for `name` (empty string = producer ships none), None when
/// unpublished. The loader's `verify_compiled_against` (B1) fail-fast gate reads this.
pub(crate) fn iface_published_types_sha256(name: &str) -> Option<String> {
    IFACES.with(|r| r.borrow().lookup(name).map(|e| e.types_sha256.clone()))
}
```

5. Fix every now-broken caller/test: `rg 'set_plugin_imports|\.set_imports\(|\.publish\(' core/src` — test call sites like
`set_plugin_imports("cons", vec![("@x/greeter".into(), "^1.0.0".into(), crate::interfaces::Kind::Hard)]);` become
`set_plugin_imports("cons", vec![crate::interfaces::ImportSpec::new("@x/greeter", "^1.0.0", crate::interfaces::Kind::Hard)]);`
and registry `publish("n","1.0.0","prod",...)` test calls gain a hash argument (`"h"` or `""`) in position 3.

- [ ] **Step 5: Implement the loader side in `core/src/loader.rs`**

`Manifest` gains:

```rust
    /// B1: interface-name → sha256 of the contract bytes this plugin COMPILED against
    /// (`s2s build` hashes the consumer's `.s2script/types/<iface>/index.d.ts` copy).
    /// Verified against the producer's published typesSha256 at load (fail-fast) and
    /// per-call (late-producer backstop). Empty for pre-B1 or copy-less consumers.
    #[serde(rename = "compiledAgainst", default)]
    pub compiled_against: std::collections::HashMap<String, String>,
```

`imports_from_manifest` returns specs:

```rust
fn imports_from_manifest(m: &Manifest) -> Vec<crate::interfaces::ImportSpec> {
    let mut out = Vec::new();
    for (name, range) in &m.plugin_dependencies {
        out.push(crate::interfaces::ImportSpec {
            name: name.clone(), range: range.clone(), kind: crate::interfaces::Kind::Hard,
            compiled_types_sha256: m.compiled_against.get(name).cloned(),
        });
    }
    for (name, range) in &m.optional_plugin_dependencies {
        out.push(crate::interfaces::ImportSpec {
            name: name.clone(), range: range.clone(), kind: crate::interfaces::Kind::Optional,
            compiled_types_sha256: m.compiled_against.get(name).cloned(),
        });
    }
    out
}
```

(The `legacy_manifest_with_builtins_in_plugin_deps_still_loads` test's assertions change from tuple-field access to `i.kind` / `i.name` on `ImportSpec` — update in place.)

New verify + refusal in `begin_load` (reusing Task 2's convention):

```rust
/// B1 (north-star §5.2): fail-fast contract-drift gate. A consumer that compiled against a
/// dependency contract whose hash differs from what the producer CURRENTLY publishes is refused
/// at load — completing "fails at typecheck AND again at load". Producer-absent deps are not
/// checked here (lazy hard-dep contract); if such a producer appears later with a different
/// hash, every call throws `InterfaceTypesMismatch` (interfaces.rs backstop).
fn verify_compiled_against(manifest: &Manifest) -> Result<(), String> {
    let mut names: Vec<&String> = manifest.compiled_against.keys().collect();
    names.sort(); // deterministic first-error
    for iface in names {
        let built = &manifest.compiled_against[iface];
        if built.is_empty() { continue; }
        let Some(published) = crate::v8host::iface_published_types_sha256(iface) else { continue };
        if !published.is_empty() && published != *built {
            return Err(format!(
                "contract drift on '{}': compiled against typesSha256 {}… but the producer publishes {}… — refresh .s2script/types/{}/index.d.ts from the producer and rebuild",
                iface,
                &built[..12.min(built.len())],
                &published[..12.min(published.len())],
                iface
            ));
        }
    }
    Ok(())
}
```

`begin_load` gets the gate at the top:

```rust
fn begin_load(manifest: &Manifest, js: &str, path: &Path, mtime: SystemTime) {
    // B1: typesSha256 fail-fast. Refusal is remembered exactly like an apiVersion refusal:
    // WATCH_STATE row (path+mtime => warn once) + FAILED state (operator-visible). A rebuilt
    // file (new mtime) retries.
    if let Err(reason) = verify_compiled_against(manifest) {
        crate::v8host::log_warn(&format!("WARN: poll_plugins: refusing {:?}: {}", path, reason));
        crate::v8host::set_failed(&manifest.id, &reason);
        WATCH_STATE.with(|ws| {
            ws.borrow_mut().insert(path.to_path_buf(), WatchedPlugin { mtime, id: manifest.id.clone() });
        });
        return;
    }
    // ... existing body unchanged ...
```

Dedupe the `PendingOp::Reload` arm in `drain_pending_ops`: its `Ok((manifest, js))` body currently repeats `begin_load`'s five calls inline — replace that body with:

```rust
                    Ok((manifest, js)) => {
                        crate::v8host::unload_plugin(&id);   // no-op if not currently loaded
                        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH);
                        begin_load(&manifest, &js, &path, mtime);
                        crate::v8host::log_warn(&format!("[plugins] reloaded '{}' (sm plugins reload)", id));
                    }
```

(Behavior preserved: `begin_load` performs the same `set_plugin_imports`/`set_plugin_publishes`/`materialize_for_load`/`start_load`/`store_config_decls`/`WATCH_STATE` sequence — and now also the hash gate.)

- [ ] **Step 6: Run the pure tests**

Run: `cargo test -p s2script-core call_target_types_mismatch call_target_ok_when_hash manifest_parses_compiled_against manifest_without_compiled_against`
Expected: PASS. Then `cargo test -p s2script-core` — fix any remaining `set_imports`/`publish` arity fallout until green (only the 0 pre-existing failures — the core suite is fully green on this branch).

- [ ] **Step 7: Write the in-isolate late-producer test (per-call backstop)**

In `core/src/v8host.rs`'s in-isolate test module (the one containing `iface` tests around line 11900 — copy the harness idioms of the neighboring test that asserts `InterfaceUnavailable` at ~12188: same `load_plugin_js` + `eval_in_context_string` helpers):

```rust
    /// B1: consumer compiled against hash "bbb…" but the producer publishes "aaa…" — every call
    /// throws InterfaceTypesMismatch (the late-producer backstop; load-time refusal is loader-side).
    #[test]
    fn iface_call_throws_types_mismatch_when_compiled_hash_differs() {
        ensure_v8();
        set_plugin_publishes("tm_prod", [(
            "@x/tm".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "aaa111".into() },
        )].into_iter().collect());
        load_plugin_js("tm_prod",
            "module.exports.default={__s2plugin:1,factory:function(ctx){ctx.publish(\"@x/tm\",{ping:function(){return 1;}});}};",
            "{}");
        set_plugin_imports("tm_cons", vec![crate::interfaces::ImportSpec {
            name: "@x/tm".into(), range: "^1.0.0".into(), kind: crate::interfaces::Kind::Hard,
            compiled_types_sha256: Some("bbb222".into()),
        }]);
        load_plugin_js("tm_cons",
            "module.exports.default={__s2plugin:1,factory:function(ctx){var h=ctx.use(\"@x/tm\");globalThis.__tmcall=function(){try{return String(h.ping());}catch(e){return String(e);}};}};",
            "{}");
        let out = eval_in_context_string("tm_cons", "globalThis.__tmcall()");
        assert!(out.contains("InterfaceTypesMismatch"), "got: {}", out);
        unload_plugin("tm_cons");
        unload_plugin("tm_prod");
    }
```

**NOTE for the implementer:** `ensure_v8()`, `eval_in_context_string`, exact `set_plugin_publishes` map-construction style, and whether `ctx.use` requires the import decl to pre-exist are all conventions of the *neighboring tests* — mirror the test at ~12275 (`InterfaceUnavailable` round-trip) verbatim for setup/teardown; only the hash fields and the assertion differ. If that harness names its helpers differently, follow the harness, not this sketch.

- [ ] **Step 8: Run the full core suite**

Run: `cargo test -p s2script-core`
Expected: all green.

- [ ] **Step 9: Commit**

```bash
git add core/src/interfaces.rs core/src/v8host.rs core/src/loader.rs
git commit -m "core: verify typesSha256 at load + per-call (B1) - contract drift fails fast

The previously-inert manifest hash is now enforced twice: begin_load refuses a
consumer whose compiledAgainst hash differs from the producer's published
typesSha256 (WATCH_STATE+failed refusal memory), and call_target returns
TypesMismatch -> throw InterfaceTypesMismatch for the late-producer window.
Fail-open when either side ships no hash (pre-B1 artifacts)."
```

---

### Task 4: Consumer `compiledAgainst` at build — the `.s2script/types/` verified-copy convention

**Files:**
- Create: `packages/sdk/src/contracts.ts`
- Modify: `packages/sdk/src/typecheck/typecheck.ts` (paths-map local contract copies instead of `any`-stubbing them)
- Modify: `packages/sdk/src/build.ts` (emit `manifest.compiledAgainst`)
- Create: `packages/sdk/test/fixtures/consumer-verified/` (package.json, `src/plugin.ts`, `.s2script/types/@demo/greeter/index.d.ts`)
- Create: `packages/sdk/test/compiled-against.test.mjs`
- Modify: `examples/zones-consumer-demo/` (port to the verified-copy flow: new `.s2script/types/@s2script/zones/index.d.ts`, `src/plugin.ts` imports, `tsconfig.json`)

**Interfaces:**
- Consumes: Task 1's `build.ts` (stamped apiVersion in place); `hashContract(typesPath)` from `packages/sdk/src/publishes.ts` (sha256 hex of raw bytes).
- Produces:
  - `export function localContractPath(pluginDir: string, dep: string): string | null` in `packages/sdk/src/contracts.ts` — resolves `<pluginDir>/.s2script/types/<dep>/index.d.ts`, `null` when absent or `dep` contains a path-traversal segment.
  - Manifest key `compiledAgainst: Record<string, string>` (interface name → sha256 hex) — **exactly what Task 3's loader deserializes**.
  - Typecheck behavior later tasks rely on: a dep with a local contract copy resolves to REAL types (no `any` stub) — Task 9's lint program inherits this for free.

The convention (design spec 2026-07-15 §4.6, first landed here): a consumer that wants verified types for a plugin-published interface keeps a **byte-copy** of the producer's contract at `.s2script/types/<interface-name>/index.d.ts` (scoped names nest: `.s2script/types/@s2script/zones/index.d.ts`). Build (a) typechecks against it via a `paths` mapping — real types instead of the ambient `any` stub — and (b) hashes those bytes into `manifest.compiledAgainst`. The producer hashed the SAME file bytes into `publishes[*].typesSha256`, so equal file ⟺ equal hash; drift ⟹ load refusal (Task 3). Fetching the copy automatically (`s2s add`) is registry-slice work — here the copy is made by hand/cp (see Open Questions #4).

- [ ] **Step 1: Create the fixture**

`packages/sdk/test/fixtures/consumer-verified/package.json`:

```json
{
  "name": "@demo/consumer-verified",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": {
    "pluginDependencies": {
      "@demo/greeter": "^1.0.0"
    }
  }
}
```

`packages/sdk/test/fixtures/consumer-verified/.s2script/types/@demo/greeter/index.d.ts`:

```ts
/** @demo/greeter contract (verified copy for the compiledAgainst fixture). */
export interface Greeter {
  greet(name: string): string;
}
```

`packages/sdk/test/fixtures/consumer-verified/src/plugin.ts`:

```ts
import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "@demo/greeter";

export default plugin((ctx) => {
  const g = ctx.use<Greeter>("@demo/greeter");
  ctx.commands.register("greet_me", (cmd) => {
    cmd.reply(g.greet("world"));
  });
});
```

- [ ] **Step 2: Write the failing tests**

Create `packages/sdk/test/compiled-against.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import AdmZip from "adm-zip";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { localContractPath } from "../src/contracts.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = join(here, "fixtures", "consumer-verified");
const packagesDir = join(here, "..", "..");
const contractFile = join(fixture, ".s2script", "types", "@demo", "greeter", "index.d.ts");

test("localContractPath resolves the verified copy and refuses traversal", () => {
  assert.equal(localContractPath(fixture, "@demo/greeter"), contractFile);
  assert.equal(localContractPath(fixture, "@demo/absent"), null);
  assert.equal(localContractPath(fixture, "../evil"), null);
  assert.equal(localContractPath(fixture, "@demo/.."), null);
});

test("build emits compiledAgainst = sha256 of the verified copy's raw bytes", async () => {
  const out = await buildPlugin(fixture, packagesDir);
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  const expected = createHash("sha256").update(readFileSync(contractFile)).digest("hex");
  assert.deepEqual(manifest.compiledAgainst, { "@demo/greeter": expected });
});

test("the verified copy replaces the any-stub: misuse of the contract FAILS the typecheck", () => {
  // g.greet(42) is fine against an `any` stub; against the real contract it is TS2345.
  const src = join(fixture, "src", "plugin.ts");
  const good = readFileSync(src, "utf8");
  writeFileSync(src, good.replace('g.greet("world")', "g.greet(42 as unknown as number)"));
  try {
    const r = typecheckPlugin(fixture, { packagesDir });
    assert.equal(r.ok, false, "wrong arg type against the verified contract must fail");
    assert.ok(r.diagnostics.some((d) => d.code === 2345), "expects TS2345 argument-type error");
  } finally {
    writeFileSync(src, good);
  }
});
```

- [ ] **Step 3: Run to verify they fail**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/compiled-against.test.mjs`
Expected: FAIL — `../src/contracts.ts` not found; then (after Step 4 alone) the build test fails with `compiledAgainst` undefined and the misuse test fails because the dep still stubs to `any`.

- [ ] **Step 4: Create `packages/sdk/src/contracts.ts`**

```ts
/**
 * The verified-copy convention (design spec 2026-07-15 §4.6, landed in B1): a consumer keeps a
 * BYTE-copy of a producer's published contract at `.s2script/types/<interface>/index.d.ts`.
 * The typecheck gate paths-maps the interface module to it (real types, not an `any` stub) and
 * `s2s build` hashes the same bytes into `manifest.compiledAgainst[<interface>]`, which the
 * loader verifies against the producer's published `typesSha256` (fail-fast + per-call).
 */

import { existsSync } from "node:fs";
import { join } from "node:path";

/** Absolute path of the plugin's verified copy for `dep`, or null (absent / traversal-unsafe). */
export function localContractPath(pluginDir: string, dep: string): string | null {
  const segs = dep.split("/");
  if (segs.some((s) => s === "" || s === "." || s === "..")) return null;
  const p = join(pluginDir, ".s2script", "types", ...segs, "index.d.ts");
  return existsSync(p) ? p : null;
}
```

- [ ] **Step 5: Wire the typecheck gate (`packages/sdk/src/typecheck/typecheck.ts`)**

Add the import:

```ts
import { localContractPath } from "../contracts.ts";
```

In `typecheckPlugin`, after `const locallyDeclared = declaredModules(localDts);` insert:

```ts
  // B1: a dep with a verified contract copy (.s2script/types/<dep>/index.d.ts) resolves to REAL
  // types via an exact `paths` entry — never the ambient `any` stub. This is what makes the
  // manifest's compiledAgainst hash a statement about types the build actually checked.
  const allDeclaredDeps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
  ];
  const contractPaths: Record<string, string[]> = {};
  for (const d of allDeclaredDeps) {
    const p = localContractPath(absDir, d);
    if (p !== null) contractPaths[d] = [p];
  }
```

Change the `deps` stub filter to exclude verified deps:

```ts
  const deps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
  ].filter((d) => !isAlwaysResolved(d) && !locallyDeclared.has(d) && contractPaths[d] === undefined);
```

Merge the exact entries into `paths` (exact keys beat the `*` patterns — TS prefers the longest/exact match):

```ts
    paths: {
      "@s2script/sdk/*": ["sdk/*.d.ts"],
      "@s2script/*": ["*/index.d.ts"],
      ...contractPaths,
    },
```

**Absolute-path note:** `paths` values are resolved relative to `baseUrl` but tsc accepts already-absolute substitutions on POSIX. If the misuse test still sees `any` (resolution miss), convert to relative at the merge point: `contractPaths[d] = [relative(packagesDir, p)]` (import `relative` from `node:path`) — the fixture test locks in whichever form actually resolves.

- [ ] **Step 6: Emit the manifest key in `packages/sdk/src/build.ts`**

Add imports:

```ts
import { localContractPath } from "./contracts.ts";
import { hashContract } from "./publishes.ts";
```

(`hashContract` is already exported from `publishes.ts`; `derivePublishes` import stays.)

After the `manifest` object literal is built (right below the `if (Object.keys(derivedPublishes).length > 0)` block), insert:

```ts
  // --- compiledAgainst (B1): hash every verified contract copy this consumer typechecked
  // against. The loader compares these to the producer's published typesSha256 at load
  // (fail-fast) and per-call (late-producer backstop).
  const compiledAgainst: Record<string, string> = {};
  for (const dep of [
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]) {
    const contractPath = localContractPath(absDir, dep);
    if (contractPath !== null) compiledAgainst[dep] = hashContract(contractPath);
  }
  if (Object.keys(compiledAgainst).length > 0) manifest.compiledAgainst = compiledAgainst;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/compiled-against.test.mjs test/build.test.mjs test/typecheck.test.mjs`
Expected: PASS (all three files; no regressions in the pre-existing build/typecheck tests).

- [ ] **Step 8: Port `examples/zones-consumer-demo` to the verified-copy flow**

```bash
mkdir -p "examples/zones-consumer-demo/.s2script/types/@s2script/zones"
cp plugins/zones/api.d.ts "examples/zones-consumer-demo/.s2script/types/@s2script/zones/index.d.ts"
```

In `examples/zones-consumer-demo/src/plugin.ts`, replace the two relative-reach type imports:

```ts
import type { Zones } from "../../../plugins/zones/api";
```
and
```ts
import type { ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "../../../plugins/zones/api";
```

with:

```ts
import type { Zones, ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent } from "@s2script/zones";
```

and replace the now-stale comment block above them (the one explaining the relative-reach hack and "Replace this import when that lands") with:

```ts
// TYPES come from the verified contract copy (.s2script/types/@s2script/zones/index.d.ts — a
// byte-copy of the producer's api.d.ts). `s2s build` typechecks against it AND hashes it into
// manifest.compiledAgainst, so if the producer's contract drifts, this consumer is refused at
// load instead of marshalling across a stale contract (B1). Refresh with:
//   cp plugins/zones/api.d.ts examples/zones-consumer-demo/.s2script/types/@s2script/zones/index.d.ts
```

Replace `examples/zones-consumer-demo/tsconfig.json` so the EDITOR resolves the same copy (build/editor parity):

```json
{
  "extends": "../../tsconfig.base.json",
  "compilerOptions": {
    "baseUrl": "../../packages",
    "paths": {
      "@s2script/sdk/*": ["sdk/*.d.ts"],
      "@s2script/zones": ["../examples/zones-consumer-demo/.s2script/types/@s2script/zones/index.d.ts"],
      "@s2script/*": ["*/index.d.ts"]
    }
  },
  "include": ["src", "../../packages/sdk/globals.d.ts"]
}
```

- [ ] **Step 9: Gate + build the example end-to-end**

```bash
./scripts/check-plugins-typecheck.sh
cd packages/sdk && node build.mjs && cd ../..
node packages/sdk/dist/cli.js build examples/zones-consumer-demo
python3 - <<'EOF'
import zipfile, json, hashlib
z = zipfile.ZipFile("examples/zones-consumer-demo/dist/_demo_zones-consumer-demo.s2sp")
m = json.loads(z.read("manifest.json"))
h = hashlib.sha256(open("plugins/zones/api.d.ts","rb").read()).hexdigest()
assert m["compiledAgainst"]["@s2script/zones"] == h, m
print("compiledAgainst OK:", h[:12])
EOF
```

Expected: gate passes (zones-consumer-demo now typechecks against REAL zones types); the python check prints `compiledAgainst OK: …`. (If the sanitized .s2sp filename differs, `ls examples/zones-consumer-demo/dist/`.)

- [ ] **Step 10: Commit**

```bash
git add packages/sdk/src/contracts.ts packages/sdk/src/typecheck/typecheck.ts packages/sdk/src/build.ts \
        packages/sdk/test/compiled-against.test.mjs packages/sdk/test/fixtures/consumer-verified \
        examples/zones-consumer-demo
git commit -m "sdk: .s2script/types verified-copy convention -> typed deps + manifest compiledAgainst (B1)

A consumer's local byte-copy of a producer contract now (a) replaces the any-stub
in the typecheck gate via an exact paths entry and (b) is hashed into
manifest.compiledAgainst for the loader's typesSha256 verify. zones-consumer-demo
ported off the relative-reach hack as the living example."
```

---

### Task 5: `publishes` derived from code (`ctx.publish` scan) + dependency advisories

**Files:**
- Create: `packages/sdk/src/publish-scan.ts`
- Modify: `packages/sdk/src/typecheck/typecheck.ts` (`TypecheckResult` gains `program`)
- Modify: `packages/sdk/src/build.ts` (pipeline reorder + reconciliation-as-generation + advisories)
- Create: `packages/sdk/test/publish-scan.test.mjs`
- Create: fixtures `packages/sdk/test/fixtures/publisher-derived-self/`, `publisher-drift/`, `publisher-dynamic/`
- Possibly modify: existing publisher fixtures (`publisher-mapform*`, `publisher-renamed`, `producer`) so their code and manifest agree — the new build check makes disagreement an error.

**Interfaces:**
- Consumes: Task 4's `build.ts`/`typecheck.ts` state; `expandPublishes` from `publishes.ts`; `hasPublishes` from `publish-gate.ts`; `PluginContext` symbol name from `packages/sdk/plugin.d.ts` (L1-frozen).
- Produces:
  - `TypecheckResult` gains `program?: import("typescript").Program` (set on every successful run) — **Task 9 passes this program to the lint engine.**
  - `export interface PublishScan { publishNames: string[]; dynamicPublishSites: string[]; useNames: string[] }` and `export function scanPluginProgram(program: ts.Program, pluginDir: string): PublishScan` from `packages/sdk/src/publish-scan.ts`.

Scope decision (per the spec's "if this is genuinely hard, scope it down"): full generation is only well-defined for the `"self"` case (a renamed/decoupled contract needs an authored concrete version — versions are data, not code). So: **the name-set is derived from code and is authoritative**; `"self"` is auto-derived when code publishes exactly the package's own name and nothing is authored; any authored block must reconcile exactly with the derived set (build error on drift — reconciliation becomes generation + verification); a non-literal `ctx.publish` name is a build error (it would defeat derivation AND fail runtime reconciliation anyway). Conditional publishes (inside `if`) are statically visible and pass the build; the runtime `reconcile_publishes` keeps catching a publish that never ran — the residual check the spec says stays.

- [ ] **Step 1: Create the three fixtures**

`packages/sdk/test/fixtures/publisher-derived-self/package.json`:

```json
{
  "name": "@demo/derived-self",
  "version": "1.2.0",
  "main": "src/plugin.ts",
  "types": "api.d.ts"
}
```

`packages/sdk/test/fixtures/publisher-derived-self/api.d.ts`:

```ts
/** @demo/derived-self contract. */
export interface DerivedSelf {
  ping(): number;
}
```

`packages/sdk/test/fixtures/publisher-derived-self/src/plugin.ts`:

```ts
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.publish("@demo/derived-self", {
    ping: () => 1,
  });
});
```

(NOTE: no `s2script.publishes` — the block is DERIVED.)

`packages/sdk/test/fixtures/publisher-drift/package.json`:

```json
{
  "name": "@demo/drift",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "types": "api.d.ts",
  "s2script": {
    "publishes": "self"
  }
}
```

`packages/sdk/test/fixtures/publisher-drift/api.d.ts`:

```ts
export interface Drift { ping(): number; }
```

`packages/sdk/test/fixtures/publisher-drift/src/plugin.ts`:

```ts
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.publish("@demo/other-name", {
    ping: () => 1,
  });
});
```

`packages/sdk/test/fixtures/publisher-dynamic/package.json`:

```json
{
  "name": "@demo/dynamic",
  "version": "1.0.0",
  "main": "src/plugin.ts",
  "types": "api.d.ts",
  "s2script": {
    "publishes": "self"
  }
}
```

`packages/sdk/test/fixtures/publisher-dynamic/api.d.ts`:

```ts
export interface Dyn { ping(): number; }
```

`packages/sdk/test/fixtures/publisher-dynamic/src/plugin.ts`:

```ts
import { plugin } from "@s2script/sdk/plugin";

const NAME = ["@demo", "dynamic"].join("/");

export default plugin((ctx) => {
  ctx.publish(NAME, {
    ping: () => 1,
  });
});
```

- [ ] **Step 2: Write the failing tests**

Create `packages/sdk/test/publish-scan.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import AdmZip from "adm-zip";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { scanPluginProgram } from "../src/publish-scan.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

test("scanPluginProgram collects literal ctx.publish/use names off the PluginContext type", () => {
  const dir = fx("consumer-verified"); // Task 4's fixture: one ctx.use("@demo/greeter")
  const r = typecheckPlugin(dir, { packagesDir });
  assert.ok(r.ok && r.program, "fixture typechecks and returns its program");
  const scan = scanPluginProgram(r.program, dir);
  assert.deepEqual(scan.publishNames, []);
  assert.deepEqual(scan.useNames, ["@demo/greeter"]);
  assert.deepEqual(scan.dynamicPublishSites, []);
});

test("publishes auto-derives 'self' when code publishes exactly the package name", async () => {
  const out = await buildPlugin(fx("publisher-derived-self"), packagesDir);
  const manifest = JSON.parse(new AdmZip(out).readAsText("manifest.json"));
  assert.ok(manifest.publishes["@demo/derived-self"], "publishes derived from ctx.publish call");
  assert.equal(manifest.publishes["@demo/derived-self"].version, "1.2.0");
  assert.match(manifest.publishes["@demo/derived-self"].typesSha256, /^[0-9a-f]{64}$/);
});

test("authored publishes that disagrees with code is a build error (drift)", async () => {
  await assert.rejects(
    () => buildPlugin(fx("publisher-drift"), packagesDir),
    /publishes drift/,
  );
});

test("a non-literal ctx.publish name is a build error", async () => {
  await assert.rejects(
    () => buildPlugin(fx("publisher-dynamic"), packagesDir),
    /string literal/,
  );
});
```

- [ ] **Step 3: Run to verify they fail**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/publish-scan.test.mjs`
Expected: FAIL — `../src/publish-scan.ts` not found; `r.program` undefined.

- [ ] **Step 4: Return the program from `typecheckPlugin`**

In `packages/sdk/src/typecheck/typecheck.ts`:

```ts
export interface TypecheckResult { ok: boolean; diagnostics: TypecheckDiag[]; program?: ts.Program; }
```

and change the return:

```ts
    return { ok: out.length === 0, diagnostics: out, program };
```

(The temp ambient-stub dir is still `rmSync`'d in the `finally` — the program has already parsed those sources into memory; later `scanPluginProgram`/lint walks never re-read the deleted stub file.)

- [ ] **Step 5: Create `packages/sdk/src/publish-scan.ts`**

```ts
/**
 * B1 (north-star §5.2): derive the manifest `publishes` name-set — and the dependency-usage
 * advisories — from CODE, off the tsc gate's own program. Receiver-typed matching (the object
 * before `.publish` / `.use` / `.tryUse` must be the SDK's `PluginContext`) keeps this exact
 * under renaming (`plugin((c) => c.publish(...))`) and immune to unrelated `.publish` methods.
 */

import ts from "typescript";

export interface PublishScan {
  /** String-literal names from `ctx.publish("name", …)`, deduped, source order. */
  publishNames: string[];
  /** `file:line` of every ctx.publish whose first arg is NOT a string literal (kills derivation). */
  dynamicPublishSites: string[];
  /** String-literal names from `ctx.use("name")` / `ctx.tryUse("name")`, deduped. */
  useNames: string[];
}

/** True when `type`'s symbol (or alias) is the SDK PluginContext. */
function isPluginContext(type: ts.Type): boolean {
  const sym = type.getSymbol() ?? type.aliasSymbol;
  return sym?.getName() === "PluginContext";
}

export function scanPluginProgram(program: ts.Program, pluginDir: string): PublishScan {
  const checker = program.getTypeChecker();
  const out: PublishScan = { publishNames: [], dynamicPublishSites: [], useNames: [] };
  const dirPrefix = pluginDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";

  for (const sf of program.getSourceFiles()) {
    if (sf.isDeclarationFile) continue;
    if (!sf.fileName.replace(/\\/g, "/").startsWith(dirPrefix)) continue;

    const visit = (node: ts.Node): void => {
      if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)) {
        const method = node.expression.name.text;
        if (method === "publish" || method === "use" || method === "tryUse") {
          const recv = checker.getTypeAtLocation(node.expression.expression);
          if (isPluginContext(recv)) {
            const arg0 = node.arguments[0];
            if (method === "publish") {
              if (arg0 !== undefined && ts.isStringLiteralLike(arg0)) {
                out.publishNames.push(arg0.text);
              } else {
                const { line } = sf.getLineAndCharacterOfPosition(node.getStart());
                out.dynamicPublishSites.push(`${sf.fileName}:${line + 1}`);
              }
            } else if (arg0 !== undefined && ts.isStringLiteralLike(arg0)) {
              out.useNames.push(arg0.text);
            }
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }

  out.publishNames = [...new Set(out.publishNames)];
  out.useNames = [...new Set(out.useNames)];
  return out;
}
```

- [ ] **Step 6: Reorder + reconcile in `packages/sdk/src/build.ts`**

Add imports:

```ts
import { expandPublishes } from "./publishes.ts";
import { hasPublishes } from "./publish-gate.ts";
import { scanPluginProgram } from "./publish-scan.ts";
```

(`derivePublishes` and `assertPublishesTypes` imports stay.)

Restructure the top of `buildPlugin` — the derivation now needs the program, so the typecheck runs FIRST and the cheap config validation stays before it as the only pre-check. Replace everything from the `// --- publishes ⇒ types gate` comment through the `derivedPublishes` line AND the existing typecheck block with:

```ts
  const s2 = pkg.s2script ?? {};

  // --- Cheap fail-fast: config block shape (no program needed). ---
  const config = s2.config ?? undefined;
  if (config !== undefined) {
    const cfgErrs = validateConfigBlock(config);
    if (cfgErrs.length) throw new Error(`invalid s2script.config:\n  ${cfgErrs.join("\n  ")}`);
  }

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on
  //     error. Runs FIRST now: the program it builds feeds the publishes/use derivation (B1)
  //     and the lint gate (B2). ---
  const tc = typecheckPlugin(absDir, packagesDir !== undefined ? { packagesDir } : undefined);
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }
  const scan = scanPluginProgram(tc.program!, absDir);

  // --- publishes: reconciliation IS generation (north-star §5.2). The name-set comes from code;
  //     "self" is auto-derived; an authored block must agree exactly; dynamic names are refused. ---
  if (scan.dynamicPublishSites.length > 0) {
    throw new Error(
      `ctx.publish name must be a string literal (the manifest publishes block is derived from code):\n  ` +
        scan.dynamicPublishSites.join("\n  "),
    );
  }
  let effectivePublishes = s2.publishes;
  if (!hasPublishes(effectivePublishes)) {
    if (scan.publishNames.length === 1 && scan.publishNames[0] === pkg.name) {
      effectivePublishes = "self"; // generated: code publishes exactly this package's own contract
    } else if (scan.publishNames.length > 0) {
      throw new Error(
        `code publishes ${JSON.stringify(scan.publishNames)} but s2script.publishes is missing — ` +
          `a contract named differently from the package needs an authored entry with a concrete version`,
      );
    }
  } else {
    const authoredNames = Object.keys(expandPublishes(effectivePublishes, pkg.name, pkg.version)).sort();
    const codeNames = [...scan.publishNames].sort();
    if (JSON.stringify(authoredNames) !== JSON.stringify(codeNames)) {
      throw new Error(
        `publishes drift: package.json declares ${JSON.stringify(authoredNames)} but the code's ` +
          `ctx.publish calls are ${JSON.stringify(codeNames)} — fix whichever is wrong (the manifest ` +
          `is generated from code; the loader re-verifies at Active)`,
      );
    }
  }

  // --- publishes ⇒ types gate + hash (unchanged mechanics, now fed the EFFECTIVE block). ---
  const gate = assertPublishesTypes({ ...pkg, s2script: { ...s2, publishes: effectivePublishes as never } }, absDir);
  if (!gate.ok) {
    throw new Error(`publish gate failed: ${gate.error}`);
  }
  const derivedPublishes = derivePublishes(
    effectivePublishes as never, pkg.name, pkg.version, gate.typesPath,
  );

  // --- Dependency advisories (lint-grade, WARN not error — spec §5.2 table, last row). ---
  const pluginDependencies = s2.pluginDependencies ?? {};
  const optionalPluginDependencies = s2.optionalPluginDependencies ?? {};
  const declaredDeps = new Set([
    ...Object.keys(pluginDependencies),
    ...Object.keys(optionalPluginDependencies),
  ]);
  for (const used of scan.useNames) {
    if (!declaredDeps.has(used)) {
      console.warn(
        `WARN: ctx.use/tryUse(${JSON.stringify(used)}) is not declared under s2script.pluginDependencies/` +
          `optionalPluginDependencies — it will throw at runtime`,
      );
    }
  }
  for (const dep of declaredDeps) {
    if (!scan.useNames.includes(dep)) {
      console.warn(`WARN: dependency ${JSON.stringify(dep)} is declared but never ctx.use()d`);
    }
  }
```

Then delete the now-duplicated lines further down (`const apiVersion…` stays from Task 1; the OLD `const pluginDependencies = …`/`optionalPluginDependencies`/`config` + `validateConfigBlock` lines are removed — they moved up). The `external` computation, esbuild call, manifest assembly (including Task 1's stamp + Task 4's `compiledAgainst`), embedded-types copy, and zip write are unchanged. **Type note:** `s2.publishes` is typed `string | Record<string, string>` in `PluginPackageJson` — `effectivePublishes` keeps that union; the two `as never` casts above satisfy the narrower `assertPublishesTypes`/`derivePublishes` parameter types without loosening them (or widen those params to `PublishesAuthored` — either is fine, pick one and keep both call sites consistent).

- [ ] **Step 7: Run tests, fix pre-existing publisher fixtures**

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/publish-scan.test.mjs test/publishes.test.mjs test/publish-gate.test.mjs test/build.test.mjs test/compiled-against.test.mjs`
Expected: the four new tests PASS. Any pre-existing publisher fixture whose `src/plugin.ts` does not `ctx.publish` exactly its authored names now fails with `publishes drift` — fix the FIXTURE (make its code publish the declared name via `ctx.publish("<name>", {...})`); do not weaken the check. Then run the whole suite: `npm test` → only the 13 known failures.

- [ ] **Step 8: Rebuild CLI + full typecheck gate + base plugins**

```bash
cd packages/sdk && node build.mjs && cd ../..
./scripts/check-plugins-typecheck.sh
./scripts/build-base-plugins.sh
```

Expected: gate passes; every base plugin builds. `plugins/zones` (authored `"publishes": "self"`, code `ctx.publish("@s2script/zones", …)`) must build clean — if it drifts, the zones plugin code/manifest is the bug, fix it there.

- [ ] **Step 9: Commit**

```bash
git add packages/sdk/src/publish-scan.ts packages/sdk/src/typecheck/typecheck.ts packages/sdk/src/build.ts \
        packages/sdk/test/publish-scan.test.mjs packages/sdk/test/fixtures
git commit -m "sdk: derive publishes name-set from ctx.publish calls; dep-use advisories (B1)

Receiver-typed scan of the tsc gate's own program: 'self' auto-derives, an
authored block must reconcile exactly (drift = build error), dynamic publish
names are refused, and declared-vs-used dependency mismatches WARN. typecheck
now returns its ts.Program (reused by the B2 lint gate)."
```

---

## B2 — `eslint-plugin-s2script`

### Task 6: `packages/eslint-plugin` package + `no-ctx-escape` (syntactic rule)

**Files:**
- Create: `packages/eslint-plugin/package.json`, `packages/eslint-plugin/build.mjs`
- Create: `packages/eslint-plugin/src/index.ts`, `packages/eslint-plugin/src/plugin-factory.ts`, `packages/eslint-plugin/src/rules/no-ctx-escape.ts`
- Create: `packages/eslint-plugin/test/no-ctx-escape.test.mjs`
- Modify: `package-lock.json` (via `npm install` at root — ADDITIVE only)

**Interfaces:**
- Consumes: nothing from other tasks (fresh package; root `workspaces: ["packages/*"]` already covers it).
- Produces:
  - npm package **`@s2script/eslint-plugin`** at `packages/eslint-plugin`, `type: module`, default export shape `{ meta, rules, configs }` (flat-config plugin object). Rules namespace prefix in configs: **`s2script/`**.
  - `export function findFactory(ast: TSESTree.Program): TSESTree.ArrowFunctionExpression | TSESTree.FunctionExpression | null` from `src/plugin-factory.ts` — **Tasks 7's rules reuse this** (locates the factory passed to `plugin()` imported from `@s2script/sdk/plugin`).
  - Rule id `s2script/no-ctx-escape`, messageId `escaped`.

- [ ] **Step 1: Verify the eslint/typescript-eslint version pair, then scaffold the package**

```bash
npm view eslint version && npm view typescript-eslint version && npm view @typescript-eslint/rule-tester version
```

Expected (as of plan-writing): `10.7.0` / `8.65.0` / `8.65.0`. If `@typescript-eslint/*@^8.65` does not declare peer support for the current eslint major, drop the eslint devDependency below to `^9.39.0` everywhere it appears in this plan (parser/utils 8.x are certified for ESLint v9) — the choice must be ONE pair used identically in Tasks 6-10.

`packages/eslint-plugin/package.json`:

```json
{
  "name": "@s2script/eslint-plugin",
  "version": "0.1.0",
  "description": "s2script residual safety rules — the small, local checks the type system cannot carry (no-ctx-escape, no-floating-promise-in-factory, no-bigint-in-interface-payloads, no-await-in-raw-view). Pinned by @s2script/sdk so the SAME engine+rules run in the editor and inside `s2s build`.",
  "type": "module",
  "main": "dist/index.js",
  "exports": {
    ".": "./dist/index.js",
    "./package.json": "./package.json"
  },
  "scripts": {
    "prepare": "node build.mjs",
    "build": "node build.mjs",
    "test": "node --experimental-strip-types --no-warnings --test test/*.test.mjs"
  },
  "dependencies": {
    "@typescript-eslint/parser": "^8.65.0",
    "@typescript-eslint/utils": "^8.65.0"
  },
  "peerDependencies": {
    "eslint": ">=9",
    "typescript": ">=5.6"
  },
  "devDependencies": {
    "@typescript-eslint/rule-tester": "^8.65.0",
    "eslint": "^10.7.0",
    "typescript": "^5.6.0"
  },
  "publishConfig": {
    "access": "public"
  },
  "files": [
    "dist",
    "README.md"
  ],
  "repository": {
    "type": "git",
    "url": "https://github.com/GabeHirakawa/s2script.git"
  }
}
```

`packages/eslint-plugin/build.mjs`:

```js
// build.mjs — esbuild driver for @s2script/eslint-plugin.
// Bundles src/index.ts → dist/index.js (ESM). The parser/utils/eslint/typescript stay external:
// they must be the SINGLE shared instances the host (editor extension or s2s build) resolves.
import * as esbuild from "esbuild";
import { mkdirSync } from "fs";

mkdirSync("dist", { recursive: true });

await esbuild.build({
  entryPoints: ["src/index.ts"],
  bundle: true,
  platform: "node",
  format: "esm",
  outfile: "dist/index.js",
  external: ["eslint", "typescript", "@typescript-eslint/*"],
  target: "node22",
});

console.log("built dist/index.js");
```

Install (root, workspaces): `npm install` then `git diff --stat package-lock.json` — additions only (eslint, typescript-eslint toolchain, the new workspace stub). **Never** regenerate the lockfile from scratch.

- [ ] **Step 2: Write the failing RuleTester test**

`packages/eslint-plugin/test/no-ctx-escape.test.mjs`:

```js
import { test } from "node:test";
import { RuleTester } from "eslint";
import tsParser from "@typescript-eslint/parser";
import { noCtxEscape } from "../src/rules/no-ctx-escape.ts";

const ruleTester = new RuleTester({
  languageOptions: { parser: tsParser, ecmaVersion: 2022, sourceType: "module" },
});

test("no-ctx-escape", () => {
  ruleTester.run("no-ctx-escape", noCtxEscape, {
    valid: [
      // Direct load-window use + a Scope driven later: the sanctioned patterns.
      `import { plugin } from "@s2script/sdk/plugin";
       export default plugin((ctx) => {
         ctx.events.on("player_death", () => {});
         const scope = ctx.createScope();
         ctx.commands.register("edit", () => { scope.clear(); });
       });`,
      // Not a plugin entry at all (no plugin() default export) — rule is inert.
      `const ctx = { events: { on() {} } };
       export function helper() { ctx.events.on("x", () => {}); }`,
      // Async factory: ctx used after await but still in the factory body — legal (load window
      // = the whole factory run).
      `import { plugin } from "@s2script/sdk/plugin";
       export default plugin(async (ctx) => {
         await Promise.resolve();
         ctx.events.on("round_start", () => {});
       });`,
    ],
    invalid: [
      {
        code: `import { plugin } from "@s2script/sdk/plugin";
               export default plugin((ctx) => {
                 ctx.commands.register("late", () => {
                   ctx.events.on("player_death", () => {});
                 });
               });`,
        errors: [{ messageId: "escaped" }],
      },
      {
        // Destructured members of the ctx param are just as load-window-only.
        code: `import { plugin } from "@s2script/sdk/plugin";
               export default plugin(({ events, commands }) => {
                 commands.register("late", () => {
                   events.on("player_death", () => {});
                 });
               });`,
        errors: [{ messageId: "escaped" }],
      },
      {
        // Captured in a returned hook — runs at unload, long after the seal.
        code: `import { plugin } from "@s2script/sdk/plugin";
               export default plugin((ctx) => {
                 return { onUnload() { ctx.events.on("x", () => {}); } };
               });`,
        errors: [{ messageId: "escaped" }],
      },
    ],
  });
});
```

- [ ] **Step 3: Run to verify it fails**

Run: `cd packages/eslint-plugin && npm test`
Expected: FAIL — `../src/rules/no-ctx-escape.ts` not found.

- [ ] **Step 4: Implement `src/plugin-factory.ts`**

```ts
/**
 * Shared factory locator: finds the function passed to `plugin(...)` in
 * `export default plugin(<factory>)`, where `plugin` was imported from "@s2script/sdk/plugin".
 * Import-source matching (not scope analysis) keeps it dependency-light; shadowing `plugin`
 * between the import and the export is not a pattern worth chasing.
 */
import type { TSESTree } from "@typescript-eslint/utils";

export type FactoryNode = TSESTree.ArrowFunctionExpression | TSESTree.FunctionExpression;

export function findFactory(ast: TSESTree.Program): FactoryNode | null {
  let pluginLocal: string | null = null;
  for (const stmt of ast.body) {
    if (stmt.type === "ImportDeclaration" && stmt.source.value === "@s2script/sdk/plugin") {
      for (const spec of stmt.specifiers) {
        if (
          spec.type === "ImportSpecifier" &&
          spec.imported.type === "Identifier" &&
          spec.imported.name === "plugin"
        ) {
          pluginLocal = spec.local.name;
        }
      }
    }
  }
  if (pluginLocal === null) return null;

  for (const stmt of ast.body) {
    if (stmt.type !== "ExportDefaultDeclaration") continue;
    const d = stmt.declaration;
    if (d.type === "CallExpression" && d.callee.type === "Identifier" && d.callee.name === pluginLocal) {
      const a = d.arguments[0];
      if (a !== undefined && (a.type === "ArrowFunctionExpression" || a.type === "FunctionExpression")) {
        return a;
      }
    }
  }
  return null;
}

/** True for any function-ish AST node (the nesting boundary the rules care about). */
export function isFunctionNode(
  n: TSESTree.Node,
): n is TSESTree.ArrowFunctionExpression | TSESTree.FunctionExpression | TSESTree.FunctionDeclaration {
  return (
    n.type === "ArrowFunctionExpression" ||
    n.type === "FunctionExpression" ||
    n.type === "FunctionDeclaration"
  );
}
```

- [ ] **Step 5: Implement `src/rules/no-ctx-escape.ts`**

```ts
/**
 * no-ctx-escape — THE one escape the type system cannot catch (L1 design §4.2/B2 §5.3): the
 * factory's `ctx` (or a member destructured from it) captured inside a nested function. Such a
 * reference runs after the plugin reaches Active, when the ctx is sealed — at runtime it throws
 * "registration outside the load window"; this rule makes it a red squiggle instead.
 * Scope handles from ctx.createScope() are intentionally NOT flagged (late driving is their job).
 */
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";
import { findFactory, isFunctionNode, type FactoryNode } from "../plugin-factory.ts";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/GabeHirakawa/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

/** Every binding name introduced by the factory's first parameter pattern. */
function param0Names(param: TSESTree.Parameter): Set<string> {
  const names = new Set<string>();
  const collect = (p: TSESTree.Node): void => {
    switch (p.type) {
      case "Identifier": names.add(p.name); break;
      case "ObjectPattern":
        for (const prop of p.properties) collect(prop.type === "Property" ? prop.value : prop.argument);
        break;
      case "ArrayPattern":
        for (const el of p.elements) if (el !== null) collect(el);
        break;
      case "AssignmentPattern": collect(p.left); break;
      case "RestElement": collect(p.argument); break;
      default: break;
    }
  };
  collect(param);
  return names;
}

export const noCtxEscape = createRule({
  name: "no-ctx-escape",
  meta: {
    type: "problem",
    docs: {
      description:
        "the plugin factory's ctx (and members destructured from it) is load-window-only; referencing it inside a nested function defers the use past the seal",
    },
    messages: {
      escaped:
        "'{{name}}' escapes the load window: it is referenced inside a nested function, which runs after the plugin is Active and the ctx is sealed (the registration will throw). Register during the factory run, or allocate a Scope with ctx.createScope() at load and drive that instead.",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const factory: FactoryNode | null = findFactory(context.sourceCode.ast);
    if (factory === null || factory.params.length === 0) return {};
    const names = param0Names(factory.params[0]);

    // The ctx bindings are declared BY the factory node itself.
    const ctxVars = context.sourceCode
      .getDeclaredVariables(factory)
      .filter((v) => names.has(v.name));

    return {
      "Program:exit"() {
        for (const v of ctxVars) {
          for (const ref of v.references) {
            // Innermost enclosing function of the reference.
            let n: TSESTree.Node | undefined = ref.identifier.parent;
            while (n !== undefined && n !== factory && !isFunctionNode(n)) n = n.parent;
            if (n !== undefined && n !== factory) {
              context.report({
                node: ref.identifier,
                messageId: "escaped",
                data: { name: v.name },
              });
            }
          }
        }
      },
    };
  },
});
```

- [ ] **Step 6: Implement `src/index.ts` (rules only — configs arrive in Task 9)**

```ts
/**
 * @s2script/eslint-plugin — the s2script residual rule set (north-star §5.3). Flat-config plugin
 * object; `configs` is populated by configs.ts (recommended: editor/projectService; build:
 * s2s-build/provided-program). One implementation, two consumers, zero drift.
 */
import { noCtxEscape } from "./rules/no-ctx-escape.ts";

const plugin = {
  meta: { name: "@s2script/eslint-plugin", version: "0.1.0" },
  rules: {
    "no-ctx-escape": noCtxEscape,
  } as Record<string, unknown>,
  configs: {} as {
    recommended?: (opts?: { tsconfigRootDir?: string }) => unknown[];
    build?: (programs: unknown[]) => unknown[];
  },
};

export default plugin;
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd packages/eslint-plugin && npm test`
Expected: PASS (`no-ctx-escape` — all valid/invalid cases). Also confirm the bundle builds: `node build.mjs` → `built dist/index.js`.

- [ ] **Step 8: Commit**

```bash
git add packages/eslint-plugin package-lock.json package.json
git commit -m "eslint-plugin: new @s2script/eslint-plugin tooling package + no-ctx-escape (B2)

The residual-rule engine (locked decision #3: ESLint, not a LS plugin). Rule 1
flags the one escape L1's types cannot: ctx (or a destructured member) captured
in a nested function - sealed at runtime, now a red squiggle at authoring time.
Scope handles are deliberately exempt."
```

---

### Task 7: Typed rules — `no-floating-promise-in-factory` + `no-await-in-raw-view`

**Files:**
- Create: `packages/eslint-plugin/src/rules/no-floating-promise-in-factory.ts`
- Create: `packages/eslint-plugin/src/rules/no-await-in-raw-view.ts`
- Create: `packages/eslint-plugin/test/fixtures/tsconfig.json`, `test/fixtures/file.ts`, `test/fixtures/s2sdk-stubs.d.ts`
- Create: `packages/eslint-plugin/test/typed-rules.test.mjs`
- Modify: `packages/eslint-plugin/src/index.ts` (register both rules)

**Interfaces:**
- Consumes: `findFactory`/`isFunctionNode` from `src/plugin-factory.ts` (Task 6); type-name contracts frozen by L1: `PluginContext` (`packages/sdk/plugin.d.ts`), `UserCmdView` (`packages/sdk/usercmd.d.ts`).
- Produces: rule ids `s2script/no-floating-promise-in-factory` (messageId `floating`) and `s2script/no-await-in-raw-view` (messageId `rawViewInAsync`); the typed-rule test fixture harness (`test/fixtures/` + the RuleTester wiring) that **Task 8 reuses**.

- [ ] **Step 1: Create the typed-rule test fixtures**

`packages/eslint-plugin/test/fixtures/tsconfig.json`:

```json
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "module": "ESNext",
    "moduleResolution": "bundler",
    "target": "ES2020",
    "lib": ["ES2020"],
    "types": [],
    "skipLibCheck": true
  },
  "include": ["*.ts", "*.d.ts"]
}
```

`packages/eslint-plugin/test/fixtures/file.ts` — empty file (the rule-tester's default virtual filename):

```ts
// intentionally empty — @typescript-eslint/rule-tester injects test code as this file.
```

`packages/eslint-plugin/test/fixtures/s2sdk-stubs.d.ts` — symbol-name-faithful stubs (the typed rules match on symbol NAMES — `PluginContext`, `UserCmdView`, `InterfaceHandle`, `PublishHandle` — so the stubs only need those names + enough members for the test snippets; the real-`.d.ts` integration is covered by Task 9's build-level test):

```ts
declare module "@s2script/sdk/plugin" {
  import type { UserCmdView } from "@s2script/sdk/usercmd";
  import type { PublishHandle } from "@s2script/sdk/interfaces";
  export interface CtxEvents { on(name: string, h: (ev: unknown) => void): void; }
  export interface CtxClients {
    onRunCmd(h: (cmd: UserCmdView, info: { slot: number }) => number | void): void;
  }
  export interface CtxCommands { register(name: string, h: (cmd: unknown) => void): void; }
  export type InterfaceHandle<T extends object> = T & {
    on(event: string, handler: (payload: any) => void): void;
  };
  export interface Scope { clear(): void; dispose(): void; }
  export interface PluginContext {
    readonly events: CtxEvents;
    readonly clients: CtxClients;
    readonly commands: CtxCommands;
    publish<T extends object>(name: string, impl: T): PublishHandle;
    use<T extends object>(name: string): InterfaceHandle<T>;
    tryUse<T extends object>(name: string): InterfaceHandle<T> | null;
    createScope(): Scope;
  }
  export interface PluginDefinition { readonly __s2plugin: 1; }
  export function plugin(factory: (ctx: PluginContext) => unknown): PluginDefinition;
}
declare module "@s2script/sdk/usercmd" {
  export interface UserCmdView { buttons: bigint; forwardMove: number; }
}
declare module "@s2script/sdk/interfaces" {
  export interface PublishHandle { emit(event: string, payload: unknown): void; }
}
declare module "@s2script/sdk/db" {
  export const Database: {
    open(name: string): Promise<{ query(sql: string): Promise<unknown> }>;
  };
}
```

- [ ] **Step 2: Write the failing tests**

`packages/eslint-plugin/test/typed-rules.test.mjs`:

```js
import * as nodeTest from "node:test";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { RuleTester } from "@typescript-eslint/rule-tester";
import { noFloatingPromiseInFactory } from "../src/rules/no-floating-promise-in-factory.ts";
import { noAwaitInRawView } from "../src/rules/no-await-in-raw-view.ts";

// @typescript-eslint/rule-tester needs a test-framework hookup; wire it to node:test.
RuleTester.afterAll = nodeTest.after;
RuleTester.describe = nodeTest.describe;
RuleTester.it = nodeTest.it;
RuleTester.itOnly = nodeTest.it;

const fixtures = join(dirname(fileURLToPath(import.meta.url)), "fixtures");

const ruleTester = new RuleTester({
  languageOptions: {
    parserOptions: {
      project: "./tsconfig.json",
      tsconfigRootDir: fixtures,
    },
  },
});

ruleTester.run("no-floating-promise-in-factory", noFloatingPromiseInFactory, {
  valid: [
    // awaited — the load window covers it.
    `import { plugin } from "@s2script/sdk/plugin";
     import { Database } from "@s2script/sdk/db";
     export default plugin(async (ctx) => {
       const db = await Database.open("prefs");
       ctx.events.on("round_start", () => { void db.query("x"); });
     });`,
    // explicit void — the author opted out, visibly.
    `import { plugin } from "@s2script/sdk/plugin";
     import { Database } from "@s2script/sdk/db";
     export default plugin((ctx) => {
       void Database.open("prefs");
       ctx.events.on("round_start", () => {});
     });`,
    // floating promise inside a HANDLER is not this rule's business.
    `import { plugin } from "@s2script/sdk/plugin";
     import { Database } from "@s2script/sdk/db";
     export default plugin((ctx) => {
       ctx.events.on("round_start", () => { Database.open("prefs"); });
     });`,
  ],
  invalid: [
    {
      code: `import { plugin } from "@s2script/sdk/plugin";
             import { Database } from "@s2script/sdk/db";
             export default plugin(async (ctx) => {
               Database.open("prefs");
               ctx.events.on("round_start", () => {});
             });`,
      errors: [{ messageId: "floating" }],
    },
    {
      // .then() chains are still thenables when discarded as a statement.
      code: `import { plugin } from "@s2script/sdk/plugin";
             import { Database } from "@s2script/sdk/db";
             export default plugin((ctx) => {
               Database.open("prefs").then((db) => db.query("x"));
               ctx.events.on("round_start", () => {});
             });`,
      errors: [{ messageId: "floating" }],
    },
  ],
});

ruleTester.run("no-await-in-raw-view", noAwaitInRawView, {
  valid: [
    // Synchronous handler use — the only sound pattern.
    `import { plugin } from "@s2script/sdk/plugin";
     export default plugin((ctx) => {
       ctx.clients.onRunCmd((view) => {
         if (view.forwardMove > 0) return 1;
       });
     });`,
    // Copy-then-async: plain values may cross awaits freely.
    `import { plugin } from "@s2script/sdk/plugin";
     export default plugin((ctx) => {
       ctx.clients.onRunCmd((view) => {
         const buttons = String(view.buttons);
         void (async () => { await Promise.resolve(); console.log(buttons); })();
       });
     });`,
  ],
  invalid: [
    {
      // The view itself dragged into async code — dead after the tick.
      code: `import { plugin } from "@s2script/sdk/plugin";
             export default plugin((ctx) => {
               ctx.clients.onRunCmd((view) => {
                 void (async () => { await Promise.resolve(); console.log(view.forwardMove); })();
               });
             });`,
      errors: [{ messageId: "rawViewInAsync" }],
    },
    {
      // Async helper PARAMETER typed as the raw view: 2 reports (param use sites inside async fn).
      code: `import { plugin } from "@s2script/sdk/plugin";
             import type { UserCmdView } from "@s2script/sdk/usercmd";
             async function log(v: UserCmdView): Promise<void> {
               await Promise.resolve();
               console.log(v.forwardMove);
             }
             export default plugin((ctx) => {
               ctx.clients.onRunCmd((view) => { void log(view); });
             });`,
      errors: [{ messageId: "rawViewInAsync" }],
    },
  ],
});
```

- [ ] **Step 3: Run to verify they fail**

Run: `cd packages/eslint-plugin && npm test`
Expected: FAIL — rule modules not found.

- [ ] **Step 4: Implement `src/rules/no-floating-promise-in-factory.ts`**

```ts
/**
 * no-floating-promise-in-factory (north-star §5.3): the load window closes when the factory's
 * promise settles — an unawaited promise started in the factory races arm-at-Active (init not
 * done when handlers arm; a failure can't fail the load). Scoped to statements whose innermost
 * function IS the factory: handlers/helpers are outside this rule's contract.
 */
import ts from "typescript";
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";
import { findFactory, isFunctionNode, type FactoryNode } from "../plugin-factory.ts";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/GabeHirakawa/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

function isThenable(checker: ts.TypeChecker, type: ts.Type): boolean {
  const parts = type.isUnion() ? type.types : [type];
  for (const part of parts) {
    const then = part.getProperty("then");
    if (then === undefined) continue;
    const decl = then.valueDeclaration ?? then.declarations?.[0];
    if (decl === undefined) continue;
    if (checker.getTypeOfSymbolAtLocation(then, decl).getCallSignatures().length > 0) return true;
  }
  return false;
}

export const noFloatingPromiseInFactory = createRule({
  name: "no-floating-promise-in-factory",
  meta: {
    type: "problem",
    docs: {
      description:
        "a promise discarded inside the plugin factory races arm-at-Active; await it or void it explicitly",
    },
    messages: {
      floating:
        "floating promise in the plugin factory: the load window closes when the factory settles, so this async work is not covered by it — `await` it (or `void` it only if it genuinely must not gate the load).",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const factory: FactoryNode | null = findFactory(context.sourceCode.ast);
    if (factory === null) return {};
    const services = ESLintUtils.getParserServices(context);
    const checker = services.program.getTypeChecker();

    return {
      ExpressionStatement(node: TSESTree.ExpressionStatement) {
        // Innermost enclosing function must be the factory itself.
        let fn: TSESTree.Node | undefined = node.parent;
        while (fn !== undefined && fn !== factory && !isFunctionNode(fn)) fn = fn.parent;
        if (fn !== factory) return;

        const expr = node.expression;
        if (expr.type === "AwaitExpression" || expr.type === "AssignmentExpression") return;
        if (expr.type === "UnaryExpression" && expr.operator === "void") return;

        const tsNode = services.esTreeNodeToTSNodeMap.get(expr);
        if (isThenable(checker, checker.getTypeAtLocation(tsNode))) {
          context.report({ node, messageId: "floating" });
        }
      },
    };
  },
});
```

- [ ] **Step 5: Implement `src/rules/no-await-in-raw-view.ts`**

```ts
/**
 * no-await-in-raw-view (north-star §5.3; standing constraint: raw-live views are block-scoped and
 * cannot cross `await`). Precise "used after an await" dataflow is loop-hostile, so the rule
 * enforces the teachable superset: a raw-view-typed value may NEVER be referenced inside an
 * async function at all — copy the fields you need into plain values first. Symbol-name keyed
 * (RAW_VIEW_TYPES) so future views join with one line.
 */
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";
import { isFunctionNode } from "../plugin-factory.ts";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/GabeHirakawa/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

const RAW_VIEW_TYPES: ReadonlySet<string> = new Set(["UserCmdView"]);

export const noAwaitInRawView = createRule({
  name: "no-await-in-raw-view",
  meta: {
    type: "problem",
    docs: {
      description:
        "raw tick-scoped views (UserCmdView) must not enter async code — they are dead across any await",
    },
    messages: {
      rawViewInAsync:
        "a {{type}} is a tick-scoped raw view: inside an async function it can outlive its tick and read/write nothing (or garbage). Copy the fields you need into plain values BEFORE going async.",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const services = ESLintUtils.getParserServices(context);
    const checker = services.program.getTypeChecker();

    return {
      Identifier(node: TSESTree.Identifier) {
        // Innermost enclosing function must be async.
        let fn: TSESTree.Node | undefined = node.parent;
        while (fn !== undefined && !isFunctionNode(fn)) fn = fn.parent;
        if (fn === undefined || !fn.async) return;

        // Skip pure type positions (import type / annotations) — they carry no runtime value.
        if (node.parent?.type === "TSTypeReference" || node.parent?.type === "ImportSpecifier") return;

        const tsNode = services.esTreeNodeToTSNodeMap.get(node);
        const t = checker.getTypeAtLocation(tsNode);
        const sym = t.getSymbol() ?? t.aliasSymbol;
        const name = sym?.getName();
        if (name !== undefined && RAW_VIEW_TYPES.has(name)) {
          context.report({ node, messageId: "rawViewInAsync", data: { type: name } });
        }
      },
    };
  },
});
```

**Calibration note for the implementer:** the second invalid case expects exactly the reports the traversal produces for `v` (the async param's annotation identifier is a declaration, not a reference — but `v.forwardMove`'s `v` IS one; the param name identifier in `v: UserCmdView` may also type as `UserCmdView`). Run the test, count the actual reports, and set the `errors` array length to match reality (1 or 2) — then LOCK it with a comment. Never loosen the rule to make the count smaller; adjust the expectation.

- [ ] **Step 6: Register both rules in `src/index.ts`**

```ts
import { noCtxEscape } from "./rules/no-ctx-escape.ts";
import { noFloatingPromiseInFactory } from "./rules/no-floating-promise-in-factory.ts";
import { noAwaitInRawView } from "./rules/no-await-in-raw-view.ts";
```

and in the `rules` object:

```ts
  rules: {
    "no-ctx-escape": noCtxEscape,
    "no-floating-promise-in-factory": noFloatingPromiseInFactory,
    "no-await-in-raw-view": noAwaitInRawView,
  } as Record<string, unknown>,
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd packages/eslint-plugin && npm test`
Expected: PASS (all three test files' cases).

- [ ] **Step 8: Commit**

```bash
git add packages/eslint-plugin
git commit -m "eslint-plugin: typed rules no-floating-promise-in-factory + no-await-in-raw-view (B2)

Factory-scoped floating-promise detection (thenable-typed expression statements
whose innermost function is the plugin factory) and the raw-view async ban
(UserCmdView referenced anywhere inside an async function - the teachable
superset of 'cannot cross await'). Typed via parserServices; fixture harness
established for Task 8."
```

---

### Task 8: `no-bigint-in-interface-payloads`

**Files:**
- Create: `packages/eslint-plugin/src/rules/no-bigint-in-interface-payloads.ts`
- Create: `packages/eslint-plugin/test/no-bigint.test.mjs`
- Modify: `packages/eslint-plugin/src/index.ts` (register)

**Interfaces:**
- Consumes: Task 7's fixture harness (`test/fixtures/` + RuleTester wiring); symbol-name contracts `PublishHandle` (`packages/sdk/interfaces.d.ts`), `InterfaceHandle` (`packages/sdk/plugin.d.ts`), `PluginContext`.
- Produces: rule id `s2script/no-bigint-in-interface-payloads`, messageId `bigintPayload`. Completes the four-rule set Task 9's configs enumerate.

The footgun (memory: cross-context-marshalling-json, locked in Slice 5B.4): inter-plugin args, returns, and forward payloads cross as JSON — a `BigInt` anywhere in the value makes `JSON.stringify` throw and the WHOLE payload silently drops. Three wire crossings are statically visible: (a) `PublishHandle.emit(event, payload)` — producer forward; (b) any method call on an `InterfaceHandle<T>` — consumer→producer args; (c) the method properties of the impl object passed to `ctx.publish(name, impl)` — producer method returns. Detection is type-driven (does the crossing value's type contain `bigint`, walked depth-limited), so `view.buttons` (a `bigint` per usercmd.d.ts) is caught without annotation.

- [ ] **Step 1: Write the failing test**

`packages/eslint-plugin/test/no-bigint.test.mjs`:

```js
import * as nodeTest from "node:test";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { RuleTester } from "@typescript-eslint/rule-tester";
import { noBigintInInterfacePayloads } from "../src/rules/no-bigint-in-interface-payloads.ts";

RuleTester.afterAll = nodeTest.after;
RuleTester.describe = nodeTest.describe;
RuleTester.it = nodeTest.it;
RuleTester.itOnly = nodeTest.it;

const fixtures = join(dirname(fileURLToPath(import.meta.url)), "fixtures");

const ruleTester = new RuleTester({
  languageOptions: {
    parserOptions: {
      project: "./tsconfig.json",
      tsconfigRootDir: fixtures,
    },
  },
});

ruleTester.run("no-bigint-in-interface-payloads", noBigintInInterfacePayloads, {
  valid: [
    // Decimal-string carry — THE documented idiom for 64-bit.
    `import { plugin } from "@s2script/sdk/plugin";
     export default plugin((ctx) => {
       const h = ctx.publish("@demo/prod", { kills: () => 3 });
       ctx.clients.onRunCmd((view) => {
         h.emit("buttons", { mask: String(view.buttons) });
       });
     });`,
    // Plain-number payloads through an InterfaceHandle.
    `import { plugin } from "@s2script/sdk/plugin";
     interface Api { setScore(v: { score: number }): void; }
     export default plugin((ctx) => {
       const api = ctx.use<Api>("@demo/api");
       api.setScore({ score: 12 });
     });`,
  ],
  invalid: [
    {
      // (a) emit payload carrying a bigint property (the usercmd buttons trap).
      code: `import { plugin } from "@s2script/sdk/plugin";
             export default plugin((ctx) => {
               const h = ctx.publish("@demo/prod", { kills: () => 3 });
               ctx.clients.onRunCmd((view) => {
                 h.emit("buttons", { mask: view.buttons });
               });
             });`,
      errors: [{ messageId: "bigintPayload" }],
    },
    {
      // (b) InterfaceHandle method arg with a bigint literal.
      code: `import { plugin } from "@s2script/sdk/plugin";
             interface Api { setMask(v: unknown): void; }
             export default plugin((ctx) => {
               const api = ctx.use<Api>("@demo/api");
               api.setMask({ mask: 1n });
             });`,
      errors: [{ messageId: "bigintPayload" }],
    },
    {
      // (c) producer impl method RETURNING bigint — drops the consumer's whole call result.
      code: `import { plugin } from "@s2script/sdk/plugin";
             export default plugin((ctx) => {
               ctx.publish("@demo/prod", { mask: () => 1n });
             });`,
      errors: [{ messageId: "bigintPayload" }],
    },
  ],
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd packages/eslint-plugin && npm test`
Expected: FAIL — rule module not found.

- [ ] **Step 3: Implement `src/rules/no-bigint-in-interface-payloads.ts`**

```ts
/**
 * no-bigint-in-interface-payloads (north-star §5.3; Slice 5B.4 lock): inter-plugin values cross
 * as JSON — a BigInt anywhere throws inside the marshaller and the WHOLE payload silently drops.
 * Flags the three statically-visible wire crossings:
 *   (a) PublishHandle.emit(event, payload)          — forward payload
 *   (b) <InterfaceHandle>.method(args...)           — consumer -> producer args
 *   (c) ctx.publish(name, impl) method return types — producer -> consumer returns
 * Fix: carry 64-bit as a decimal string (String(v)).
 */
import ts from "typescript";
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/GabeHirakawa/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

function symbolName(type: ts.Type): string | undefined {
  return (type.aliasSymbol ?? type.getSymbol())?.getName();
}

function containsBigInt(checker: ts.TypeChecker, type: ts.Type, depth = 0): boolean {
  if (depth > 3) return false;
  if (type.flags & ts.TypeFlags.BigIntLike) return true;
  if (type.isUnionOrIntersection()) {
    return type.types.some((t) => containsBigInt(checker, t, depth + 1));
  }
  const numIndex = checker.getIndexTypeOfType(type, ts.IndexKind.Number); // arrays/tuples
  if (numIndex !== undefined && containsBigInt(checker, numIndex, depth + 1)) return true;
  if (type.getFlags() & ts.TypeFlags.Object) {
    for (const prop of type.getProperties()) {
      const decl = prop.valueDeclaration ?? prop.declarations?.[0];
      if (decl === undefined) continue;
      if (containsBigInt(checker, checker.getTypeOfSymbolAtLocation(prop, decl), depth + 1)) {
        return true;
      }
    }
  }
  return false;
}

export const noBigintInInterfacePayloads = createRule({
  name: "no-bigint-in-interface-payloads",
  meta: {
    type: "problem",
    docs: {
      description:
        "BigInt cannot cross the inter-plugin JSON wire — the whole payload is silently dropped; carry 64-bit as a decimal string",
    },
    messages: {
      bigintPayload:
        "BigInt cannot cross the inter-plugin wire: JSON marshalling throws and the WHOLE payload/call is silently dropped (Slice 5B.4). Carry 64-bit values as decimal strings — String(v) here, BigInt(s) on the far side.",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const services = ESLintUtils.getParserServices(context);
    const checker = services.program.getTypeChecker();

    const typeOf = (node: TSESTree.Node): ts.Type =>
      checker.getTypeAtLocation(services.esTreeNodeToTSNodeMap.get(node));

    return {
      CallExpression(node: TSESTree.CallExpression) {
        if (node.callee.type !== "MemberExpression" || node.callee.property.type !== "Identifier") {
          return;
        }
        const method = node.callee.property.name;
        const recvType = typeOf(node.callee.object);
        const recvName = symbolName(recvType);

        // (a) PublishHandle.emit(event, payload) — check the payload arg.
        if (recvName === "PublishHandle" && method === "emit") {
          const payload = node.arguments[1];
          if (payload !== undefined && containsBigInt(checker, typeOf(payload))) {
            context.report({ node: payload, messageId: "bigintPayload" });
          }
          return;
        }

        // (b) any method on an InterfaceHandle — check every argument.
        if (recvName === "InterfaceHandle" && method !== "on") {
          for (const arg of node.arguments) {
            if (containsBigInt(checker, typeOf(arg))) {
              context.report({ node: arg, messageId: "bigintPayload" });
            }
          }
          return;
        }

        // (c) ctx.publish(name, impl) — check each impl method's RETURN type.
        if (recvName === "PluginContext" && method === "publish") {
          const impl = node.arguments[1];
          if (impl === undefined) return;
          const implType = typeOf(impl);
          for (const prop of implType.getProperties()) {
            const decl = prop.valueDeclaration ?? prop.declarations?.[0];
            if (decl === undefined) continue;
            const propType = checker.getTypeOfSymbolAtLocation(prop, decl);
            for (const sig of propType.getCallSignatures()) {
              if (containsBigInt(checker, sig.getReturnType())) {
                context.report({ node: impl, messageId: "bigintPayload" });
                return;
              }
            }
          }
        }
      },
    };
  },
});
```

**Calibration note:** `InterfaceHandle` is a type ALIAS (`T & {on}`) — `aliasSymbol` carries the name when the value is typed straight off `ctx.use<T>()`; `symbolName` checks the alias first for exactly this reason. If the (b) case misses because the alias evaporates through the intersection, fall back to detecting via the receiver's ORIGIN (`ctx.use`/`tryUse` call in the variable's initializer) — but try the alias route first and lock whichever works with the test.

- [ ] **Step 4: Register in `src/index.ts`**

```ts
import { noBigintInInterfacePayloads } from "./rules/no-bigint-in-interface-payloads.ts";
```

```ts
  rules: {
    "no-ctx-escape": noCtxEscape,
    "no-floating-promise-in-factory": noFloatingPromiseInFactory,
    "no-bigint-in-interface-payloads": noBigintInInterfacePayloads,
    "no-await-in-raw-view": noAwaitInRawView,
  } as Record<string, unknown>,
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd packages/eslint-plugin && npm test`
Expected: PASS — all four rules' suites green.

- [ ] **Step 6: Commit**

```bash
git add packages/eslint-plugin
git commit -m "eslint-plugin: no-bigint-in-interface-payloads (B2) - the silent-drop footgun

Type-driven detection of BigInt at the three visible inter-plugin wire
crossings: PublishHandle.emit payloads, InterfaceHandle method args, and
ctx.publish impl method returns. Message teaches the decimal-string idiom."
```

---

### Task 9: Configs + `s2s build` runs the rules in-process (after the tsc gate)

**Files:**
- Create: `packages/eslint-plugin/src/configs.ts`
- Modify: `packages/eslint-plugin/src/index.ts` (wire `configs`)
- Create: `packages/sdk/src/lint/lint.ts`
- Modify: `packages/sdk/src/build.ts` (lint step between the scan and esbuild)
- Modify: `packages/sdk/package.json` (deps: `eslint`, `@s2script/eslint-plugin`) + `packages/sdk/build.mjs` (externals)
- Create: `packages/sdk/test/lint.test.mjs` + fixtures `packages/sdk/test/fixtures/lint-violation/`
- Modify: `package-lock.json` (additive, root `npm install`)

**Interfaces:**
- Consumes: Task 5's `TypecheckResult.program` (`ts.Program` built by the gate — identical resolution to the typecheck); Tasks 6-8's four rules + the plugin default export.
- Produces:
  - `plugin.configs.recommended(opts?: { tsconfigRootDir?: string }): unknown[]` — flat-config array for EDITOR use (projectService; Task 10 scaffolds it).
  - `plugin.configs.build(programs: unknown[]): unknown[]` — flat-config array for the in-process build (provided-program parsing, no tsconfig needed).
  - `export async function lintPlugin(pluginDir: string, program: ts.Program): Promise<LintResult>` with `interface LintResult { ok: boolean; output: string; errorCount: number }` from `packages/sdk/src/lint/lint.ts`.
  - Build behavior: **lint errors abort `s2s build` (no .s2sp)**; a plugin's own `eslint.config.{js,mjs,cjs,ts}` wins (editor/build parity), else the canonical config runs.

- [ ] **Step 1: Implement `packages/eslint-plugin/src/configs.ts`**

```ts
/**
 * The two faces of ONE rule set (north-star §5.3 — parity by construction):
 *  - recommended(): editor flat config (tsserver-independent; projectService reads the plugin's
 *    own tsconfig.json — the same file the tsc gate's semantics are mirrored into).
 *  - build(programs): what `s2s build` runs in-process, parsing against the ALREADY-BUILT
 *    typecheck-gate program — byte-identical module resolution to the tsc gate, zero extra
 *    program construction, works for in-repo plugins with no eslint.config of their own.
 * Same rules, same severities, same parser — only the type-info source differs.
 */
import tsParser from "@typescript-eslint/parser";

const RULES = {
  "s2script/no-ctx-escape": "error",
  "s2script/no-floating-promise-in-factory": "error",
  "s2script/no-bigint-in-interface-payloads": "error",
  "s2script/no-await-in-raw-view": "error",
} as const;

const IGNORES = { ignores: ["dist/**", "node_modules/**"] };

export function recommended(plugin: unknown, opts?: { tsconfigRootDir?: string }): unknown[] {
  return [
    IGNORES,
    {
      files: ["**/*.ts"],
      languageOptions: {
        parser: tsParser,
        parserOptions: {
          projectService: true,
          ...(opts?.tsconfigRootDir !== undefined ? { tsconfigRootDir: opts.tsconfigRootDir } : {}),
        },
      },
      plugins: { s2script: plugin },
      rules: RULES,
    },
  ];
}

export function buildConfig(plugin: unknown, programs: unknown[]): unknown[] {
  return [
    IGNORES,
    {
      files: ["**/*.ts"],
      languageOptions: {
        parser: tsParser,
        parserOptions: { programs },
      },
      plugins: { s2script: plugin },
      rules: RULES,
    },
  ];
}
```

Wire into `src/index.ts` (replace the empty `configs` initialization — note the two-step wiring because the config closes over the plugin object itself):

```ts
import { recommended, buildConfig } from "./configs.ts";
```

```ts
plugin.configs = {
  recommended: (opts?: { tsconfigRootDir?: string }) => recommended(plugin, opts),
  build: (programs: unknown[]) => buildConfig(plugin, programs),
};

export default plugin;
```

(Adjust the `plugin` literal so `configs` is assigned after the object exists — declare `const plugin = { meta, rules, configs: {} as ... };` then the assignment above, exactly as scaffolded in Task 6.)

Rebuild: `cd packages/eslint-plugin && node build.mjs` → `built dist/index.js`.

- [ ] **Step 2: Add the SDK dependencies + externals**

`packages/sdk/package.json` `dependencies` becomes:

```json
  "dependencies": {
    "esbuild": "^0.25.0",
    "adm-zip": "^0.5.16",
    "typescript": "^5.6.0",
    "eslint": "^10.7.0",
    "@s2script/eslint-plugin": "0.1.0"
  },
```

(**exact pin** on `@s2script/eslint-plugin` — "pinned by the SDK" is the parity guarantee; changesets will bump both together. Use the eslint range chosen in Task 6 Step 1.)

`packages/sdk/build.mjs` external list becomes:

```js
  external: ["esbuild", "adm-zip", "typescript", "eslint", "@s2script/eslint-plugin", "@typescript-eslint/*"],
```

Root: `npm install` (workspace-links the plugin; lockfile additions only — verify with `git diff --stat package-lock.json`).

- [ ] **Step 3: Create the failing fixture + test**

`packages/sdk/test/fixtures/lint-violation/package.json`:

```json
{
  "name": "@demo/lint-violation",
  "version": "0.1.0",
  "main": "src/plugin.ts"
}
```

`packages/sdk/test/fixtures/lint-violation/src/plugin.ts` (typechecks CLEAN — the violation is lint-only, proving the lint gate catches what tsc cannot):

```ts
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("late", (cmd) => {
    ctx.events.on("player_death", () => {
      cmd.reply("someone died");
    });
  });
});
```

`packages/sdk/test/lint.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { lintPlugin } from "../src/lint/lint.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

test("lintPlugin flags a ctx escape against the REAL sdk .d.ts (no stub drift)", async () => {
  const dir = fx("lint-violation");
  const tc = typecheckPlugin(dir, { packagesDir });
  assert.ok(tc.ok, "fixture must typecheck — the violation is lint-only");
  const r = await lintPlugin(dir, tc.program);
  assert.equal(r.ok, false);
  assert.match(r.output, /no-ctx-escape/);
});

test("s2s build refuses a lint violation (no .s2sp), passes a clean plugin", async () => {
  await assert.rejects(
    () => buildPlugin(fx("lint-violation"), packagesDir),
    /lint failed[\s\S]*no-ctx-escape/,
  );
  // The hello fixture is clean: build (which now lints) must still succeed.
  const out = await buildPlugin(fx("hello"), packagesDir);
  assert.ok(out.endsWith(".s2sp"));
});
```

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/lint.test.mjs`
Expected: FAIL — `../src/lint/lint.ts` not found.

- [ ] **Step 4: Implement `packages/sdk/src/lint/lint.ts`**

```ts
/**
 * B2 (north-star §5.3): run the pinned @s2script/eslint-plugin rules in-process, AFTER the tsc
 * gate — the same engine + rule versions the editor runs. A plugin's own eslint.config.* wins
 * (editor/build parity: what the author's editor shows is what the build enforces); otherwise
 * the canonical config runs against the typecheck gate's ALREADY-BUILT ts.Program, giving the
 * lint byte-identical module resolution to the gate with no tsconfig/node_modules dependence.
 */
import { ESLint } from "eslint";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import type ts from "typescript";
import s2lint from "@s2script/eslint-plugin";

export interface LintResult { ok: boolean; output: string; errorCount: number; }

const CONFIG_FILES = ["eslint.config.js", "eslint.config.mjs", "eslint.config.cjs", "eslint.config.ts"];

export async function lintPlugin(pluginDir: string, program: ts.Program): Promise<LintResult> {
  const absDir = resolve(pluginDir);
  const hasOwnConfig = CONFIG_FILES.some((f) => existsSync(join(absDir, f)));

  const eslint = hasOwnConfig
    ? new ESLint({ cwd: absDir, errorOnUnmatchedPattern: false })
    : new ESLint({
        cwd: absDir,
        overrideConfigFile: true,
        overrideConfig: s2lint.configs.build!([program]) as never,
        errorOnUnmatchedPattern: false,
      });

  // Canonical path: lint exactly the program's own in-dir sources (provided-program parsing
  // rejects files outside the program). Own-config path: the project's config governs.
  const dirPrefix = absDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";
  const targets = hasOwnConfig
    ? ["**/*.ts"]
    : program
        .getSourceFiles()
        .filter((sf) => !sf.isDeclarationFile && sf.fileName.replace(/\\/g, "/").startsWith(dirPrefix))
        .map((sf) => sf.fileName);

  if (targets.length === 0) return { ok: true, output: "", errorCount: 0 };

  const results = await eslint.lintFiles(targets);
  const errorCount = results.reduce((n, r) => n + r.errorCount, 0);
  const formatter = await eslint.loadFormatter("stylish");
  const output = String(await formatter.format(results));
  return { ok: errorCount === 0, output, errorCount };
}
```

- [ ] **Step 5: Hook into `packages/sdk/src/build.ts`**

Add the import:

```ts
import { lintPlugin } from "./lint/lint.ts";
```

Directly after the `const scan = scanPluginProgram(tc.program!, absDir);` line (i.e. tsc gate passed, program in hand — BEFORE the publishes reconciliation, so authors see code-shape errors before manifest-shape errors):

```ts
  // --- Residual-rule lint gate (B2): the pinned eslint-plugin-s2script rules, in-process,
  //     AFTER tsc (spec §5.3). Errors abort the build — no .s2sp. Warnings pass through.
  const lint = await lintPlugin(absDir, tc.program!);
  if (!lint.ok) {
    throw new Error(`lint failed (${lint.errorCount} error(s)):\n${lint.output}`);
  }
  if (lint.output.trim().length > 0) console.warn(lint.output);
```

- [ ] **Step 6: Run tests to verify they pass**

```bash
cd packages/sdk && node --experimental-strip-types --no-warnings --test test/lint.test.mjs test/build.test.mjs
```
Expected: PASS. **Known trap:** if the provided-`programs` path errors with "file was not found in any of the provided project(s)" for the temp ambient stub, the `targets` filter above already excludes out-of-dir files — check the message names an IN-dir file before touching anything; if the parser rejects `.ts` files because `allowImportingTsExtensions` conflicts, mirror the gate's exact options (the program already carries them — the parser only consumes, never rebuilds).

- [ ] **Step 7: Full-suite + base-plugin sweep**

```bash
cd packages/sdk && node build.mjs && npm test        # only the 13 known failures
cd ../.. && ./scripts/build-base-plugins.sh          # every SHIPPED plugin now passes the lint gate
```

If a base plugin trips a rule: it is either a REAL latent bug (fix the plugin code — e.g. a genuine ctx capture becomes a Scope; a floating factory promise gets awaited) or a rule false-positive (fix the rule + add the pattern as a `valid` RuleTester case). Record which in the commit message. Do NOT silence via config.

- [ ] **Step 8: Commit**

```bash
git add packages/eslint-plugin packages/sdk/src/lint packages/sdk/src/build.ts packages/sdk/package.json \
        packages/sdk/build.mjs packages/sdk/test/lint.test.mjs packages/sdk/test/fixtures/lint-violation \
        package-lock.json plugins
git commit -m "sdk: run the pinned s2script lint rules in-process after the tsc gate (B2)

configs.recommended (editor/projectService) + configs.build (provided program)
are one rule set. s2s build lints against the typecheck gate's own ts.Program -
identical resolution, no extra tsconfig - and refuses the .s2sp on any error.
A plugin's own eslint.config.* wins for editor/build parity."
```

---

### Task 10: Scaffold + editor/build parity debt (exports `./plugin`, tsconfigs, eslint config template)

**Files:**
- Modify: `packages/sdk/src/create/create.ts` (eslint.config.mjs template + devDeps)
- Modify: `packages/sdk/package.json` (`exports` gains `./plugin`)
- Modify: `tsconfig.base.json` (add `allowImportingTsExtensions`)
- Modify: every `plugins/*/tsconfig.json`, `plugins/disabled/*/tsconfig.json`, `examples/*/tsconfig.json` still including the DELETED `packages/globals/globals.d.ts`
- Create: `packages/sdk/test/tsconfig-base-parity.test.mjs`
- Modify: `packages/sdk/test/create-resolve.test.mjs` (expectations for the new devDeps)

**Interfaces:**
- Consumes: Task 9's `configs.recommended` signature (`recommended(opts?: { tsconfigRootDir?: string })`); Task 1's `packageJsonContent` (already apiVersion-less); `sharedCompilerOptionsJson` from `packages/sdk/src/tsconfig-shared.ts` (L1's T6 single source of truth).
- Produces: the scaffolded project shape every future `s2s create` emits (package.json + tsconfig.json + eslint.config.mjs + src/plugin.ts + .gitignore); the parity test that pins `tsconfig.base.json` to `sharedCompilerOptionsJson` forever.

Parity debts found while verifying this plan (all three are editor-green/build-red or editor-red/build-green divergences — exactly the class B2 exists to kill):
1. `packages/sdk/package.json` `exports` has NO `./plugin` entry — `import { plugin } from "@s2script/sdk/plugin"` (the L1 template!) fails under exports-aware node_modules resolution in an editor, while the gate's `paths` mapping bypasses `exports` and stays green.
2. ~40 in-repo plugin/example tsconfigs still `include` `../../packages/globals/globals.d.ts`, deleted by the consolidation — the editor loses the `console` global while the gate injects `packages/sdk/globals.d.ts`.
3. `tsconfig.base.json` duplicates the shared option literals by hand and misses `allowImportingTsExtensions` — drift is silent (it cannot `extends` a `.ts` module, so a TEST pins it instead).

- [ ] **Step 1: Write the failing parity test**

`packages/sdk/test/tsconfig-base-parity.test.mjs`:

```js
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { sharedCompilerOptionsJson } from "../src/tsconfig-shared.ts";

const here = dirname(fileURLToPath(import.meta.url));

test("tsconfig.base.json carries EVERY shared compiler option (editor == gate)", () => {
  const base = JSON.parse(
    readFileSync(join(here, "..", "..", "..", "tsconfig.base.json"), "utf8"),
  );
  for (const [key, want] of Object.entries(sharedCompilerOptionsJson)) {
    assert.deepEqual(
      base.compilerOptions[key],
      want,
      `tsconfig.base.json compilerOptions.${key} drifted from tsconfig-shared.ts`,
    );
  }
});

test("sdk exports map serves every root .d.ts subpath (no editor-only 404s)", () => {
  const pkg = JSON.parse(readFileSync(join(here, "..", "package.json"), "utf8"));
  assert.ok(pkg.exports["./plugin"], "exports must include ./plugin — the L1 entry surface");
  assert.equal(pkg.exports["./plugin"].types, "./plugin.d.ts");
});
```

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/tsconfig-base-parity.test.mjs`
Expected: FAIL on both tests (`allowImportingTsExtensions` missing; `./plugin` missing).

- [ ] **Step 2: Fix the three debts**

`packages/sdk/package.json` — insert into `exports` (alphabetically, after `"./net"`):

```json
    "./plugin": {
      "types": "./plugin.d.ts"
    },
```

`tsconfig.base.json` — add to `compilerOptions` (after `"types": []`):

```json
    "allowImportingTsExtensions": true,
```

In-repo tsconfig sweep (from repo root):

```bash
grep -rl 'packages/globals/globals.d.ts' plugins examples | while read -r f; do
  sed -i 's#packages/globals/globals\.d\.ts#packages/sdk/globals.d.ts#' "$f"
done
grep -rl 'packages/globals' plugins examples || echo CLEAN
```

Expected: `CLEAN`. (The relative depth `../../` vs `../../../` is untouched — only the `globals` → `sdk` path segment changes.)

- [ ] **Step 3: Scaffold the eslint config in `s2s create`**

In `packages/sdk/src/create/create.ts`:

Add the template function (next to `tsconfigJson()`):

```ts
/** Aligned with packages/sdk's own eslint dependency — bump the two together. */
const ESLINT_RANGE = "^10.7.0";

function eslintConfig(): string {
  return `import s2script from "@s2script/eslint-plugin";

// The SAME pinned rules \`s2s build\` enforces — the editor's ESLint extension picks this up,
// so a violation is a red squiggle before you ever build (green editor => green build).
export default s2script.configs.recommended({ tsconfigRootDir: import.meta.dirname });
`;
}
```

Extend the dep set: `createPackageNames` becomes

```ts
function createPackageNames(game: GameChoice): string[] {
  if (game === "cs2") {
    return ["sdk", "cs2", "eslint-plugin"];
  }
  return ["sdk", "eslint-plugin"];
}
```

(`registryDevDeps` then resolves `@s2script/eslint-plugin` live from the registry; `fileDevDeps` links `file:packages/eslint-plugin` in-tree — both existing code paths, no change needed there.)

In `packageJsonContent`, add eslint to the devDependencies object before serialization:

```ts
  const fileDeps = localPackagesDir ? fileDevDeps(localPackagesDir, game) : undefined;
  const devDependencies: Record<string, string> = {
    ...(fileDeps ?? registryDevDeps(game, version)),
    eslint: ESLINT_RANGE,
  };
```

In `createPlugin`, after the `tsconfig.json` write:

```ts
  writeFileSync(join(targetPath, "eslint.config.mjs"), eslintConfig());
```

- [ ] **Step 4: Update `create-resolve` expectations + run the create tests**

In `packages/sdk/test/create-resolve.test.mjs`, extend the `registryDevDeps` assertions: the cs2 map now has THREE keys (`@s2script/sdk`, `@s2script/cs2`, `@s2script/eslint-plugin`), the injectable `resolve` stub receives `@s2script/eslint-plugin` too. Mirror the existing test style, e.g. where it asserts the cs2 result equals `{"@s2script/sdk": "^X", "@s2script/cs2": "<resolved>"}` add `"@s2script/eslint-plugin": "<resolved>"`.

Run: `cd packages/sdk && node --experimental-strip-types --no-warnings --test test/create-resolve.test.mjs test/tsconfig-base-parity.test.mjs`
Expected: PASS.

- [ ] **Step 5: End-to-end scaffold smoke test (in-tree, file: links)**

```bash
cd packages/sdk && node build.mjs && cd ../..
rm -rf /tmp/claude-1000/-home-gkh-projects-s2script/6f69266b-df73-4a9f-b6d8-200f8ffd2103/scratchpad/s2s-create-smoke
node packages/sdk/dist/cli.js create \
  /tmp/claude-1000/-home-gkh-projects-s2script/6f69266b-df73-4a9f-b6d8-200f8ffd2103/scratchpad/s2s-create-smoke \
  --game none --name @demo/smoke -y --no-install
ls /tmp/claude-1000/-home-gkh-projects-s2script/6f69266b-df73-4a9f-b6d8-200f8ffd2103/scratchpad/s2s-create-smoke
node packages/sdk/dist/cli.js build \
  /tmp/claude-1000/-home-gkh-projects-s2script/6f69266b-df73-4a9f-b6d8-200f8ffd2103/scratchpad/s2s-create-smoke
```

Expected: the listing shows `eslint.config.mjs` next to `package.json`/`tsconfig.json`/`src`; the build prints a `dist/_demo_smoke.s2sp` path (typecheck + lint + bundle all green on the template — the template itself is the canary for rule false-positives).

- [ ] **Step 6: Full gates**

```bash
./scripts/check-plugins-typecheck.sh
cd packages/sdk && npm test && cd ../..
cd packages/eslint-plugin && npm test && cd ..
```

Expected: gate PASS; sdk suite = only the 13 known failures; eslint-plugin suite green.

- [ ] **Step 7: Commit**

```bash
git add packages/sdk/src/create/create.ts packages/sdk/package.json packages/sdk/test tsconfig.base.json \
        plugins examples
git commit -m "sdk: scaffold eslint.config.mjs + clear the editor/build parity debts (B2)

s2s create emits the recommended flat config (same pinned rules as s2s build)
and dev-deps eslint + @s2script/eslint-plugin. Fixes three divergences: exports
lacked ./plugin (L1 template unresolvable via node_modules), ~40 in-repo
tsconfigs included the deleted packages/globals path, tsconfig.base.json missed
allowImportingTsExtensions (now pinned to tsconfig-shared.ts by test)."
```

---

### Task 11: Finalization — full gate suite, changesets, PROGRESS entry

**Files:**
- Create: `.changeset/b1-b2-toolchain.md`
- Modify: `docs/PROGRESS.md` (append the finished-slice entry)

**Interfaces:**
- Consumes: everything above, complete and committed.
- Produces: the releasable state (changesets staged; history documented). No PR/`gt submit` — human decision.

- [ ] **Step 1: Run the ENTIRE verification battery**

```bash
cargo test -p s2script-core                       # green (single-threaded; no --test-threads)
cd packages/sdk && node build.mjs && npm test && cd ../..          # only the 13 known failures
cd packages/eslint-plugin && node build.mjs && npm test && cd ..   # green
make check-boundary
./scripts/check-plugins-typecheck.sh
./scripts/check-schema-generated.sh
./scripts/check-nav-generated.sh
./scripts/check-events-generated.sh
./scripts/check-csitem-generated.sh
./scripts/test-boundary-nameleak.sh
./scripts/build-base-plugins.sh
```

Every command must pass (modulo the enumerated 13). Any regression = stop, fix in the owning task's files, amend that concern here.

- [ ] **Step 2: Changeset**

`.changeset/b1-b2-toolchain.md`:

```md
---
"@s2script/sdk": minor
"@s2script/eslint-plugin": minor
---

B1 (build ⊇ load): `s2s build` now DERIVES the manifest — `apiVersion` is stamped from the SDK's
host-major constant (authored values ignored with a warning), the `publishes` name-set is derived
from `ctx.publish` calls (drift is a build error; `"self"` auto-derives), dependency-usage
advisories warn on declared-vs-used mismatches, and a `.s2script/types/<iface>/index.d.ts`
verified contract copy gives a consumer REAL dependency types plus a `compiledAgainst` hash that
the host verifies at load (contract drift now fails fast at load AND per-call).

B2: new `@s2script/eslint-plugin` — `no-ctx-escape`, `no-floating-promise-in-factory`,
`no-bigint-in-interface-payloads`, `no-await-in-raw-view` — pinned by the SDK, scaffolded by
`s2s create` (`eslint.config.mjs`), and executed in-process by `s2s build` after the tsc gate
against the gate's own `ts.Program`. Lint errors refuse the `.s2sp`.
```

- [ ] **Step 3: Append the PROGRESS entry**

Append to `docs/PROGRESS.md` (follow the file's existing entry format — date, slice name, what/why/result). Content requirements: B1+B2 as the final two slices of the safety-by-construction re-arch; name the four derivations/verifications (apiVersion stamp, typesSha256 load+call verify with the `.s2script/types` convention, publishes-from-code, warn-once refusals with `failed` visibility) and the four rules + two-config parity design; note the live gate for the loader-side pieces (a drifted-hash `.s2sp` refused on the Docker server, `sm plugins list` showing `failed`) is HELD for the human, consistent with the L1 live-gate precedent.

- [ ] **Step 4: Commit**

```bash
git add .changeset/b1-b2-toolchain.md docs/PROGRESS.md
git commit -m "docs(rearch): B1+B2 toolchain complete - changeset + PROGRESS entry

build superset-of load: apiVersion derived, typesSha256 enforced, publishes
generated, refusals remembered. eslint-plugin-s2script: 4 residual rules, one
pinned engine in editor and s2s build. Live gate (drifted .s2sp refusal on the
Docker server) HELD for human run."
```

---

## Open questions for human review

1. **`@s2script/eslint-plugin` is a NEW package** (`packages/eslint-plugin`). Locked decision #10 says "no new `@s2script/*` packages", which I read as governing *runtime capability* packages (its own text: a new capability is a subpath, `Hud` ≠ `@s2script/hud`). The lint plugin is dev-time tooling that the editor's ESLint must import as a standalone JS module; folding it into `@s2script/sdk` as a subpath would require adding a JS runtime export to a package whose `exports` map is types-only — a bigger contract change than a new tooling package. Spec §5.3's own name for it ("eslint-plugin-s2script, pinned by the SDK") reads as a separate artifact; the scoped form follows the npm-scope taxonomy (first-party ⇒ `@s2script/*`). **Veto point:** rename to unscoped `eslint-plugin-s2script` or fold as an `@s2script/sdk` subpath with a real `exports` overhaul.
2. **`publishes`-from-code is scoped to name-set derivation** (auto-`"self"` + exact reconciliation + literal-only names), NOT full generation: a decoupled contract's *version* is data that cannot come from code, and a conditional `ctx.publish` is statically indistinguishable from an unconditional one (the runtime `reconcile_publishes` keeps that residual, per spec §5.2's "runtime keeps the residual check"). Full generation would require an authored version *somewhere* anyway.
3. **Authored `s2script.apiVersion` is warn-and-ignore**, not a build error — erroring would break every existing out-of-tree plugin build on SDK upgrade for a field that no longer does anything. The warn names the stamped value and tells the author to delete the field.
4. **`compiledAgainst` is opt-in** (present only when the consumer keeps a `.s2script/types/<iface>/index.d.ts` copy). Hand-written ambient `declare module` consumers keep today's unverified-but-working behavior. The `s2s add <iface>` fetch command (extracting the contract from a producer `.s2sp`'s embedded `types/` member) is deliberately deferred to the registry slice — here the copy is a `cp`, and `zones-consumer-demo` documents the refresh command inline.
5. **Late-producer hash mismatch poisons per-call (`InterfaceTypesMismatch`), it does not unload the consumer.** Load-time refusal covers the doctrine's "fails at load"; once a consumer is running, a producer hot-swap to a drifted contract degrades that consumer's calls exactly like producer-absence does today (loud, named, per-call) instead of cascading unloads.
6. **ESLint version pair** pinned at plan-time as `eslint ^10.7.0` + `typescript-eslint 8.65.x`; Task 6 Step 1 verifies peer compatibility and falls back to `eslint ^9.39.0` if needed. Whichever pair lands is used identically in `packages/eslint-plugin`, `packages/sdk`, and the `s2s create` template constant `ESLINT_RANGE`.
7. **`no-ctx-escape` does not chase `ctx` passed as a function argument** (`registerAll(ctx)` at load is legal and common; the helper's own parameter is a fresh binding). A helper that stores its param for later use escapes the lint but still hits the runtime seal — accepted residual, documented in the rule description.
8. **`state()` returning BigInt is NOT covered** by `no-bigint-in-interface-payloads` (the reload-handoff wire is a different crossing than the inter-plugin wire, though it has the same JSON+BigInt failure). Candidate fifth rule if the footgun shows up in practice.
9. **Task 9's canonical lint path ignores warnings for gating** (errors only refuse the `.s2sp`); all four rules default to `error`, so this only matters for future advisory-severity rules.

## Self-Review

**Spec §5 coverage:**
- §5.2 row "apiVersion major — derive at build, load keeps gate" → Task 1 (stamp + drift test; loader gate untouched, asserted in Task 2's scope). ✓
- §5.2 row "publishes — derive from code, runtime keeps residual" → Task 5 (scoped derivation, documented in Open Questions #2; `reconcile_publishes` untouched). ✓
- §5.2 row "typesSha256 — wire the load-side verify" → Task 3 (load fail-fast + per-call backstop) + Task 4 (the consumer-side hash provenance the check needs — the "trace how a consumer records what it compiled against" answer: the `.s2script/types` byte-copy, hashed at build into `compiledAgainst`). ✓
- §5.2 row "dep advisories (declared-never-imported / import-not-declared)" → Task 5 Step 6 WARNs. ✓
- §5.2 row "config validation — done" → untouched, correctly. ✓
- Minor bug fold-in (poll_plugins re-warn) → Task 2 (WATCH_STATE row as path+mtime memory; reload variant keeps running version). ✓
- §5.3 ESLint over LS-plugin (locked #3) → Tasks 6-9; same pinned engine both ends (Task 9 configs; Task 10 scaffold). ✓
- §5.3 parity debt 1 (tsconfig fork) → L1's T6 already landed `tsconfig-shared.ts`; the REMAINING debt verified against the tree (stale `packages/globals` includes, missing `allowImportingTsExtensions` in base, missing `./plugin` export) → Task 10 + pinned by test. ✓
- §5.3 parity debt 2 (the four rules) → Tasks 6/7/8, each with valid+invalid RuleTester cases and exact messages. ✓
- In-process execution after the tsc gate, scaffolded by `s2s create`, SDK-pinned → Tasks 9/10 (exact pin `0.1.0`, `configs.build` reuses the gate's program). ✓

**Placeholder scan:** no TBD/TODO/"similar to Task N"; every code step carries the actual code; the two deliberate implementer-judgment points (Task 7's report-count calibration, Task 8's alias-symbol fallback) specify the decision procedure and the lock-in rule rather than deferring the design. Task 3 Step 7 explicitly subordinates its sketch to the neighboring in-isolate harness conventions — a verified-context instruction, not a placeholder.

**Consumes/Produces type-consistency:** `ImportSpec`/`set_plugin_imports(Vec<ImportSpec>)` (T3) matches T3's loader/v8host usage and test updates; `compiledAgainst: Record<string,string>` (T4 build) ⟷ `compiled_against: HashMap<String,String>` serde-renamed `"compiledAgainst"` (T3 loader); `TypecheckResult.program?: ts.Program` (T5) ⟷ `lintPlugin(dir, tc.program!)` (T9); `findFactory`/`isFunctionNode` (T6) reused verbatim in T7; rule ids in T9's `RULES` match the `rules` keys registered in T6/T7/T8 under the `s2script/` prefix; `configs.recommended({tsconfigRootDir})` (T9) matches T10's scaffold call; `STAMPED_API_VERSION` (T1) is the value asserted in T1's updated build test. Checked: `hashContract` is exported by `publishes.ts` (verified in tree); `hasPublishes`/`expandPublishes` exports verified; `sharedCompilerOptionsJson` export verified.



