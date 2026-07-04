# Slice 5E.1 — the `tsc` typecheck gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `s2script build` typecheck each plugin (full `strict`) against the shipped engine `.d.ts` and FAIL the build (emit no `.s2sp`) on any type error.

**Architecture:** A new `packages/cli/src/typecheck/` unit runs the TypeScript compiler API in-process (`--noEmit`, `strict`) with `paths` mapping `@s2script/*` → `packages/*/index.d.ts`, a shipped `packages/globals/globals.d.ts` modelling the injected `console` global, and a temp ambient `.d.ts` stubbing inter-plugin deps as `any`. `buildPlugin` calls it FIRST. The examples become a forcing function; a `check-examples-typecheck.sh` CI gate joins the `check-*` suite.

**Tech Stack:** TypeScript compiler API (new `typescript` dep, externalized in the CLI bundle), esbuild (existing), `node:test`.

**Spec:** `docs/superpowers/specs/2026-07-03-slice-5e1-tsc-typecheck-gate-design.md`.

## Global Constraints

- **Full `strict`** — `compilerOptions.strict = true` is the FIXED baseline (a plugin cannot loosen it). It is the shipped contract, not a per-plugin preference.
- **The gate produces no `.s2sp` on a type error** — `buildPlugin` typechecks FIRST and throws formatted diagnostics; the CLI exits non-zero; no archive is written.
- **Model the sandbox, not the browser/node** — `lib: ["lib.es2020.d.ts"]`, `types: []`; the injected `console` comes from a shipped `globals.d.ts`, NEVER `lib: dom` (no `window`/`document`).
- **`@s2script/*` resolves to the shipped `.d.ts`** via `paths: { "@s2script/*": ["*/index.d.ts"] }` + `baseUrl: <packagesDir>`. Inter-plugin `pluginDependencies` resolve to `any` via an ambient `declare module "<dep>";` stub (full inter-plugin typing deferred).
- **`packagesDir` is passed explicitly** to `typecheckPlugin`/`buildPlugin` (the codegen pattern — `cli.ts` computes `repoRoot` from `import.meta.url` and passes `join(repoRoot, "packages")`), so tests can point at a fixture `packages` dir.
- **Commit trailer:** every commit message ends with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn`.
- **Test runner:** `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs`. No Docker live gate this slice (build-tool behaviour).

## File Structure

| File | Create/Modify | Responsibility |
|---|---|---|
| `packages/globals/package.json` + `packages/globals/globals.d.ts` | Create | Ambient global `console` (the injected-sandbox globals; a script/global `.d.ts`, no import/export) |
| `packages/console/index.d.ts` | Modify | Lib-free `console` shape (drop `typeof globalThis.console`) |
| `packages/cli/src/typecheck/typecheck.ts` | Create | `typecheckPlugin(dir,{packagesDir})` + `formatDiagnostics` |
| `packages/cli/package.json` | Modify | Add `typescript` dependency |
| `packages/cli/build.mjs` | Modify | Mark `typescript` external in the CLI bundle |
| `packages/cli/src/build.ts` | Modify | `buildPlugin(dir, packagesDir)` — typecheck first, throw on failure |
| `packages/cli/src/cli.ts` | Modify | Pass `join(repoRoot,"packages")` into `buildPlugin` |
| `packages/cli/test/typecheck.test.mjs` + `packages/cli/test/fixtures/typecheck/**` | Create | Hermetic unit tests (fake packages + clean/broken plugin fixtures) |
| `examples/*/src/*.ts` (+ maybe a `.d.ts`) | Modify | Make every example pass full strict |
| `scripts/check-examples-typecheck.sh` | Create | CI gate: every example typechecks |
| `README.md` / `CLAUDE.md` | Modify | Document the gate |

---

## Task 1: The typecheck unit + shipped globals

**Files:**
- Create: `packages/globals/package.json`, `packages/globals/globals.d.ts`, `packages/cli/src/typecheck/typecheck.ts`, `packages/cli/test/typecheck.test.mjs`, `packages/cli/test/fixtures/typecheck/**`
- Modify: `packages/console/index.d.ts`, `packages/cli/package.json`, `packages/cli/build.mjs`

**Interfaces:**
- Produces: `typecheckPlugin(pluginDir: string, opts: { packagesDir: string }): { ok: boolean, diagnostics: TypecheckDiag[] }` where `TypecheckDiag = { file: string; line: number; col: number; code: number; message: string }`; and `formatDiagnostics(diags): string`.

- [ ] **Step 1: Add the shipped globals + fix the console package**

Create `packages/globals/globals.d.ts` (a GLOBAL/script `.d.ts` — NO `import`/`export`, so `console` is a global):

```typescript
// @s2script/globals — ambient declarations for the globals the engine injects into EVERY plugin
// context. NO runtime code, NO import/export (this is a global/script .d.ts). Included by the
// typecheck gate as a root file so plugins that use these globals WITHOUT importing type-check against
// the real sandbox — NOT lib.dom (the sandbox has no window/document/etc.).

/** The engine-injected console (a subset of the browser/node console). */
declare const console: {
  log(...data: any[]): void;
  error(...data: any[]): void;
  warn(...data: any[]): void;
  info(...data: any[]): void;
};
```

Create `packages/globals/package.json`:

```json
{
  "name": "@s2script/globals",
  "version": "0.1.0",
  "types": "globals.d.ts"
}
```

Modify `packages/console/index.d.ts` — replace `typeof globalThis.console` with a self-contained shape (lib-free):

```typescript
/**
 * @s2script/console — author-time type stubs for the engine console.
 * NO runtime code: the engine injects the implementation at load time.
 */

