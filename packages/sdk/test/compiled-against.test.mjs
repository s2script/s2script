import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import AdmZip from "adm-zip";
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
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  const expected = createHash("sha256").update(readFileSync(contractFile)).digest("hex");
  assert.deepEqual(manifest.compiledAgainst, { "@demo/greeter": expected });
});

test("the verified copy replaces the any-stub: misuse of the contract FAILS the typecheck", () => {
  // g.greet(42) is fine against an `any` stub; against the real contract it is TS2345.
  const src = join(fixture, "src", "plugin.ts");
  const good = readFileSync(src, "utf8");
  writeFileSync(src, good.replace('g.greet("world")', "g.greet(42 as unknown as number)"));
  try {
    const r = typecheckPlugin(fixture, { packagesDir });
    assert.equal(r.ok, false, "wrong arg type against the verified contract must fail");
    assert.ok(r.diagnostics.some((d) => d.code === 2345), "expects TS2345 argument-type error");
  } finally {
    writeFileSync(src, good);
  }
});
