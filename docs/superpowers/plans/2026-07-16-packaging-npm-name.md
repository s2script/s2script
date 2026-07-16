# Part A: Claim the `s2script` npm name — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (- [ ]) syntax for tracking.

**Goal:** Claim the unscoped `s2script` npm name now with a minimal package whose real forwarding bin runs the installed `@s2script/cli`, so `npx s2script build` works today and the name can't be squatted.

**Architecture:** A new `packages/s2script/` package declares `bin { s2script }` and a real npm `dependency` on `@s2script/cli`. Its bin is a tiny shim that forwards to the CLI *by module path* (`require.resolve("@s2script/cli/dist/cli.js")` → spawn `node <resolvedPath>` with argv passed through), never by the `s2script` bin name — the load-bearing correctness point that avoids infinite recursion. The forwarding logic is factored into a dependency-injectable `forward()` so it is unit-testable without publishing.

**Tech Stack:** Node CJS bin shim (`.cjs`), `node:child_process` spawn, node's built-in test runner (`.mjs` under `test/`). No TypeScript, no build step for this package.

## Global Constraints

- **Ship work as a stack, not a branch (Graphite).** This part is one atomic PR; still use `gt` and run the gate suite on it. Branch naming: `packaging-name/<terse-change>` (per CLAUDE.md §7 of the spec: stack prefix `packaging-name/…`).
- **Gate suite must pass on the PR:** `make check-boundary`, `./scripts/check-plugins-typecheck.sh`, `cargo test -p s2script-core`, and the `check-*-generated.sh` scripts. This part touches only a new isolated npm package (no core, no games, no generated files), so the binding proof is the new package's own unit test plus the untouched gate suite staying green.
- **Boundary rule:** core is engine-generic and never imports `games/*`. This part touches neither `core/` nor `games/` — no boundary impact.
- **Forward by module path, never by bin name.** Both `s2script` and `@s2script/cli` declare a bin named `s2script`; forwarding via PATH / `.bin` / the bin name can resolve back to this shim and loop. The shim MUST `require.resolve("@s2script/cli/dist/cli.js")` and execute that file directly. This is the one non-negotiable correctness invariant of Part A.
- **`@s2script/cli` is a real npm `dependency`** of `s2script` (so it is always installed); it is NOT a `pluginDependency` and never reaches any derived plugin manifest.
- **Commit-message trailer** (CLAUDE.md git section): end every commit message with `Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`. **PR-body trailer:** end the PR body with `https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`. Write PR bodies with the Write tool to a file and `gh pr edit N --body-file` — never a heredoc.
- **Scope guard:** Part A only plants the flag and wires the bin. It does NOT move any `.d.ts` files, does NOT touch `s2require`, and does NOT rename the root `package.json`. Part C replaces this package's contents later.

---

## PR 1: `packaging-name/forwarding-bin` — the minimal `s2script` package with a forwarding bin

Everything in Part A is one atomic PR: it adds a brand-new, self-contained `packages/s2script/` directory (package.json + bin shim + forwarding module + its unit test) and nothing else. It merges safely on its own — no existing code depends on it, and its own unit test proves the forwarding invariant.

**Gate that proves this PR:** `node --test packages/s2script/test/*.test.mjs` passes (the forwarding-logic unit test, including the anti-recursion assertion); the existing gate suite stays green (nothing it covers changed).

### Task A1 — the forwarding module (`forward.cjs`), test-first

**Files:**
- Create: `packages/s2script/forward.cjs` (new, ~25 lines)
- Test: `packages/s2script/test/forward.test.mjs` (new)

**Interfaces:**
- Produces: `forward(args: string[], deps?: { resolve?: (id: string) => string, spawn?: typeof import("node:child_process").spawn }): ChildProcess` — resolves the CLI entry by module path via `deps.resolve` (default `require.resolve`), spawns `process.execPath` (node) with `[cliPath, ...args]` and `stdio: "inherit"` via `deps.spawn` (default `node:child_process` spawn), and wires the child's exit to `process.exit`. Dependency injection exists purely so the test can observe the spawn call without launching a real process.
- Consumes: `@s2script/cli/dist/cli.js` (resolved by module path, never by bin name).