/** Engine-provided console (log/error/warn/info). Also available as the global `console`. */
export declare const console: {
  log(...data: any[]): void;
  error(...data: any[]): void;
  warn(...data: any[]): void;
  info(...data: any[]): void;
};
```

- [ ] **Step 2: Add the `typescript` dep + externalize it in the CLI bundle**

In `packages/cli/package.json` `dependencies`, add:

```json
    "typescript": "^5.6.0",
```

In `packages/cli/build.mjs`, add `"typescript"` to the `external` array:

```javascript
  external: ["esbuild", "adm-zip", "typescript"],
```

Then install it:

Run: `cd packages/cli && npm install`
Expected: `typescript` appears in `packages/cli/node_modules/typescript`.

- [ ] **Step 3: Write the hermetic fixtures**

Create a fake `packages` dir + two plugin fixtures under `packages/cli/test/fixtures/typecheck/`:

`fake-packages/globals/globals.d.ts` (copy the Step-1 globals content).
`fake-packages/cs2/index.d.ts`:

```typescript
export interface Player { readonly slot: number; readonly health: number | null; }
export declare const Player: { fromSlot(slot: number): Player | null; all(): Player[]; };
```

`clean/package.json`:

```json
{ "name": "@fix/clean", "version": "1.0.0", "main": "src/plugin.ts",
  "s2script": { "apiVersion": "1.x", "pluginDependencies": { "@other/dep": "^1.0.0" } } }
```

`clean/src/plugin.ts` (uses `@s2script/cs2`, the global `console`, and an inter-plugin dep — all must resolve; null-guards the `number | null`):

```typescript
import { Player } from "@s2script/cs2";
import dep from "@other/dep";
export function onLoad(): void {
  const p = Player.fromSlot(0);
  if (p && p.health !== null) console.log("hp=" + p.health);
  console.log("dep=" + String(dep));
}
```

`broken/package.json`:

```json
{ "name": "@fix/broken", "version": "1.0.0", "main": "src/plugin.ts", "s2script": { "apiVersion": "1.x" } }
```

`broken/src/plugin.ts` (line 3 is a strict error — assigning `number | null` to `number`):

```typescript
import { Player } from "@s2script/cs2";
export function onLoad(): void {
  const hp: number = Player.fromSlot(0)!.health;   // TS2322: Type 'number | null' is not assignable to 'number'
  console.log(hp);
}
```

- [ ] **Step 4: Write the failing test (`packages/cli/test/typecheck.test.mjs`)**

```javascript
import { test } from "node:test";
import assert from "node:assert";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "fixtures", "typecheck");
const fakePkgs = join(fixtures, "fake-packages");

