# `s2s create` live version resolution — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the `s2s create` scaffolder from writing an unsatisfiable `@s2script/cs2` pin; resolve each non-`sdk` dependency version live from the registry at scaffold time.

**Architecture:** Three-PR Graphite stack off `main` (`f30c4cd`). PR 1 (bottom) migrates a stranded test fixture, fixing a currently-red build test. PR 2 (middle) reconciles `package-lock.json` with the post-consolidation workspace. PR 3 (top) is the version-resolution fix in the CLI plus a changeset, landing on a green, reconciled base. The version logic splits a thin `spawnSync` network seam (`resolvePublishedVersion`) from a pure, unit-testable formatter (`versionSpecFrom`), and threads an injectable resolver through `registryDevDeps` so tests never touch the network.

> **Execution note (2026-07-16):** This plan was authored for a 2-PR stack that folded a small `@s2script/cli` lockfile prune into PR 1's cleanup. During execution, that prune revealed the consolidation (#58/#59) had left the *entire* lockfile stale (30+ dead workspace entries + dangling `node_modules/@s2script/*` symlinks), so the lockfile work became its own **PR 2 — reconcile the post-consolidation lockfile** (remove dangling symlinks + from-scratch `npm install --package-lock-only` regen; 9 insertions / 893 deletions, no transitive churn). The version-resolution fix is now **PR 3**. Part A below is the fixture-only PR 1; Part B's re-parent step targets PR 2. The `package-lock.json` line in File Structure is superseded by the PR 2 reconcile.

**Tech Stack:** TypeScript (Node ESM, run via `node --experimental-strip-types`), `node:test` + `node:assert`, npm (registry query + lockfile), Changesets, Graphite (`gt`).

## Global Constraints

- **Dependencies point one way (game → core).** This change lives entirely in `@s2script/sdk` (the CLI); it must not import any game package. No `@s2script/cs2` import is added.
- **`sdk` pins to the running CLI's own version** (`readCliVersion()` → `^<version>`) — unchanged. It is correct by construction because the CLI *is* the `@s2script/sdk` artifact.
- **Every non-`sdk` package resolves live from the registry**; on any failure (non-zero exit, empty/garbage output, `npm` absent, package unpublished) it degrades to the floating spec `latest` — never a hardcoded number.
- **Do not touch** `runInstall`, the local `file:` (`fileDevDeps`) path, or the package-manager surface (npm/pnpm/yarn/bun/none).
- **Naming:** PascalCase types, camelCase functions/properties.
- **Run the gate suite per PR.** Each PR must pass on its own; `npm test` in `packages/sdk` and `./scripts/check-plugins-typecheck.sh` are the relevant gates.
- **13 known-failing CLI tests** (`schema-runtime.test.mjs` = 7, `player-identity.test.mjs` = 6) are pre-existing and out of scope — the failing count must be exactly these after each PR (the build-RANGE test moves out of the failing set in PR 1).

## File Structure

- `packages/sdk/test/fixtures/publisher-mapform-typeerror/` — **created** (moved from `packages/cli/`): the 3-file fixture the build-RANGE test needs.
- `packages/cli/` — **deleted**: the dead post-consolidation shell.
- `package-lock.json` — **modified**: drop the stale `@s2script/cli` workspace entries.
- `packages/sdk/src/create/create.ts` — **modified**: add `resolvePublishedVersion` + `versionSpecFrom`; export + re-signature `registryDevDeps`.
- `packages/sdk/test/create-resolve.test.mjs` — **modified**: import + assert the two exported functions.
- `.changeset/s2s-create-live-version-resolution.md` — **created**: patch bump for `@s2script/sdk`.

---

## Part A — PR 1 (bottom): `packages/cli` cleanup

Branch: `cli-shell-cleanup/migrate-fixture`, off `main`.

### Task 1: Migrate the stranded fixture (turns the red build-RANGE test green)

**Files:**
- Move: `packages/cli/test/fixtures/publisher-mapform-typeerror/{package.json,api.d.ts,src/plugin.ts}` → `packages/sdk/test/fixtures/publisher-mapform-typeerror/`
- Delete: `packages/cli/` (whole directory)
- Test (already exists, currently failing): `packages/sdk/test/build.test.mjs:93` — "build rejects a RANGE BEFORE the typecheck gate"

