# Packaging Review-Debt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (- [ ]) syntax for tracking.

**Goal:** Close the six review-debt findings (B1–B6) surfaced by `/code-review high` on the
contract-grammar stack (#36–#41) — mechanical fixes plus one that corrects a just-frozen manifest
shape — as a Graphite stack of small, independently-atomic PRs.

**Architecture:** Four CLI/TypeScript findings live in `packages/cli/src/{publishes,build}.ts` (the
`.s2sp` build path); two Rust findings live in `core/src/{interfaces,v8host}.rs` (the interface
registry + the isolate shutdown teardown). Each finding is isolated to one PR; the two `build.ts`
findings are ordered (read-once → fail-fast) so they don't churn each other, and the one that touches
the frozen manifest shape (B2) is isolated with a body callout.

**Tech Stack:** TypeScript (Node's built-in test runner, `.mjs` under `packages/cli/test/`); Rust
(`cargo test -p s2script-core`, single-threaded per `.cargo/config.toml`).

## Global Constraints

- **Ship as a stack, not a branch.** One Graphite stack `packaging-debt`, roughly one PR per finding;
  argue for more PRs, never fewer. Branch naming `packaging-debt/<terse-change>` per CLAUDE.md.
- **Run the gate suite PER PR, not once at the top.** The relevant gates for this stack:
  `make check-boundary`; `./scripts/check-plugins-typecheck.sh`; `cargo test -p s2script-core`
  (single-threaded — NEVER pass `--test-threads`); `cd packages/cli && npm test`. CLI-only PRs are
  proven by `npm test` + `check-plugins-typecheck.sh`; Rust PRs by `cargo test -p s2script-core` +
  `make check-boundary`.
- **Core is engine-generic; never imports `games/*`.** No finding here adds a game import; the
  boundary gate must stay green on the two Rust PRs regardless.
- **Each PR is atomic** — passes the gate suite and is safe to merge on its own.
- **B2 corrects a just-frozen manifest shape** (distribution spec §9). Isolate it in its own PR and
  name the frozen-shape change in the PR body.
- **Commit with `gt`.** Every commit message ends with the trailer
  `Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`; every PR body ends with
  `https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`. In a fresh worktree run
  `gt track -p main` before the first `gt create`.

## Stack map (PR boundaries, dependency order)

| PR | Finding | Files | Gate that proves it |
|----|---------|-------|---------------------|
| 1 | B1 — trim the stamped version | `publishes.ts` | `packages/cli` `npm test` (new trim regression) |
| 2 | B4 — read `package.json` once | `build.ts` | `packages/cli` `npm test` (existing build tests) |
| 3 | B3 — fail fast before tsc/esbuild | `build.ts` | `packages/cli` `npm test` (new fail-fast regression) |
| 4 | B2 — one contract per plugin (frozen shape) | `publishes.ts`, `build.ts` | `packages/cli` `npm test` (new rejection regression) |
| 5 | B5 — handle the discarded `Result`s | `interfaces.rs` | `cargo test -p s2script-core` + clean `cargo build --tests` |
| 6 | B6 — clear the publishes registries on shutdown | `v8host.rs` | `cargo test -p s2script-core` (new shutdown test) |

Order rationale: B1 is a self-contained `publishes.ts` edit and lands first. B4 (read-once) lands
before B3 (fail-fast) because B3 hoists the derive step above tsc/esbuild and reuses the single early
`pkg` that B4 establishes — reversing them would re-cut the same region twice. B2 is isolated last of
the CLI trio (frozen-shape blast radius). The two Rust findings are independent and land on top.

---

## PR 1: B1 — trim the stamped `publishes` version

**Finding (`publishes.ts:99`):** `isConcreteVersion` trims for the *check* (`CONCRETE_SEMVER.test(v.trim())`),
but `derivePublishes` stamps the *untrimmed* value into the manifest — `" 1.0.0 "` reaches the
manifest with surrounding whitespace. Fix: trim at the stamp.

### Task 1.1 — Trim the version at the manifest stamp

**Files:**
- Modify: `packages/cli/src/publishes.ts` (the stamp loop, ~lines 98–101)
- Test: `packages/cli/test/publishes.test.mjs` (add one test)

**Interfaces:**
- Consumes: `derivePublishes(authored: PublishesAuthored, pkgName: string, pkgVersion: string, typesPath: string | null): Record<string, PublishDecl>`
- Produces: same signature; the `PublishDecl.version` field is now whitespace-trimmed.

- [ ] **Step 1: Write the failing test** — append to `packages/cli/test/publishes.test.mjs`:
```js
test("derivePublishes: trims surrounding whitespace off the stamped version", () => {
  // isConcreteVersion trims for the CHECK; the stamp must trim too, or " 1.0.0 " reaches the manifest.
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  writeFileSync(p, "export declare function w(): void;\n");
  const out = derivePublishes({ "@x/y": " 1.0.0 " }, "@a/b", "1.0.0", p);
  assert.equal(out["@x/y"].version, "1.0.0", "stamped version must be trimmed, not ' 1.0.0 '");
});
```
- [ ] **Step 2: Run it, expect FAIL** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/publishes.test.mjs`
  Expected: the new test fails with `AssertionError [ERR_ASSERTION]: stamped version must be trimmed` —
  actual `' 1.0.0 '`, expected `'1.0.0'`.
- [ ] **Step 3: Implement** — in `packages/cli/src/publishes.ts`, change the stamp loop from:
```ts
  const typesSha256 = hashContract(typesPath);
  const out: Record<string, PublishDecl> = {};
  for (const name of names) {
    out[name] = { version: expanded[name], typesSha256 };
  }
  return out;
```
  to (trim the range at the stamp — the concrete-version check already trims, so the two agree):
```ts
  const typesSha256 = hashContract(typesPath);
  const out: Record<string, PublishDecl> = {};
  for (const name of names) {
    out[name] = { version: expanded[name].trim(), typesSha256 };
  }
  return out;
```
- [ ] **Step 4: Run it, expect PASS** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/publishes.test.mjs`
  Expected: all tests pass (`# pass <n>`, `# fail 0`), including the new trim test.
- [ ] **Step 5: Run the CLI suite** — from `packages/cli`: `npm test`
  Expected: `# fail 0` across every `test/*.test.mjs`.
- [ ] **Step 6: Commit** —
```bash
git add packages/cli/src/publishes.ts packages/cli/test/publishes.test.mjs
gt create packaging-debt/trim-stamped-version -m "fix(cli): trim the stamped publishes version (B1)

isConcreteVersion trims for the concrete-version CHECK, but derivePublishes
stamped the UNTRIMMED range into the manifest — ' 1.0.0 ' reached manifest.json
with surrounding whitespace. Trim at the stamp so the two agree.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

**Gate for PR 1:** `cd packages/cli && npm test` green (the new trim regression is the proof).

---

## PR 2: B4 — read `package.json` once, dedupe the import

**Finding (`build.ts:48`):** `package.json` is read + parsed twice (`pkgEarly` at line 48, `pkg` at
line 62) and `readFileSync` is imported twice (line 14 and, aliased as `readFileSyncRaw`, line 20).
Read once, reuse; drop the duplicate import. Pure refactor — the existing build tests are the proof.

### Task 2.1 — Collapse to a single read and a single import

**Files:**
- Modify: `packages/cli/src/build.ts` (imports ~14/20; the early gate block ~44–62; the embed read ~133)
- Test: `packages/cli/test/build.test.mjs` (existing — no change; it is the regression)

**Interfaces:**
- Consumes/Produces: `buildPlugin(dir: string, packagesDir?: string): Promise<string>` — unchanged
  signature and behavior; only the internal read/import structure changes.

- [ ] **Step 1: Confirm the existing tests pass first** — from `packages/cli`: `npm test`
  Expected: `# fail 0`. (This refactor must not change behavior; establish the green baseline.)
- [ ] **Step 2: Implement — remove the duplicate import.** In `packages/cli/src/build.ts` delete
  line 20 entirely:
```ts
import { readFileSync as readFileSyncRaw } from "node:fs";
```
- [ ] **Step 3: Implement — read `package.json` once.** Replace the current early block (lines
  ~44–62), which reads twice:
```ts
export async function buildPlugin(dir: string, packagesDir?: string): Promise<string> {
  const absDir = resolve(dir);

  // --- publishes ⇒ types gate (before we spend cycles on tsc/esbuild) ---
  const pkgEarly: PluginPackageJson = JSON.parse(readFileSync(join(absDir, "package.json"), "utf8"));
  const gate = assertPublishesTypes(pkgEarly, absDir);
  if (!gate.ok) {
    throw new Error(`publish gate failed: ${gate.error}`);
  }

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on error. ---
  const tc = typecheckPlugin(absDir, packagesDir !== undefined ? { packagesDir } : undefined);
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }

  // --- Read package.json ---   (existing code continues unchanged) ---
  const pkgPath = join(absDir, "package.json");
  const pkg: PluginPackageJson = JSON.parse(readFileSync(pkgPath, "utf8"));

  const { name, version } = pkg;
```
  with a single read reused by the gate and the rest:
```ts
export async function buildPlugin(dir: string, packagesDir?: string): Promise<string> {
  const absDir = resolve(dir);

  // --- Read package.json ONCE; every step below reuses this parse. ---
  const pkgPath = join(absDir, "package.json");
  const pkg: PluginPackageJson = JSON.parse(readFileSync(pkgPath, "utf8"));

  // --- publishes ⇒ types gate (before we spend cycles on tsc/esbuild) ---
  const gate = assertPublishesTypes(pkg, absDir);
  if (!gate.ok) {
    throw new Error(`publish gate failed: ${gate.error}`);
  }

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on error. ---
  const tc = typecheckPlugin(absDir, packagesDir !== undefined ? { packagesDir } : undefined);
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }

  const { name, version } = pkg;
```
- [ ] **Step 4: Implement — retire the `readFileSyncRaw` alias at the embed read.** In the embedded
  verified-copy block, change:
```ts
    const contract = readFileSyncRaw(gate.typesPath);
```
  to:
```ts
    const contract = readFileSync(gate.typesPath);
```
- [ ] **Step 5: Run the build tests, expect PASS** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/build.test.mjs`
  Expected: all build tests pass (`# fail 0`) — same behavior, one read.
- [ ] **Step 6: Run the CLI suite** — from `packages/cli`: `npm test` → `# fail 0`.
- [ ] **Step 7: Commit** —
```bash
git add packages/cli/src/build.ts
gt create packaging-debt/read-pkg-once -m "refactor(cli): read plugin package.json once in buildPlugin (B4)

package.json was read + parsed twice (pkgEarly, pkg) and readFileSync imported
twice (once aliased readFileSyncRaw). Read once at the top, reuse for the gate
and the manifest; drop the duplicate import. No behavior change.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

**Gate for PR 2:** `cd packages/cli && npm test` green (the existing build tests are the regression
that proves the refactor is behavior-preserving).

---

## PR 3: B3 — reject a `publishes` range before tsc + esbuild (fail fast)

**Finding (`build.ts:118`):** the range rejection lives inside `derivePublishes`, which is only called
at line 118 — *after* the typecheck gate (line 55) and the esbuild bundle (line 94). A plugin that
declares an unresolvable range still pays for a full tsc + esbuild pass before being told no. Move the
derive/validate step ahead of the expensive steps and reuse its result at manifest assembly.

Builds on PR 2: reuses the single early `pkg`. `derivePublishes` also throws on the RANGE case, so
hoisting the call is exactly the fail-fast fix — no new validation function needed.

### Task 3.1 — Hoist `derivePublishes` above the typecheck/esbuild steps

**Files:**
- Modify: `packages/cli/src/build.ts` (the early block + the manifest-assembly block ~106–121)
- Test: `packages/cli/test/build.test.mjs` (add one fail-fast regression)
- Test fixtures: reuse the existing `packages/cli/test/fixtures/publisher-mapform` (range publishes)

**Interfaces:**
- Consumes: `derivePublishes(publishes, name, version, gate.typesPath)` — throws on a RANGE.
- Produces: `buildPlugin(...)` — unchanged signature; the RANGE rejection now precedes tsc + esbuild.

- [ ] **Step 1: Inspect the fail-fast fixture** — confirm `publisher-mapform` declares a range:
  `cat packages/cli/test/fixtures/publisher-mapform/package.json`
  Expected: `s2script.publishes` is `{ "@community/contract": "^1.0.0" }` (a range).
- [ ] **Step 2: Write the failing test** — append to `packages/cli/test/build.test.mjs`. It spies on
  `esbuild.build` to prove it is never reached when the range is rejected:
```js
import * as esbuild from "esbuild";

test("build rejects a RANGE BEFORE running esbuild (fail fast)", async () => {
  // The range rejection must fire before the expensive tsc/esbuild steps. Spy on esbuild.build:
  // if the rejection is fail-fast, the bundler is never invoked.
  const realBuild = esbuild.build;
  let esbuildCalled = false;
  esbuild.build = ((opts) => { esbuildCalled = true; return realBuild(opts); });
  try {
    await assert.rejects(
      () => buildPlugin(join(here, "fixtures", "publisher-mapform"), packagesDir),
      /is a RANGE/,
    );
    assert.equal(esbuildCalled, false, "esbuild.build must NOT run when the range is rejected");
  } finally {
    esbuild.build = realBuild;
  }
});
```
  **Implementer note (test mechanism):** `import * as esbuild` yields a live but often **read-only**
  namespace — `esbuild.build = …` may throw `Cannot assign to read only property`. If it does, use the
  robust alternative instead of the spy: give the fixture BOTH a range `publishes` AND a deliberate
  type error, then assert `buildPlugin` rejects with `/is a RANGE/` and **not** `/typecheck failed/`.
  Today the typecheck runs first, so the current code surfaces the typecheck error; after the fix
  (derive hoisted above tsc), the range error surfaces first — a clean fail-fast observable with no
  monkeypatching. Pick whichever runs in this repo's node version; both prove the same ordering.

- [ ] **Step 3: Run it, expect FAIL** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/build.test.mjs`
  Expected: the new test fails on `esbuild.build must NOT run when the range is rejected` — the
  bundle runs before the range is rejected today.
- [ ] **Step 4: Implement — derive publishes right after the gate.** In `packages/cli/src/build.ts`,
  insert the derive/validate step immediately after the publish gate and before the typecheck gate
  (using the early `pkg` and the hoisted `s2`):
```ts
  // --- publishes ⇒ types gate (before we spend cycles on tsc/esbuild) ---
  const gate = assertPublishesTypes(pkg, absDir);
  if (!gate.ok) {
    throw new Error(`publish gate failed: ${gate.error}`);
  }

  // --- Derive + validate the publishes block BEFORE tsc/esbuild (fail fast). derivePublishes
  //     throws on a RANGE (which needs the registry — spec §4.6, §10), so a plugin with an
  //     unresolvable publishes map is rejected before it pays for a full typecheck + bundle. ---
  const s2 = pkg.s2script ?? {};
  const derivedPublishes = derivePublishes(s2.publishes, pkg.name, pkg.version, gate.typesPath);

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on error. ---
```
- [ ] **Step 5: Implement — drop the now-duplicate `s2` declaration.** Further down, the block that
  reads the manifest fields currently re-declares `s2`. Change:
```ts
  const { name, version } = pkg;
  const s2 = pkg.s2script ?? {};
  const apiVersion = s2.apiVersion ?? "";
  const pluginDependencies = s2.pluginDependencies ?? {};
  const optionalPluginDependencies = s2.optionalPluginDependencies ?? {};
  const publishes = s2.publishes;
  const config = s2.config ?? undefined;
```
  to (drop the second `const s2`; `publishes` is no longer needed here since `derivedPublishes`
  already exists):
```ts
  const { name, version } = pkg;
  const apiVersion = s2.apiVersion ?? "";
  const pluginDependencies = s2.pluginDependencies ?? {};
  const optionalPluginDependencies = s2.optionalPluginDependencies ?? {};
  const config = s2.config ?? undefined;
```
- [ ] **Step 6: Implement — reuse `derivedPublishes` at manifest assembly.** Change the manifest
  block that re-derives:
```ts
  // publishes.ts owns the grammar, including which forms resolve locally ("self", or a map with
  // a CONCRETE version naming a contract this plugin ships) versus which need the registry
  // (a RANGE against someone else's published contract — spec §4.6, §10).
  const derivedPublishes = derivePublishes(publishes, name, version, gate.typesPath);
  if (Object.keys(derivedPublishes).length > 0) {
    manifest.publishes = derivedPublishes;
  }
```
  to (reuse the value derived up top — no second derive):
```ts
  // publishes.ts owns the grammar; the block was derived + validated up front (fail fast).
  if (Object.keys(derivedPublishes).length > 0) {
    manifest.publishes = derivedPublishes;
  }
```
- [ ] **Step 7: Run the build tests, expect PASS** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/build.test.mjs`
  Expected: all pass (`# fail 0`), including the new fail-fast test (esbuild never invoked) and the
  existing `build rejects a RANGE` test.
- [ ] **Step 8: Run the CLI suite** — from `packages/cli`: `npm test` → `# fail 0`.
- [ ] **Step 9: Commit** —
```bash
git add packages/cli/src/build.ts packages/cli/test/build.test.mjs
gt create packaging-debt/publishes-fail-fast -m "fix(cli): reject a publishes range before tsc + esbuild (B3)

derivePublishes (which throws on an unresolvable RANGE) was only called after the
typecheck gate and the esbuild bundle, so a bad publishes map paid for a full
tsc + bundle before rejection. Hoist derive/validate above the expensive steps
and reuse its result at manifest assembly.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

**Gate for PR 3:** `cd packages/cli && npm test` green; the new spy test proves the range is rejected
before `esbuild.build` runs.

---

## PR 4: B2 — one contract per plugin (rejects a multi-interface `publishes` map)

**Finding (`build.ts:132`):** a multi-interface `publishes` map gives *every* entry the same
`typesSha256`, because a plugin ships exactly one `types` contract file — so the hash cannot identify
which contract belongs to which interface. **This touches the frozen manifest shape (distribution
spec §9)** — isolate it in its own PR and call out the frozen-shape change in the body.

**Decision (documented):** enforce an explicit **single-contract-per-plugin constraint** — reject a
multi-interface map with a named error — rather than inventing a per-interface hash source. Rationale:
there is exactly one `types` field per plugin (`gate.typesPath`) and no per-interface `types`
mechanism, so two interfaces sharing one file's bytes is meaningless; a distinct hash per interface
would require a bigger grammar change (a per-interface `types` map) that is out of this slice's scope.
Verified against the tree: every real `publishes` today is `"self"` or a single-entry map
(`plugins/zones`, `examples/{entref-producer,greeter-plugin}`, all fixtures), so the constraint breaks
no existing consumer. The future per-interface `types` map is logged as an open question.

### Task 4.1 — Reject a multi-interface publishes map in `derivePublishes`

**Files:**
- Modify: `packages/cli/src/publishes.ts` (`derivePublishes`, after `expandPublishes`, ~lines 76–83)
- Test: `packages/cli/test/publishes.test.mjs` (add a rejection regression)
- Test: `packages/cli/test/build.test.mjs` (optional end-to-end reject; see Step 5)

**Interfaces:**
- Consumes/Produces: `derivePublishes(...)` — unchanged signature; now throws a named error when the
  expanded map has more than one interface.

- [ ] **Step 1: Write the failing test** — append to `packages/cli/test/publishes.test.mjs`:
```js
test("derivePublishes: a two-interface map is rejected (one plugin ships one contract)", () => {
  // Two interfaces cannot share one plugin's single "types" file — the same typesSha256 could not
  // identify which contract is which. Reject with a named error rather than stamp a meaningless hash.
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  writeFileSync(p, "export declare function m(): void;\n");
  assert.throws(
    () => derivePublishes({ "@x/a": "1.0.0", "@x/b": "1.0.0" }, "@a/b", "1.0.0", p),
    /single .*contract|one interface per plugin/i,
  );
});

test("derivePublishes: a single-interface map still derives (one contract, one hash)", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  writeFileSync(p, "export declare function s(): void;\n");
  const out = derivePublishes({ "@x/only": "1.0.0" }, "@a/b", "1.0.0", p);
  assert.deepEqual(Object.keys(out), ["@x/only"]);
  assert.equal(out["@x/only"].typesSha256, hashContract(p));
});
```
- [ ] **Step 2: Run it, expect FAIL** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/publishes.test.mjs`
  Expected: the two-interface test fails — `derivePublishes` currently stamps both entries the same
  hash instead of throwing.
- [ ] **Step 3: Implement** — in `packages/cli/src/publishes.ts`, add the constraint immediately
  after the empty-check in `derivePublishes`. Change:
```ts
  const expanded = expandPublishes(authored, pkgName, pkgVersion);
  const names = Object.keys(expanded);
  if (names.length === 0) return {};
  if (typesPath === null) {
    throw new Error(
      `publishes is set but no contract .d.ts was resolved — set "types": "api.d.ts" in package.json`,
    );
  }
```
  to:
```ts
  const expanded = expandPublishes(authored, pkgName, pkgVersion);
  const names = Object.keys(expanded);
  if (names.length === 0) return {};
  // One plugin ships ONE `types` contract, so it hashes to ONE typesSha256. A multi-interface map
  // would give every entry that SAME hash, which cannot identify which contract belongs to which
  // interface (design spec 2026-07-16 §Part B/B2 — frozen-shape fix). Publish one interface per
  // plugin. A per-interface `types` map is out of scope (see the plan's open questions).
  if (names.length > 1) {
    throw new Error(
      `publishes declares ${names.length} interfaces (${names.join(", ")}) but a plugin ships a ` +
        `single "types" contract, so they would all carry the SAME typesSha256 — a hash that ` +
        `cannot say which contract is which. Publish one interface per plugin (a per-interface ` +
        `"types" map is not supported).`,
    );
  }
  if (typesPath === null) {
    throw new Error(
      `publishes is set but no contract .d.ts was resolved — set "types": "api.d.ts" in package.json`,
    );
  }
```
- [ ] **Step 4: Run it, expect PASS** — from `packages/cli`:
  `node --experimental-strip-types --no-warnings --test test/publishes.test.mjs`
  Expected: all pass (`# fail 0`), including both new tests.
- [ ] **Step 5: Run the full CLI suite** — from `packages/cli`: `npm test`
  Expected: `# fail 0` everywhere (build tests included — no fixture uses a multi-interface map, so
  none regress).
- [ ] **Step 6: Commit** —
```bash
git add packages/cli/src/publishes.ts packages/cli/test/publishes.test.mjs
gt create packaging-debt/one-contract-per-plugin -m "fix(cli): reject a multi-interface publishes map (B2)

A multi-interface publishes map gave every entry the SAME typesSha256 (one plugin
ships one 'types' file), a hash that can't identify which contract is which.
Enforce one interface per plugin with a named error.

FROZEN-SHAPE NOTE: corrects the just-frozen manifest publishes shape (distribution
spec 2026-07-15 §9). No existing consumer publishes a multi-interface map, so this
tightens the contract without breaking anyone. A per-interface 'types' map is out
of scope.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```
- [ ] **Step 7: Write the PR body with the frozen-shape callout.** Use the **Write tool** (NOT a
  heredoc — CLAUDE.md: shell escaping mangles tables/code blocks) to write this exact content to
  `/tmp/claude-1000/-home-gkh-projects-s2script/513cd495-bd10-4b41-b735-e6057bbabbe5/scratchpad/pr-b2-body.md`:
```markdown
## Stack Context
`packaging-debt` — close the six review-debt findings from `/code-review high` on the
contract-grammar stack (#36–#41).

## Why
B2: a multi-interface `publishes` map stamped every entry the same `typesSha256`, because a plugin
ships exactly one `types` contract file. The shared hash cannot identify which contract belongs to
which interface.

## Frozen-shape callout
This corrects the manifest `publishes` shape that the contract-grammar stack just froze (distribution
spec 2026-07-15 §9). The fix is a tightening: it rejects a shape that could only ever produce a
meaningless hash. Verified that no plugin, example, or fixture in the tree publishes a multi-interface
map, so no consumer regresses. A per-interface `types` map (the alternative that would allow distinct
hashes) is deliberately out of scope.

https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf
```
  Then attach it after `gt submit`: `gh pr edit <N> --body-file <that path>`.

**Gate for PR 4:** `cd packages/cli && npm test` green; the two-interface rejection is the targeted
regression, and the full suite proves no fixture regresses.

---

## PR 5: B5 — handle the 17 discarded `Result`s from the fallible publish path

**Finding (`interfaces.rs:63`):** `InterfaceRegistry::publish` now returns `Result<(), String>`, but 17
call sites in the module's test suite discard it as a bare statement — 17 `unused_must_use` warnings,
and a genuine publish failure at any of those setup calls would be silently swallowed. Make each drop
explicit with a reason (`.expect(...)`), so a setup publish that fails surfaces as a test panic rather
than a green run against a half-built registry.

Verified: exactly 17 sites (`cargo build -p s2script-core --tests` reports them) at
`interfaces.rs:257, 268, 269, 270, 290, 293, 302, 305, 312, 326, 338, 349, 356, 366, 384, 394, 396`.
All are expected-success setup publishes (the rejection paths already use `.expect_err(...)`).

### Task 5.1 — `.expect(...)` every discarded publish in the test suite

**Files:**
- Modify: `core/src/interfaces.rs` (the 17 test call sites listed above)
- Test: the module's existing `#[cfg(test)] mod tests` (these ARE the tests; the change hardens them)

**Interfaces:**
- Consumes: `InterfaceRegistry::publish(&mut self, name, version, producer_id, producer_gen, method_names) -> Result<(), String>`
- Produces: no API change; the 17 call sites now assert success instead of dropping the `Result`.

- [ ] **Step 1: Confirm the 17 warnings exist (the failing state)** — from repo root:
  `cargo build -p s2script-core --tests 2>&1 | grep -c "unused \`Result\` that must be used"`
  Expected: `17` (each of the 17 sites). Also confirm the suite is currently green:
  `cargo test -p s2script-core` → `test result: ok`.
- [ ] **Step 2: Implement** — append `.expect("test-setup publish must succeed")` to each of the 17
  bare `r.publish(...);` statements. The complete set of edits (each line's `;` becomes
  `.expect("test-setup publish must succeed");`):
  - `257`: `r.publish("@x/if", "1.2.0", "prod", 0, vec!["greet".into()]).expect("test-setup publish must succeed");`
  - `268`: `r.publish("@a", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `269`: `r.publish("@b", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `270`: `r.publish("@c", "1.0.0", "other", 0, vec![]).expect("test-setup publish must succeed");`
  - `290`: `r.publish("@hard", "1.5.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `293`: `r.publish("@opt", "2.0.0", "prod2", 0, vec![]).expect("test-setup publish must succeed");`
  - `302`: `r.publish("@x", "1.2.0", "prod", 0, vec!["greet".into()]).expect("test-setup publish must succeed");`
  - `305`: `r.publish("@x", "3.0.0", "prod", 1, vec!["greet".into()]).expect("test-setup publish must succeed");   // republished incompatible`
  - `312`: `r.publish("@x", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `326`: `r.publish("@x", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `338`: `r.publish("@x", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `349`: `r.publish("@x", "1.0.0", "prod", 3, vec![]).expect("test-setup publish must succeed");`
  - `356`: `r.publish("@x", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `366`: `r.publish("@x", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `384`: `r.publish("@x", "1.0.0", "prod", 0, vec![]).expect("test-setup publish must succeed");`
  - `394`: `r.publish("@x", "1.0.0", "prod", 0, vec!["greet".into()]).expect("test-setup publish must succeed");`
  - `396`: `r.publish("@x", "1.1.0", "prod", 0, vec!["greet".into(), "wave".into()]).expect("test-setup publish must succeed"); // in-place update`

  Note several of these lines are identical (`r.publish("@x", "1.0.0", "prod", 0, vec![]);` at 312,
  326, 338, 356, 366, 384) — apply the change to each occurrence individually (they live in different
  `#[test]` functions), not with a blanket replace-all that could touch a line you did not mean to.
- [ ] **Step 3: Confirm the warnings are gone** — from repo root:
  `cargo build -p s2script-core --tests 2>&1 | grep -c "unused \`Result\` that must be used"`
  Expected: `0`.
- [ ] **Step 4: Run the suite, expect PASS** — from repo root: `cargo test -p s2script-core`
  Expected: `test result: ok. <n> passed; 0 failed` — every publish setup still succeeds, so no
  `.expect` panics; a future silent publish failure at any site would now panic loudly.
- [ ] **Step 5: Boundary gate stays green** — `make check-boundary` → core imports no `games/*`.
- [ ] **Step 6: Commit** —
```bash
git add core/src/interfaces.rs
gt create packaging-debt/publish-result-explicit -m "test(core): assert on the fallible publish Result in interfaces tests (B5)

InterfaceRegistry::publish returns Result<(), String>, but 17 test setup call
sites dropped it as a bare statement — a silently-swallowed publish failure would
pass against a half-built registry. Make each drop explicit with .expect(...) so a
failing setup publish panics loudly. Clears 17 unused_must_use warnings.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

**Gate for PR 5:** `cargo test -p s2script-core` green + `cargo build --tests` reports zero
`unused_must_use` for `interfaces.rs` + `make check-boundary` green.

---

## PR 6: B6 — clear `PLUGIN_PUBLISHES` / `UNDECLARED_PUBLISHES` on shutdown

**Finding (`v8host.rs:8607`):** `shutdown()` clears every other registry thread_local (RESOLVERS,
CONCOMMANDS, IFACE_METHODS, IFACES, …) but leaves `PLUGIN_PUBLISHES` and `UNDECLARED_PUBLISHES`
populated — an asymmetry that lets a re-init inherit a previous cycle's publishes state for any plugin
that was `set` but never ran a normal per-plugin unload (e.g. set via the loader but never loaded).
Clear both for teardown symmetry.

### Task 6.1 — Add the two clears to `shutdown()` and pin them with a test

**Files:**
- Modify: `core/src/v8host.rs` (`shutdown()`, next to the `IFACES` clear at ~line 8610)
- Test: `core/src/v8host.rs` (`#[cfg(test)] mod frame_tests` — add one test)

**Interfaces:**
- Consumes: `PLUGIN_PUBLISHES`, `UNDECLARED_PUBLISHES` (module-private thread_locals);
  `set_plugin_publishes(...)`, `load_plugin_js(...)`, `shutdown()`.
- Produces: `shutdown()` now leaves both registries empty; no signature change.

- [ ] **Step 1: Write the failing test** — add to the `frame_tests` module in `core/src/v8host.rs`
  (it has `use super::*;`, so the private thread_locals are in scope), next to the
  `reconcile_publishes_*` tests:
```rust
    #[test]
    fn shutdown_clears_the_publishes_registries() {
        let _ = init(dummy_logger());
        // Populate PLUGIN_PUBLISHES via a `set` with no matching load (so no per-plugin unload ever
        // clears it), and UNDECLARED_PUBLISHES via a plugin that publishes an interface it never
        // declared. Both thread_locals must be non-empty going into shutdown.
        set_plugin_publishes("prod", [(
            "@x/greeter".to_string(),
            crate::loader::PublishDecl { version: "1.0.0".into(), types_sha256: "h".into() },
        )].into_iter().collect());
        set_plugin_publishes("forgetful", std::collections::HashMap::new());
        load_plugin_js("forgetful", r#"
            const { publishInterface } = require("@s2script/interfaces");
            publishInterface("@x/undeclared", { a: function () { return 1; } });
        "#, "{}");
        assert!(!PLUGIN_PUBLISHES.with(|p| p.borrow().is_empty()),
            "precondition: PLUGIN_PUBLISHES populated");
        assert!(!UNDECLARED_PUBLISHES.with(|p| p.borrow().is_empty()),
            "precondition: UNDECLARED_PUBLISHES populated");

        shutdown();

        assert!(PLUGIN_PUBLISHES.with(|p| p.borrow().is_empty()),
            "shutdown must clear PLUGIN_PUBLISHES");
        assert!(UNDECLARED_PUBLISHES.with(|p| p.borrow().is_empty()),
            "shutdown must clear UNDECLARED_PUBLISHES");
    }
```
- [ ] **Step 2: Run it, expect FAIL** — from repo root:
  `cargo test -p s2script-core shutdown_clears_the_publishes_registries`
  Expected: the test fails on `shutdown must clear PLUGIN_PUBLISHES` — the `"prod"` entry (set but
  never loaded, so never per-plugin-unloaded) survives `shutdown()`.
- [ ] **Step 3: Implement** — in `core/src/v8host.rs`, add the two clears in `shutdown()` right after
  the `IFACES` clear. Change:
```rust
    // Clear the interface registry (pure Rust, no V8 handles; cleared for re-init hygiene).
    IFACES.with(|r| r.borrow_mut().clear());
    // Reset the subscription-id allocator for a clean slate (symmetric with TimerQueue::new()).
    NEXT_SUB_ID.with(|c| c.set(1));
```
  to:
```rust
    // Clear the interface registry (pure Rust, no V8 handles; cleared for re-init hygiene).
    IFACES.with(|r| r.borrow_mut().clear());
    // Clear the publishes registries (pure Rust, no V8 handles): the per-plugin unload path clears
    // these per id, but a plugin that was `set` and never loaded (or a partial init) leaves an entry
    // no unload ever walks. This bulk clear is the teardown backstop, symmetric with IFACES above.
    PLUGIN_PUBLISHES.with(|p| p.borrow_mut().clear());
    UNDECLARED_PUBLISHES.with(|p| p.borrow_mut().clear());
    // Reset the subscription-id allocator for a clean slate (symmetric with TimerQueue::new()).
    NEXT_SUB_ID.with(|c| c.set(1));
```
- [ ] **Step 4: Run it, expect PASS** — from repo root:
  `cargo test -p s2script-core shutdown_clears_the_publishes_registries`
  Expected: `test result: ok. 1 passed; 0 failed`.
- [ ] **Step 5: Run the full core suite** — from repo root: `cargo test -p s2script-core`
  Expected: `test result: ok` (no other shutdown-dependent test regresses).
- [ ] **Step 6: Boundary gate stays green** — `make check-boundary`.
- [ ] **Step 7: Commit** —
```bash
git add core/src/v8host.rs
gt create packaging-debt/shutdown-clear-publishes -m "fix(core): clear the publishes registries on shutdown (B6)

shutdown() cleared every other registry thread_local but left PLUGIN_PUBLISHES and
UNDECLARED_PUBLISHES populated, so a re-init could inherit prior-cycle publishes
state for a plugin that was set but never per-plugin-unloaded. Add the two bulk
clears next to the IFACES clear for teardown symmetry.

Claude-Session: https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf"
```

**Gate for PR 6:** `cargo test -p s2script-core` green (the new shutdown test is the proof) +
`make check-boundary` green.

---

## Submitting the stack

- [ ] **Restack + submit** — from repo root: `gt restack` then `gt submit --no-interactive`.
- [ ] **Set the B2 PR body** — `gh pr edit <B2-PR-number> --body-file /tmp/pr-b2-body.md` (the
  frozen-shape callout from Task 4.1 Step 7). Give every other PR a short body with **Stack Context**
  (the `packaging-debt` stack) and **Why** (the one finding), each ending with
  `https://claude.ai/code/session_018bx2t1BsWSqLa9PKUXj2Hf`.
- [ ] **Confirm each PR passed its own gate** — the stack was built PR-by-PR; re-run the per-PR gate
  from each PR's "Gate for PR N" line if any rebase touched it.