test("clean plugin type-checks (resolves @s2script/*, global console, inter-plugin dep)", () => {
  const r = typecheckPlugin(join(fixtures, "clean"), { packagesDir: fakePkgs });
  assert.deepEqual(r.diagnostics, [], "no diagnostics: " + JSON.stringify(r.diagnostics));
  assert.equal(r.ok, true);
});

test("broken plugin fails with a diagnostic at the offending line", () => {
  const r = typecheckPlugin(join(fixtures, "broken"), { packagesDir: fakePkgs });
  assert.equal(r.ok, false);
  assert.ok(r.diagnostics.length >= 1, "expected >= 1 diagnostic");
  assert.ok(r.diagnostics.some((d) => d.code === 2322 && d.line === 3),
    "expected TS2322 at line 3: " + JSON.stringify(r.diagnostics));
});
```

- [ ] **Step 5: Run to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs`
Expected: FAIL — `Cannot find module '../src/typecheck/typecheck.ts'`.

- [ ] **Step 6: Implement `packages/cli/src/typecheck/typecheck.ts`**

```typescript
import ts from "typescript";
import { readFileSync, writeFileSync, rmSync, mkdtempSync } from "node:fs";
import { join, resolve } from "node:path";
import { tmpdir } from "node:os";

export interface TypecheckDiag { file: string; line: number; col: number; code: number; message: string; }
export interface TypecheckResult { ok: boolean; diagnostics: TypecheckDiag[]; }

/** Typecheck a plugin dir (full strict) against the shipped engine .d.ts under `packagesDir`.
 *  @s2script/* -> packagesDir/<name>/index.d.ts; the global `console` -> packagesDir/globals/globals.d.ts;
 *  each declared pluginDependency -> an ambient `declare module "<dep>";` (any). Never emits. */
export function typecheckPlugin(pluginDir: string, opts: { packagesDir: string }): TypecheckResult {
  const absDir = resolve(pluginDir);
  const pkg = JSON.parse(readFileSync(join(absDir, "package.json"), "utf8"));
  const s2 = pkg.s2script ?? {};
  const entryRel = s2.main ?? pkg.main;
  if (!entryRel) throw new Error(`typecheckPlugin: no entry point in ${join(absDir, "package.json")}`);
  const entry = resolve(absDir, entryRel);
  const deps = [
    ...Object.keys(s2.pluginDependencies ?? {}),
    ...Object.keys(s2.optionalPluginDependencies ?? {}),
  ];

  const options: ts.CompilerOptions = {
    strict: true,
    noEmit: true,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2020,
    lib: ["lib.es2020.d.ts"],
    types: [],
    baseUrl: opts.packagesDir,
    paths: { "@s2script/*": ["*/index.d.ts"] },
    skipLibCheck: true,
  };

  const rootNames = [entry, join(opts.packagesDir, "globals", "globals.d.ts")];
  const tmp = mkdtempSync(join(tmpdir(), "s2tc-"));
  try {
    if (deps.length) {
      const stub = join(tmp, "ambient.d.ts");
      writeFileSync(stub, deps.map((d) => `declare module ${JSON.stringify(d)};`).join("\n") + "\n");
      rootNames.push(stub);
    }
    const program = ts.createProgram(rootNames, options);
    const diags = [
      ...program.getSyntacticDiagnostics(),
      ...program.getSemanticDiagnostics(),
      ...program.getGlobalDiagnostics(),
    ];
    const out: TypecheckDiag[] = diags.map((d) => {
      let file = "?", line = 0, col = 0;
      if (d.file && d.start !== undefined) {
        const lc = d.file.getLineAndCharacterOfPosition(d.start);
        file = d.file.fileName; line = lc.line + 1; col = lc.character + 1;
      }
      return { file, line, col, code: d.code, message: ts.flattenDiagnosticMessageText(d.messageText, "\n") };
    });
    return { ok: out.length === 0, diagnostics: out };
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
}

export function formatDiagnostics(diags: TypecheckDiag[]): string {
  return diags.map((d) => `  ${d.file}:${d.line}:${d.col} — TS${d.code}: ${d.message}`).join("\n");
}
```

