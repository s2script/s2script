import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";

const root = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const fixtures = join(root, "packages/sdk/test/fixtures/ui-api-contract");
const packagesDir = join(root, "packages");

test("the structured UI contract typechecks alongside the legacy API", () => {
  const result = typecheckPlugin(join(fixtures, "valid"), { packagesDir });
  assert.deepEqual(result.diagnostics, [], "no diagnostics: " + JSON.stringify(result.diagnostics));
  assert.equal(result.ok, true);
});

test("the structured UI contract rejects invalid codes and result values", () => {
  const result = typecheckPlugin(join(fixtures, "invalid"), { packagesDir });
  assert.equal(result.ok, false);
  assert.deepEqual(
    result.diagnostics.map(diagnostic => ({ code: diagnostic.code, line: diagnostic.line })),
    [
      { code: 2322, line: 5 },
      { code: 2322, line: 6 },
      { code: 2739, line: 7 },
      { code: 2345, line: 10 },
    ],
  );
});