**Interfaces:**
- Consumes: nothing (starts from `main`).
- Produces: a green `packages/sdk` build test suite for PR 2 to build on.

- [ ] **Step 1: Start PR 1 from a clean `main`**

```bash
cd /home/gkh/projects/s2script
git checkout main   # the spec branch cli-create-versions/live-registry-resolution keeps commit 03c07cd safe
```

- [ ] **Step 2: Confirm the build-RANGE test is RED (fixture missing under sdk)**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings --test-name-pattern="RANGE BEFORE" test/build.test.mjs
```
Expected: FAIL — `AssertionError`, `actual: "ENOENT: no such file or directory, open '.../packages/sdk/test/fixtures/publisher-mapform-typeerror/package.json'"`.

- [ ] **Step 3: Move the 3 fixture files into the sdk test tree**

```bash
cd /home/gkh/projects/s2script
mkdir -p packages/sdk/test/fixtures/publisher-mapform-typeerror/src
git mv packages/cli/test/fixtures/publisher-mapform-typeerror/package.json packages/sdk/test/fixtures/publisher-mapform-typeerror/package.json
git mv packages/cli/test/fixtures/publisher-mapform-typeerror/api.d.ts     packages/sdk/test/fixtures/publisher-mapform-typeerror/api.d.ts
git mv packages/cli/test/fixtures/publisher-mapform-typeerror/src/plugin.ts packages/sdk/test/fixtures/publisher-mapform-typeerror/src/plugin.ts
```

- [ ] **Step 4: Delete the dead `packages/cli/` shell**

After the moves, `packages/cli/` holds only untracked cruft (`dist/`, `node_modules/`, now-empty test dirs).
```bash
cd /home/gkh/projects/s2script
rm -rf packages/cli
git status --short   # expect: only the 3 renames staged (R packages/cli/... -> packages/sdk/...)
```

- [ ] **Step 5: Confirm the build-RANGE test is now GREEN**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings --test-name-pattern="RANGE BEFORE" test/build.test.mjs
```
Expected: PASS (`tests 1`, `pass 1`, `fail 0`). The fixture's `publishes: { "@community/contract": "^1.0.0" }` range is now rejected before the typecheck gate, so the assertion matches `/is a RANGE/`.

- [ ] **Step 6: Confirm no NEW test breakage across the sdk suite**

```bash
cd /home/gkh/projects/s2script/packages/sdk
npm test 2>&1 | tail -25
```
Expected: the only failing tests are the 13 known ones (`schema-runtime.test.mjs` + `player-identity.test.mjs`). `build.test.mjs` is fully green.

- [ ] **Step 7: Create PR 1 branch with this commit**

```bash
cd /home/gkh/projects/s2script
git add -A
gt create cli-shell-cleanup/migrate-fixture -m "test: migrate stranded publisher-mapform-typeerror fixture into @s2script/sdk

The CLI-into-@s2script/sdk consolidation moved every test fixture into
packages/sdk/test/fixtures/ except publisher-mapform-typeerror, whose only
copy was left under the otherwise-dead packages/cli/. That stranded the
build.test.mjs RANGE test on ENOENT. Move the fixture and delete the dead shell.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```
Expected: `gt` creates branch `cli-shell-cleanup/migrate-fixture` off `main` with one commit.

### Task 2: Prune the stale `@s2script/cli` entries from `package-lock.json`

**Files:**
- Modify: `package-lock.json` (remove the two `@s2script/cli` blocks left from before the consolidation)

**Interfaces:**
- Consumes: PR 1 branch at Task 1's commit (with `packages/cli/` gone from disk).
- Produces: a lockfile with no reference to the removed workspace.

