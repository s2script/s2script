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