**Steps:**

- [ ] **Step 1: Write the failing test.** Create `packages/s2script/test/forward.test.mjs` with the COMPLETE code below. It stubs `resolve` and `spawn`, asserts the shim (a) resolves the CLI *by module path* `@s2script/cli/dist/cli.js`, (b) spawns the node binary (`process.execPath`) with `[cliPath, ...args]` so argv passes through, and (c) does NOT shell out to a PATH/`.bin` `s2script` (asserts the spawned command is node and the CLI came from `require.resolve`, never the string `"s2script"`/`"npx"`).

```js
import { test } from "node:test";
import assert from "node:assert";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { forward } = require("../forward.cjs");

function stubChild() {
  // Minimal ChildProcess stand-in: forward() registers an "exit" handler but
  // the test never fires it, so process.exit is never reached.
  return { on() { return this; } };
}

test("forwards to the CLI by module path, spawning node with argv passed through", () => {
  const calls = { resolve: [], spawn: [] };
  const child = forward(["build", "--out", "x"], {
    resolve: (id) => {
      calls.resolve.push(id);
      return "/fake/node_modules/@s2script/cli/dist/cli.js";
    },
    spawn: (cmd, args, opts) => {
      calls.spawn.push({ cmd, args, opts });
      return stubChild();
    },
  });

  // Resolved BY MODULE PATH, not by bin name.
  assert.deepEqual(calls.resolve, ["@s2script/cli/dist/cli.js"],
    "must resolve the CLI entry by module path");

  assert.equal(calls.spawn.length, 1, "spawns exactly once");
  const s = calls.spawn[0];

  // Spawns the node binary, not a PATH/.bin `s2script` (anti-recursion).
  assert.equal(s.cmd, process.execPath, "must spawn the node binary, not a bin name");
  assert.notEqual(s.cmd, "s2script", "must never spawn a `s2script` bin (would recurse)");
  assert.notEqual(s.cmd, "npx", "must never shell out via npx");

  // First arg is the resolved CLI path; the rest is argv passed through verbatim.
  assert.deepEqual(s.args, [
    "/fake/node_modules/@s2script/cli/dist/cli.js",
    "build", "--out", "x",
  ], "must pass the resolved CLI path then argv through unchanged");

  assert.equal(s.opts && s.opts.stdio, "inherit", "child inherits stdio");
  assert.ok(child, "returns the child process");
});
```

- [ ] **Step 2: Run it, expect FAIL.** Run `node --test packages/s2script/test/forward.test.mjs`. Expected: a failure because `../forward.cjs` does not exist yet — output contains `Cannot find module` (or `MODULE_NOT_FOUND`) and the test file is reported as failing / erroring.

- [ ] **Step 3: Implement `forward.cjs`.** Create `packages/s2script/forward.cjs` with this COMPLETE code:

```js
"use strict";

// Forward the `s2script` bin to the installed `@s2script/cli`.
//
// LOAD-BEARING INVARIANT: forward BY MODULE PATH, never by bin name.
// Both `s2script` (this package) and `@s2script/cli` declare a bin named
// `s2script`. Forwarding by spawning the `s2script` bin (PATH / `.bin`) can
// resolve back to THIS shim → infinite recursion. `require.resolve` on the
// module path always lands inside @s2script/cli (a real dependency of this
// package), so this is the only safe entry.

const { spawn } = require("node:child_process");

/**
 * @param {string[]} args argv to hand to the CLI (typically process.argv.slice(2))
 * @param {{ resolve?: (id: string) => string, spawn?: typeof spawn }} [deps]
 *        injectable seams for tests; production uses the defaults.
 * @returns {import("node:child_process").ChildProcess}
 */
function forward(args, deps = {}) {
  const resolve = deps.resolve || ((id) => require.resolve(id));
  const spawnFn = deps.spawn || spawn;

  const cliPath = resolve("@s2script/cli/dist/cli.js");
  const child = spawnFn(process.execPath, [cliPath, ...args], { stdio: "inherit" });

  child.on("exit", (code, signal) => {
    if (signal) process.kill(process.pid, signal);
    else process.exit(code == null ? 0 : code);
  });

  return child;
}

module.exports = { forward };
```

