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

test("acceptance: an unfetched interface typo stays any (correctly indistinguishable)", () => {
  const r = typecheckPlugin(join(fixtures, "typo-interface"), { packagesDir: fakePkgs });
  assert.deepEqual(r.diagnostics, [], "interface typo must stub to any, not error: "
    + JSON.stringify(r.diagnostics));
  assert.equal(r.ok, true);
});