- [ ] **Step 7: Run to verify it passes**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/typecheck.test.mjs`
Expected: PASS (both tests).

If the `clean` test reports "cannot find module '@s2script/cs2'" (TS2307), the `paths`/`moduleResolution` pairing needs adjustment — try `moduleResolution: ts.ModuleResolutionKind.NodeNext` with `module: ts.ModuleKind.NodeNext`. The test is the arbiter; iterate until `clean` resolves and `broken` reports TS2322.

- [ ] **Step 8: Run the full CLI suite + commit**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs`
Expected: all pass (existing + the 2 new).

```bash
git add packages/globals packages/console/index.d.ts packages/cli/src/typecheck packages/cli/package.json packages/cli/package-lock.json packages/cli/build.mjs packages/cli/test/typecheck.test.mjs packages/cli/test/fixtures/typecheck
git commit -m "$(printf 'feat(slice5e1): typecheckPlugin (compiler API, full strict) + shipped globals.d.ts\n\nThe TypeScript compiler API, in-process: typecheckPlugin(dir,{packagesDir}) resolves @s2script/* ->\npackages/*/index.d.ts, the injected global console -> packages/globals/globals.d.ts (a script .d.ts,\nNOT lib.dom), and each pluginDependency -> an ambient declare-module (any). strict + noEmit +\nskipLibCheck. @s2script/console made lib-free. Hermetic unit tests (fake packages + clean/broken fixtures).\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 2: Wire the gate into `buildPlugin`

**Files:**
- Modify: `packages/cli/src/build.ts`, `packages/cli/src/cli.ts`
- Create: `packages/cli/test/build-typecheck.test.mjs`

**Interfaces:**
- Consumes: `typecheckPlugin`/`formatDiagnostics` (Task 1).
- Produces: `buildPlugin(dir: string, packagesDir: string)` — typechecks first; throws `Error` with formatted diagnostics (no `.s2sp`) on failure; otherwise unchanged.

- [ ] **Step 1: Write the failing integration test (`packages/cli/test/build-typecheck.test.mjs`)**

```javascript
import { test } from "node:test";
import assert from "node:assert";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { existsSync, rmSync } from "node:fs";
import { buildPlugin } from "../src/build.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "fixtures", "typecheck");
const fakePkgs = join(fixtures, "fake-packages");

test("build FAILS (no .s2sp) on a type error", async () => {
  const dist = join(fixtures, "broken", "dist");
  rmSync(dist, { recursive: true, force: true });
  await assert.rejects(() => buildPlugin(join(fixtures, "broken"), fakePkgs), /TS2322/);
  assert.equal(existsSync(join(dist, "_fix_broken.s2sp")), false, "no .s2sp on typecheck failure");
});