- [ ] **Step 4: Run it, expect PASS.** Run `node --test packages/s2script/test/forward.test.mjs`. Expected output includes `pass 1` and `fail 0` (the single test passes).

- [ ] **Step 5: Commit.** From the repo root:
```bash
git add packages/s2script/forward.cjs packages/s2script/test/forward.test.mjs
gt create packaging-name/forwarding-bin -m "feat(s2script): forwarding-bin module that runs @s2script/cli by module path

The bin shim for the reserved `s2script` name forwards to @s2script/cli by
require.resolve on the module path (never the bin name), so `npx s2script build`
cannot recurse into itself. Unit-tested via injectable resolve/spawn seams.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```
(In a fresh worktree the branch usually starts untracked — run `gt track -p main` first if `gt create` reports the branch is not tracked.)

### Task A2 — the bin entrypoint (`bin/s2script.cjs`)

**Files:**
- Create: `packages/s2script/bin/s2script.cjs` (new, ~4 lines)

**Interfaces:**
- Consumes: `forward` from `../forward.cjs`.
- Produces: the executable the `bin` field points at; passes `process.argv.slice(2)` to `forward`.

**Steps:**

- [ ] **Step 1: Implement the bin entry.** Create `packages/s2script/bin/s2script.cjs` with this COMPLETE code:

```js
#!/usr/bin/env node
"use strict";
const { forward } = require("../forward.cjs");
forward(process.argv.slice(2));
```

