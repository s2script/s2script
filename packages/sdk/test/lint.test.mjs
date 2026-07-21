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
