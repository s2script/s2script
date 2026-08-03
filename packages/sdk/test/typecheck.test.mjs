import { test } from "node:test";
import assert from "node:assert";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixtures = join(here, "fixtures", "typecheck");
const fakePkgs = join(fixtures, "fake-packages");
// A SEPARATE, isolated packagesDir for the gamePackageDeclarationFiles regression test below: its
// sdk/plugin.d.ts is a deliberately minimal PluginContext (just `id`), and adding that to the
// SHARED fake-packages/sdk/ would shadow the real PluginContext members (e.g. `tryUse`) for every
// other fixture that resolves it — see fake-packages-hooks/sdk/globals.d.ts for the full story.
const fakePkgsHooks = join(fixtures, "fake-packages-hooks");

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

test("canary: a deliberate builtin type error still FAILS (legacy @s2script/entity)", () => {
  const r = typecheckPlugin(join(fixtures, "canary-legacy"), { packagesDir: fakePkgs });
  assert.equal(r.ok, false, "legacy canary must fail — green means resolution degraded to any");
  assert.ok(r.diagnostics.some((d) => d.code === 2322),
    "expected TS2322: " + JSON.stringify(r.diagnostics));
});

test("canary: a deliberate builtin type error still FAILS (consolidated @s2script/sdk/entity)", () => {
  const r = typecheckPlugin(join(fixtures, "canary-sdk"), { packagesDir: fakePkgs });
  assert.equal(r.ok, false, "sdk canary must fail — green means resolution degraded to any");
  assert.ok(r.diagnostics.some((d) => d.code === 2322),
    "expected TS2322: " + JSON.stringify(r.diagnostics));
});

test("acceptance: a builtin TYPO yields TS2307, not any", () => {
  const r = typecheckPlugin(join(fixtures, "typo-builtin"), { packagesDir: fakePkgs });
  assert.equal(r.ok, false);
  assert.ok(r.diagnostics.some((d) => d.code === 2307),
    "expected TS2307 for @s2script/sdk/frmae: " + JSON.stringify(r.diagnostics));
});

test("narrowed filter: a declared @s2script/sdk/* typo still yields TS2307 (never stubs)", () => {
  const r = typecheckPlugin(join(fixtures, "decl-builtin-typo"), { packagesDir: fakePkgs });
  assert.equal(r.ok, false);
  assert.ok(r.diagnostics.some((d) => d.code === 2307),
    "@s2script/sdk/* must resolve-or-error, never stub: " + JSON.stringify(r.diagnostics));
});

test("a plugin declaring @s2script/cs2 sees the game package's ctx augmentation with NO explicit import from it (gamePackageDeclarationFiles)", () => {
  // Regression guard for a real bug found while building the declarative-inbound-hooks ctx codegen:
  // hooks.generated.d.ts's `declare module "@s2script/sdk/plugin" { interface PluginContext {...} }`
  // is invisible to the program unless something reaches the file via an import chain — exactly the
  // reachability problem `.s2script/gamedata.d.ts` (generatedDeclarationFiles) already exists for.
  // Every REAL cs2 plugin in this repo also imports Player/Pawn/etc., which happens to reach the
  // file anyway — so this fixture, which imports NOTHING by name from "@s2script/cs2", is the only
  // thing that would catch gamePackageDeclarationFiles regressing.
  const r = typecheckPlugin(join(fixtures, "game-ctx-only"), { packagesDir: fakePkgsHooks });
  assert.deepEqual(r.diagnostics, [], "no diagnostics: " + JSON.stringify(r.diagnostics));
  assert.equal(r.ok, true);
});

test("acceptance: an unfetched interface typo stays any (correctly indistinguishable)", () => {
  const r = typecheckPlugin(join(fixtures, "typo-interface"), { packagesDir: fakePkgs });
  assert.deepEqual(r.diagnostics, [], "interface typo must stub to any, not error: "
    + JSON.stringify(r.diagnostics));
  assert.equal(r.ok, true);
});
