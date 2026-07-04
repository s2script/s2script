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
