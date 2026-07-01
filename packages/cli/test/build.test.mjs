/**
 * TDD test: build produces a .s2sp with derived manifest + cjs plugin.js
 *
 * Run via: node --experimental-strip-types --no-warnings test/build.test.mjs
 * (invoked by `npm test`)
 */

import { test } from "node:test";
import assert from "node:assert";
import { buildPlugin } from "../src/build.ts";
import AdmZip from "adm-zip";

test("build produces a .s2sp with derived manifest + cjs plugin.js", async () => {
  const out = await buildPlugin("test/fixtures/hello");

  const zip = new AdmZip(out);

  // manifest.json must exist and have the derived fields
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  assert.equal(manifest.id, "@demo/hello", "manifest.id should be the package name");
  assert.ok(manifest.apiVersion, "manifest.apiVersion should be truthy");
  assert.equal(manifest.apiVersion, "1.x", "manifest.apiVersion should match s2script.apiVersion");
  assert.equal(manifest.version, "0.1.0", "manifest.version should match package version");

  // plugin.js must exist and be a CJS bundle with @s2script/* left as external require()
  const js = zip.readAsText("plugin.js");
  assert.ok(
    js.includes('require("@s2script/std")'),
    `@s2script/* must be left external as a cjs require — got:\n${js.slice(0, 500)}`
  );
});