- [ ] **Step 1: Regenerate the lockfile (same command the repo's `version-packages` script uses)**

```bash
cd /home/gkh/projects/s2script
npm install --package-lock-only --ignore-scripts
```

- [ ] **Step 2: Verify the diff is limited to the `@s2script/cli` removal**

```bash
cd /home/gkh/projects/s2script
git diff --stat package-lock.json
git diff package-lock.json | grep -nE '@s2script/cli|packages/cli' || echo "no cli refs remain in the diff context"
grep -nE '"@s2script/cli"|"packages/cli"' package-lock.json || echo "OK: no @s2script/cli left in lockfile"
```
Expected: the diff removes the `node_modules/@s2script/cli` link entry and the `packages/cli` workspace block; the final `grep` prints the `OK:` line. If the diff shows large unrelated churn, STOP and investigate before committing (the lockfile may have been out of date for another reason — do not bundle that here).

- [ ] **Step 3: Commit onto the PR 1 branch**

```bash
cd /home/gkh/projects/s2script
git add package-lock.json
git commit -m "chore: drop stale @s2script/cli entry from package-lock after CLI consolidation

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

- [ ] **Step 4: Gate — PR 1 stands on its own**

```bash
cd /home/gkh/projects/s2script
./scripts/check-plugins-typecheck.sh 2>&1 | tail -5
```
Expected: passes (this change does not affect plugin typechecking, but the gate must be green on PR 1 alone).

---

## Part B — PR 2 (top): live version resolution in the CLI

Branch: `cli-create-versions/live-registry-resolution`, re-parented onto PR 1 (carries the already-committed spec `03c07cd`).

### Task 3: Re-parent PR 2 onto PR 1 and commit the plan doc

**Files:**
- Create: `docs/superpowers/plans/2026-07-16-s2s-create-live-version-resolution.md` (this file)

**Interfaces:**
- Consumes: PR 1 branch tip.
- Produces: PR 2 branch based on PR 1, holding the spec + plan docs.

- [ ] **Step 1: Replay the spec commit onto PR 1**

```bash
cd /home/gkh/projects/s2script
git rebase --onto cli-shell-cleanup/migrate-fixture main cli-create-versions/live-registry-resolution
git checkout cli-create-versions/live-registry-resolution
gt track -p cli-shell-cleanup/migrate-fixture   # tell gt PR 2's parent is PR 1
```
Expected: `cli-create-versions/live-registry-resolution` now has PR 1 as its parent; `git log --oneline -3` shows spec commit on top of PR 1's two commits.

- [ ] **Step 2: Commit the plan doc**

```bash
cd /home/gkh/projects/s2script
git add docs/superpowers/plans/2026-07-16-s2s-create-live-version-resolution.md
git commit -m "docs: implementation plan for s2s create live version resolution

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

### Task 4: `versionSpecFrom` — pure formatter (TDD)

**Files:**
- Modify: `packages/sdk/src/create/create.ts` (add exported `versionSpecFrom`)
- Test: `packages/sdk/test/create-resolve.test.mjs`

**Interfaces:**
- Produces: `export function versionSpecFrom(status: number | null, stdout: string | null): string` — returns `^<semver>` when `status === 0` and `stdout` (trimmed) starts with `\d+.\d+.\d+`, else `"latest"`.

- [ ] **Step 1: Write the failing test**

Edit `packages/sdk/test/create-resolve.test.mjs`. Change the create import (line 14) to also pull the new exports, and append the test:

```js
// line 14 becomes:
import { createPlugin, versionSpecFrom, registryDevDeps } from "../src/create/create.ts";
```

```js
// appended at end of file:
test("versionSpecFrom carets a clean semver and degrades to latest on any failure", () => {
  assert.equal(versionSpecFrom(0, "0.5.0\n"), "^0.5.0");
  assert.equal(versionSpecFrom(0, "1.2.3-beta.1\n"), "^1.2.3-beta.1");
  assert.equal(versionSpecFrom(0, ""), "latest");
  assert.equal(versionSpecFrom(0, "not-a-version"), "latest");
  assert.equal(versionSpecFrom(1, "0.5.0"), "latest");
  assert.equal(versionSpecFrom(null, "0.5.0"), "latest");
});
```

- [ ] **Step 2: Run it — verify it fails**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings --test-name-pattern="versionSpecFrom" test/create-resolve.test.mjs
```
Expected: FAIL — `versionSpecFrom` is not exported (`... is not a function` / import error).

- [ ] **Step 3: Implement `versionSpecFrom`**

In `packages/sdk/src/create/create.ts`, add these two functions immediately above `registryDevDeps` (which is at line 134). `spawnSync` is already imported at line 11.

```ts
/** Resolve a published package's current version from the registry, as a caret range.
 *  `npm view` respects .npmrc / private registries. Any failure — non-zero exit, empty or
 *  malformed output, npm absent, package unpublished — degrades to the floating `latest` spec. */
function resolvePublishedVersion(pkg: string): string {
  const r = spawnSync("npm", ["view", pkg, "version"], { encoding: "utf8", timeout: 5000 });
  return versionSpecFrom(r.status, r.stdout);
}

/** Pure formatter for a `npm view <pkg> version` result: a caret range on a clean semver,
 *  else the floating `latest`. Split out so the fallback logic is unit-testable without a network. */
export function versionSpecFrom(status: number | null, stdout: string | null): string {
  const v = (stdout ?? "").trim();
  return status === 0 && /^\d+\.\d+\.\d+/.test(v) ? `^${v}` : "latest";
}
```

- [ ] **Step 4: Run it — verify it passes**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings --test-name-pattern="versionSpecFrom" test/create-resolve.test.mjs
```
Expected: PASS (`pass 1`).

- [ ] **Step 5: Commit**

```bash
cd /home/gkh/projects/s2script
git add packages/sdk/src/create/create.ts packages/sdk/test/create-resolve.test.mjs
git commit -m "feat(cli): add versionSpecFrom + resolvePublishedVersion registry seam

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

### Task 5: `registryDevDeps` — resolve non-`sdk` packages live (TDD)

**Files:**
- Modify: `packages/sdk/src/create/create.ts:134-140` (export + re-signature `registryDevDeps`)
- Test: `packages/sdk/test/create-resolve.test.mjs`

**Interfaces:**
- Consumes: `resolvePublishedVersion` / `versionSpecFrom` from Task 4.
- Produces: `export function registryDevDeps(game: GameChoice, sdkVersion: string, resolve?: (pkg: string) => string): Record<string, string>` — `sdk` → `^${sdkVersion}`, every other package → `resolve("@s2script/<name>")` (default resolver = `resolvePublishedVersion`).

- [ ] **Step 1: Write the failing tests**

Append to `packages/sdk/test/create-resolve.test.mjs`:

```js
test("registryDevDeps pins sdk to the CLI version and resolves other packages live", () => {
  const resolve = (pkg) => (pkg === "@s2script/cs2" ? "^0.5.0" : "latest");
  const deps = registryDevDeps("cs2", "0.1.0", resolve);
  // sdk stays tied to the CLI's own version...
  assert.equal(deps["@s2script/sdk"], "^0.1.0");
  // ...but cs2 is whatever the registry resolver returned — NOT ^0.1.0 (the bug).
  assert.equal(deps["@s2script/cs2"], "^0.5.0");
});

test("registryDevDeps for game=none includes only the sdk", () => {
  const deps = registryDevDeps("none", "0.1.0", () => "unused");
  assert.deepEqual(deps, { "@s2script/sdk": "^0.1.0" });
});
```

- [ ] **Step 2: Run — verify they fail**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings --test-name-pattern="registryDevDeps" test/create-resolve.test.mjs
```
Expected: FAIL — `registryDevDeps` is not exported (import error), or (once exported) the 3-arg form is ignored and cs2 comes back `^0.1.0`.

- [ ] **Step 3: Implement — replace the `registryDevDeps` body**

In `packages/sdk/src/create/create.ts`, replace the current function (lines 134-140):

```ts
function registryDevDeps(game: GameChoice, version: string): Record<string, string> {
  const deps: Record<string, string> = {};
  for (const n of createPackageNames(game)) {
    deps[`@s2script/${n}`] = `^${version}`;
  }
  return deps;
}
```

with:

```ts
/** Registry-path dev deps. `@s2script/sdk` pins to the running CLI's own version (the CLI *is*
 *  that artifact, so its version is installable by construction); every other package versions
 *  independently and must be resolved live. `resolve` is injectable so tests avoid the network. */
export function registryDevDeps(
  game: GameChoice,
  sdkVersion: string,
  resolve: (pkg: string) => string = resolvePublishedVersion,
): Record<string, string> {
  const deps: Record<string, string> = {};
  for (const n of createPackageNames(game)) {
    deps[`@s2script/${n}`] = n === "sdk" ? `^${sdkVersion}` : resolve(`@s2script/${n}`);
  }
  return deps;
}
```

The existing call site at `packageJsonContent` (`const devDependencies = fileDeps ?? registryDevDeps(game, version);`) is unchanged — it passes `version` positionally as `sdkVersion`, and the default resolver applies.

- [ ] **Step 4: Run — verify they pass**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings --test-name-pattern="registryDevDeps" test/create-resolve.test.mjs
```
Expected: PASS (`pass 2`).

- [ ] **Step 5: Run the full create-resolve + build suites (existing `file:` scaffold test stays green)**

```bash
cd /home/gkh/projects/s2script/packages/sdk
node --experimental-strip-types --no-warnings test/create-resolve.test.mjs 2>&1 | tail -10
node --experimental-strip-types --no-warnings test/build.test.mjs 2>&1 | tail -10
```
Expected: both fully green. The in-tree scaffold test still asserts `@s2script/cs2` matches `/^file:/` (local path untouched).

- [ ] **Step 6: Commit**

```bash
cd /home/gkh/projects/s2script
git add packages/sdk/src/create/create.ts packages/sdk/test/create-resolve.test.mjs
git commit -m "fix(cli): resolve non-sdk create deps from the registry, not the sdk version

@s2script/cs2 was pinned to the sdk version (readCliVersion), unsatisfiable
once the packages diverged. Resolve every non-sdk package live via npm view;
sdk still self-reads; degrade to latest on failure.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

### Task 6: Changeset + full gate

**Files:**
- Create: `.changeset/s2s-create-live-version-resolution.md`

**Interfaces:**
- Consumes: the completed PR 2 code.
- Produces: a patch-bump changeset for `@s2script/sdk`.

- [ ] **Step 1: Write the changeset**

Create `.changeset/s2s-create-live-version-resolution.md`:

```markdown
---
"@s2script/sdk": patch
---

`s2s create` resolves non-`sdk` dependency versions live from the registry

The scaffolder pinned `@s2script/cs2` to the CLI's own (`@s2script/sdk`) version, which
is wrong once the two packages diverge — it emitted an unsatisfiable `^0.1.0` for a
`0.5.0` package and `npm install` failed. `@s2script/sdk` still pins to the CLI version
(the CLI *is* that artifact); every other package is now resolved from the registry at
scaffold time (`npm view`, respecting `.npmrc`), degrading to `latest` only when the
registry is unreachable, npm is absent, or the package is unpublished.
```

- [ ] **Step 2: Full gate for PR 2**

```bash
cd /home/gkh/projects/s2script/packages/sdk
npm test 2>&1 | tail -25   # only the 13 known failures; all create/build tests green
cd /home/gkh/projects/s2script
./scripts/check-plugins-typecheck.sh 2>&1 | tail -5   # passes
```
Expected: `packages/sdk` suite shows exactly the 13 known failures; the typecheck gate passes.

- [ ] **Step 3: Commit**

```bash
cd /home/gkh/projects/s2script
git add .changeset/s2s-create-live-version-resolution.md
git commit -m "chore: changeset — s2s create live version resolution (@s2script/sdk patch)

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

---

## Stack assembly & submission

- [ ] **Step 1: Verify the stack shape**

```bash
cd /home/gkh/projects/s2script
gt ls
```
Expected: `main` → `cli-shell-cleanup/migrate-fixture` (PR 1) → `cli-create-versions/live-registry-resolution` (PR 2).

- [ ] **Step 2: Submit the stack** (only after the user approves opening PRs)

```bash
cd /home/gkh/projects/s2script
gt submit --no-interactive
```

- [ ] **Step 3: Fill PR bodies** — each needs **Stack Context** + **Why** (write the body to a file, `gh pr edit N --body-file`, never a heredoc). PR bodies end with the session link `https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`.

## Self-review notes

- **Spec coverage:** registry-resolve non-sdk (Task 5) ✓; sdk self-read unchanged (Task 5 keeps the `n === "sdk"` branch) ✓; `latest` fallback (Task 4 `versionSpecFrom`) ✓; local `file:` path untouched (asserted green in Task 5 Step 5) ✓; `runInstall` untouched ✓; behavior matrix rows all map to code ✓; cleanup PR fixes the stranded fixture (Task 1) + prunes lockfile (Task 2) ✓; changeset (Task 6) ✓.
- **Type consistency:** `versionSpecFrom(status: number | null, stdout: string | null): string` and `registryDevDeps(game, sdkVersion, resolve?)` are used identically in tests and call site; `resolvePublishedVersion(pkg: string): string` is the default resolver. `spawnSync` already imported.
- **Placeholder scan:** none — every step carries exact code or an exact command with expected output.