test("build SUCCEEDS (emits .s2sp) on a clean plugin", async () => {
  const out = await buildPlugin(join(fixtures, "clean"), fakePkgs);
  assert.ok(existsSync(out), "clean plugin emits a .s2sp");
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/build-typecheck.test.mjs`
Expected: FAIL — `buildPlugin` currently takes 1 arg + does not typecheck (the broken build succeeds / signature mismatch).

- [ ] **Step 3: Add the typecheck to `buildPlugin` (`packages/cli/src/build.ts`)**

Add the import at the top:

```typescript
import { typecheckPlugin, formatDiagnostics } from "./typecheck/typecheck.ts";
```

Change the signature + add the typecheck as the FIRST step of the function body (right after `const absDir = resolve(dir);`):

```typescript
export async function buildPlugin(dir: string, packagesDir: string): Promise<string> {
  const absDir = resolve(dir);

  // --- Typecheck gate (Slice 5E.1): full strict against the shipped engine .d.ts. No .s2sp on error. ---
  const tc = typecheckPlugin(absDir, { packagesDir });
  if (!tc.ok) {
    throw new Error(`typecheck failed (${tc.diagnostics.length} error(s)):\n${formatDiagnostics(tc.diagnostics)}`);
  }

  // --- Read package.json ---   (existing code continues unchanged) ---
```

- [ ] **Step 4: Pass `packagesDir` from `cli.ts`**

In `packages/cli/src/cli.ts`, the `build` branch currently is `console.log(await buildPlugin(arg));`. Change it to compute `repoRoot` (as the gen commands do) and pass `packages`:

```typescript
} else if (command === "build" && arg) {
  const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");   // dist/ → packages/cli → packages → repo
  try { console.log(await buildPlugin(arg, join(repoRoot, "packages"))); }
  catch (e) { console.error(String(e instanceof Error ? e.message : e)); process.exit(1); }
```

(Match the existing `try/catch`/`process.exit(1)` shape already in that branch; keep `fileURLToPath`/`dirname`/`join` imports — they're already imported for the gen commands.)

- [ ] **Step 5: Run to verify it passes**

Run: `cd packages/cli && node --experimental-strip-types --no-warnings --test test/build-typecheck.test.mjs`
Expected: PASS (broken rejects with TS2322 + no `.s2sp`; clean emits).

- [ ] **Step 6: Rebuild the CLI bundle + full suite + commit**

Run:
```bash
cd packages/cli && node build.mjs && node --experimental-strip-types --no-warnings --test test/*.test.mjs
```
Expected: `built dist/cli.js`; all tests pass.

```bash
git add packages/cli/src/build.ts packages/cli/src/cli.ts packages/cli/dist/cli.js packages/cli/test/build-typecheck.test.mjs
git commit -m "$(printf 'feat(slice5e1): buildPlugin typechecks first — no .s2sp on a type error\n\nbuildPlugin(dir, packagesDir) runs typecheckPlugin FIRST (full strict); on any diagnostic it throws the\nformatted errors and the CLI exits non-zero with no archive written. cli.ts passes join(repoRoot,\n\"packages\"). Integration tests: broken -> rejects TS2322 + no .s2sp; clean -> emits. CLI bundle rebuilt.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 3: Make every example pass full strict + the CI gate

**Files:**
- Modify: `examples/*/src/*.ts` (and possibly a `packages/*/index.d.ts` if an example reveals a `.d.ts` gap)
- Create: `scripts/check-examples-typecheck.sh`

**Interfaces:**
- Consumes: `s2script build` (now typecheck-gated).

- [ ] **Step 1: Write the CI gate `scripts/check-examples-typecheck.sh`**

```bash
#!/usr/bin/env bash
# Typecheck every example plugin against the shipped engine .d.ts (the Slice-5E.1 gate).
# Fails if any example has a type error — a .d.ts regression that breaks the examples is caught here.
set -euo pipefail
cd "$(dirname "$0")/.."
fail=0
for d in examples/*/; do
  [ -f "$d/package.json" ] || continue
  echo "=== typecheck $d ==="
  if ! node --experimental-strip-types --no-warnings -e "
    import('./packages/cli/src/typecheck/typecheck.ts').then(({typecheckPlugin, formatDiagnostics}) => {
      const r = typecheckPlugin('$d', { packagesDir: 'packages' });
      if (!r.ok) { console.error(formatDiagnostics(r.diagnostics)); process.exit(1); }
      console.log('  OK');
    });
  "; then fail=1; fi
done
[ "$fail" = 0 ] && echo "PASS: all examples typecheck" || { echo "FAIL: an example has type errors"; exit 1; }
```

Make it executable: `chmod +x scripts/check-examples-typecheck.sh`.

- [ ] **Step 2: Run the gate to surface the failures**

Run: `bash scripts/check-examples-typecheck.sh`
Expected: FAIL — one or more examples report type errors (most likely `TS2531`/`TS18047` "possibly null" on the `T | null` accessors, or `TS7006` implicit-any on an untyped param).

- [ ] **Step 3: Fix each example to pass full strict**

For each diagnostic, apply the minimal honest fix — do NOT weaken the gate or add blanket `any`:
- **"possibly null" (`TS18047`/`TS2531`)** on a `T | null` read (`pawn.health`, `Player.fromSlot`, `ev.getPlayerSlot`→then a nav, `readHandle`, etc.): add the null-guard the safety model already requires (`const p = Player.fromSlot(0); if (!p) return; …`, or `x === null ? fallback : use(x)`).
- **implicit `any` (`TS7006`)** on a callback param: add the explicit type from the `.d.ts` (e.g. `(ev: GameEvent) => …`, `(p) => …` → `(p: <PayloadType>)`).
- **"cannot find name" for an injected global**: if it's a real injected global missing from `globals.d.ts`, ADD it there (a genuine `.d.ts` gap — this is the forcing function working); otherwise import it from its `@s2script/*` package.
- **a genuinely wrong/missing type in a `.d.ts`**: fix the `.d.ts` (regenerate if it's a `*.generated.d.ts` — then the freshness gate must still pass) — an example revealing a real surface bug is the point of the slice.

Re-run `bash scripts/check-examples-typecheck.sh` after each example until it prints `PASS: all examples typecheck`.

- [ ] **Step 4: Confirm the built demo still bundles + loads (spot check)**

Run: `cd examples/demo-plugin && npx s2script build . && cd -`
Expected: prints the `.s2sp` path (the typecheck passes + esbuild bundles). (No live gate needed — the runtime is unchanged; this only confirms the gated build still produces a valid archive.)

- [ ] **Step 5: Commit**

```bash
git add examples scripts/check-examples-typecheck.sh packages/  # include any regenerated .d.ts
git commit -m "$(printf 'feat(slice5e1): all examples pass full strict + check-examples-typecheck.sh gate\n\nThe examples are the forcing function: fixed each to type-check under full strict (null-guards the\nT|null accessors, explicit callback param types) — validating the shipped .d.ts surface is usable.\nscripts/check-examples-typecheck.sh joins the check-* suite. Any .d.ts gaps an example revealed are fixed.\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Task 4: Docs + full gate sweep

**Files:**
- Modify: `README.md`, `CLAUDE.md`

- [ ] **Step 1: Document the gate**

- README: a "The typecheck gate (Slice 5E.1)" section — `s2script build` typechecks full-strict against the shipped `.d.ts`; a type error fails the build (no `.s2sp`), so a failing dev-reload leaves the running plugin untouched; inter-plugin deps are `any` for now (typed interfaces deferred).
- CLAUDE.md `## Current state`: append a 5E.1 paragraph + update `Current focus` (the typecheck gate is done; the remaining "do em all" slices are lifecycle, re-entrant fire / non-event hooks, and the base-plugin suite).

- [ ] **Step 2: Full gate sweep**

Run:
```bash
cargo test -p s2script-core -- --test-threads=1
cd packages/cli && node --experimental-strip-types --no-warnings --test test/*.test.mjs && cd -
for g in check-examples-typecheck check-nav-generated check-schema-generated check-events-generated check-core-boundary test-boundary-nameleak; do bash scripts/$g.sh >/dev/null 2>&1 && echo "$g PASS" || echo "$g FAIL"; done
```
Expected: core green; CLI green (incl. the new typecheck + build tests); all 6 gates PASS.

- [ ] **Step 3: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "$(printf 'docs(slice5e1): document the typecheck gate + update CLAUDE state\n\nClaude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn')"
```

---

## Self-Review notes (author checklist — completed)

- **Spec coverage:** §2 compiler-API → T1; §3 resolution/compile-set → T1 (options+paths); §3.1 globals → T1; §4 inter-plugin stub → T1 (temp ambient); §5 strict → T1 (`strict:true`); §6 examples forcing function → T3; §7 CI gate → T3; §8 tests → T1/T2 + T3 gate; §9 tasks → T1–T4.
- **Type consistency:** `typecheckPlugin(dir,{packagesDir})` + `formatDiagnostics` identical across T1 (def), T2 (import in build.ts), T3 (the CI script). `buildPlugin(dir, packagesDir)` signature identical in T2 (def) + T2 (cli.ts call) + T2 integration test. `TypecheckDiag` fields (`file/line/col/code/message`) used consistently in the tests + `formatDiagnostics`.
- **No placeholders:** every code step carries complete code. T3-Step-3 is intentionally method-not-verbatim (the exact example fixes are discovered by running the gate) — it names the diagnostic codes, the fix per category, and the pass condition, which is the correct shape for a forcing-function task.
