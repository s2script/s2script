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
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(here, "..", "..", "..");   // test/ → cli/ → packages/ → repo
const packagesDir = join(repoRoot, "packages");

test("build produces a .s2sp with derived manifest + cjs plugin.js", async () => {
  const out = await buildPlugin("test/fixtures/hello", packagesDir);

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
    js.includes('require("@s2script/frame")'),
    `@s2script/* must be left external as a cjs require — got:\n${js.slice(0, 500)}`
  );
});

test("consumer manifest carries both dep maps and externalizes the inter-plugin dep", async () => {
  const out = await buildPlugin(join(here, "fixtures", "consumer"), packagesDir);
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  assert.equal(manifest.pluginDependencies["@demo/greeter"], "^1.0.0");
  assert.equal(manifest.optionalPluginDependencies["@demo/extra"], "^1.0.0");
  const js = zip.readAsText("plugin.js");
  assert.match(js, /require\(["']@demo\/greeter["']\)/); // kept external, not bundled
});