- [ ] **Step 2: Verify it is a thin wrapper (no logic to test separately).** Run `node -e "require('./packages/s2script/bin/s2script.cjs')" -- --help 2>&1 | head -c 200`. Expected: it attempts to resolve `@s2script/cli` — because `@s2script/cli` is not yet installed under `packages/s2script/node_modules` (it's a workspace sibling, resolvable from the repo), this may print the CLI's help text or a `Cannot find module '@s2script/cli/dist/cli.js'` error. Either outcome confirms the wrapper delegates to `forward`; the wrapper itself holds no branch logic worth a dedicated test (A1's test already covers `forward`). Do not gate on the exact output here.

- [ ] **Step 3: Commit.**
```bash
git add packages/s2script/bin/s2script.cjs
gt modify -m "feat(s2script): bin entry that delegates argv to forward()

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

### Task A3 — the package manifest (`package.json`) + test script

**Files:**
- Create: `packages/s2script/package.json` (new)

**Interfaces:**
- Produces: name `"s2script"`, version `"0.0.1"`, `bin { s2script: "bin/s2script.cjs" }`, npm `dependency` `@s2script/cli`, `files` whitelist, `test` script, `publishConfig.access: public`.
- Consumes: `@s2script/cli` (real dependency).

**Steps:**

- [ ] **Step 1: Implement `package.json`.** Create `packages/s2script/package.json` with this COMPLETE content. (No `"type": "module"` — the shim is CJS `.cjs`, so `require`/`require.resolve` are available natively, matching the spec's `require.resolve("@s2script/cli/dist/cli.js")` wording.)

```json
{
  "name": "s2script",
  "version": "0.0.1",
  "description": "The s2script CLI for building Source 2 / CS2 plugins. Placeholder release that forwards to @s2script/cli; the real types + CLI land in a later version.",
  "bin": {
    "s2script": "bin/s2script.cjs"
  },
  "scripts": {
    "test": "node --test test/*.test.mjs"
  },
  "dependencies": {
    "@s2script/cli": "^0.2.0"
  },
  "publishConfig": {
    "access": "public"
  },
  "files": [
    "bin",
    "forward.cjs"
  ],
  "repository": {
    "type": "git",
    "url": "https://github.com/GabeHirakawa/s2script.git"
  }
}
```

- [ ] **Step 2: Run the package's own test through its script.** Run `npm test --prefix packages/s2script`. Expected: node's test runner reports `pass 1`, `fail 0` for `test/forward.test.mjs`.

- [ ] **Step 3: Install the workspace so `@s2script/cli` links under the new package, then smoke the resolution seam.** Run `npm install` at the repo root (the root `workspaces: ["packages/*"]` glob auto-includes `packages/s2script`). Then run:
```bash
node -e 'const {forward}=require("./packages/s2script/forward.cjs"); forward(["--nonexistent-cmd"], { spawn:(c,a)=>{console.log("SPAWN", c===process.execPath?"node":c, a[0]); return {on(){return this;}};} });'
```
Expected output: `SPAWN node <abs path>/packages/cli/dist/cli.js` — proving the default (unstubbed) `require.resolve` finds the real CLI entry by module path and would spawn node against it.

- [ ] **Step 4: Confirm the gate suite is unaffected.** Run `make check-boundary` and `./scripts/check-plugins-typecheck.sh`. Expected: both pass unchanged (this PR added only an isolated npm package; it touches no core, no games, no plugin sources, no generated files).

- [ ] **Step 5: Commit and submit the stack.**
```bash
git add packages/s2script/package.json package-lock.json
gt modify -m "feat(s2script): package.json claiming the name with a forwarding bin

name \"s2script\" @0.0.1, bin -> bin/s2script.cjs, @s2script/cli as a real npm
dependency so the CLI is always installed. Minimal placeholder; Part C replaces
the contents with the real types + absorbed CLI.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
gt submit --no-interactive
```

- [ ] **Step 6: Write the PR body and attach it.** Write the body to a file with the Write tool, then:
```bash
gh pr edit <N> --body-file /tmp/claude-1000/-home-gkh-projects-s2script/513cd495-bd10-4b41-b735-e6057bbabbe5/scratchpad/pr-body-partA.md
```
Body must contain a **Stack Context** section (Part A of the packaging-consolidation spec: claim the `s2script` name now with a real forwarding bin so `npx s2script build` works today and the name can't be squatted; Parts B/C are independent stacks), a **Why** section (a types-only placeholder would turn today's honest `npx s2script build` 404 into `could not determine executable` — worse; the bin must forward by module path, never by bin name, or it recurses), and end with the trailer line `https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`.

---

## Manual steps (NOT CI) — first publish, provenance bootstrap, and the live smoke check

These cannot run in CI (they require npm registry credentials and a network publish). Perform them by hand after PR 1 merges. Record the outcome in `docs/PROGRESS.md` per the slice cadence.

1. **npm trusted-publishing bootstrap (per-package, first time).** `npm publish` provenance / trusted publishing re-bootstraps per package *name*; `s2script` has never been published, so its OIDC/trusted-publisher relationship must be set up fresh (the `@s2script/*` scope's existing setup does not carry over to the unscoped name). Configure the trusted publisher for `s2script` in the npm UI / CI publish workflow before the first publish.

2. **Publish.** From `packages/s2script/`: `npm publish` (the package is public via `publishConfig.access`). Confirm the published tarball contains only `bin/`, `forward.cjs`, and `package.json` (the `files` whitelist) — no stray sources.

3. **Live smoke check — `npx s2script build` in a clean directory (the Part A gate).** In an empty temp dir with no repo checkout:
```bash
cd "$(mktemp -d)"
npx -y s2script build
```
Expected: `npx` fetches `s2script`, which pulls `@s2script/cli` as a dependency, and the forwarding bin runs the CLI's `build` command — you get the CLI's real behavior (e.g. an "expected a plugin directory / no package.json" style error from the CLI), **never** `could not determine executable` and **never** an infinite-recursion / stack-overflow. That distinction is the whole point of the real forwarding bin.

4. **Known transient `.bin` collision warning.** A user who installs *both* `s2script` and `@s2script/cli` sees an npm `.bin` collision warning (both declare a `s2script` bin). This is expected and harmless for the placeholder window; it disappears in **Part C**, when the CLI is absorbed into `s2script` and `@s2script/cli` is deprecated/aliased. Note this in the release notes so it isn't mistaken for a bug.
