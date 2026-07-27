import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, rmSync, cpSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { openZip } from "./zip.mjs";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { localContractPath } from "../src/contracts.ts";

const here = dirname(fileURLToPath(import.meta.url));
const fixture = join(here, "fixtures", "consumer-verified");
const packagesDir = join(here, "..", "..");
const contractFile = join(fixture, ".s2script", "types", "@demo", "greeter", "index.d.ts");

test("localContractPath resolves the verified copy and refuses traversal", () => {
  assert.equal(localContractPath(fixture, "@demo/greeter"), contractFile);
  assert.equal(localContractPath(fixture, "@demo/absent"), null);
  assert.equal(localContractPath(fixture, "../evil"), null);
  assert.equal(localContractPath(fixture, "@demo/.."), null);
});

test("build emits compiledAgainst = sha256 of the verified copy's raw bytes", async () => {
  const out = await buildPlugin(fixture, packagesDir);
  const zip = openZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  const expected = createHash("sha256").update(readFileSync(contractFile)).digest("hex");
  assert.deepEqual(manifest.compiledAgainst, { "@demo/greeter": expected });
});

test("the verified copy replaces the any-stub: misuse of the contract FAILS the typecheck", () => {
  // Mutates its own COPY of the fixture, never the shared one.
  //
  // This test breaks the fixture's source on purpose and restores it in a `finally`. The node test
  // runner runs test FILES in parallel, and publish-scan.test.mjs typechecks this same fixture and
  // asserts it compiles — so while the source was broken, that unrelated test failed. It reproduced
  // roughly one run in three and passed 4/4 when its file was run alone, which is the signature of
  // shared mutable state rather than a defect in either test.
  const work = mkdtempSync(join(tmpdir(), "s2s-compiled-against-"));
  try {
    cpSync(fixture, work, { recursive: true });
    // g.greet(42) is fine against an `any` stub; against the real contract it is TS2345.
    const src = join(work, "src", "plugin.ts");
    const good = readFileSync(src, "utf8");
    writeFileSync(src, good.replace('g.greet("world")', "g.greet(42 as unknown as number)"));
    const r = typecheckPlugin(work, { packagesDir });
    assert.equal(r.ok, false, "wrong arg type against the verified contract must fail");
    assert.ok(r.diagnostics.some((d) => d.code === 2345), "expects TS2345 argument-type error");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
