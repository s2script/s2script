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
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

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

test("build derives publishes {version, typesSha256} and embeds the contract", async () => {
  const out = await buildPlugin("test/fixtures/publisher", packagesDir);
  const zip = new AdmZip(out);

  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  const decl = manifest.publishes["@demo/publisher"];
  assert.ok(decl, "self sugar must expand to a self-named entry");
  assert.equal(decl.version, "2.1.0", "self takes the package version");

  const contractBytes = readFileSync("test/fixtures/publisher/api.d.ts");
  const expected = createHash("sha256").update(contractBytes).digest("hex");
  assert.equal(decl.typesSha256, expected, "hash is of the contract's raw bytes");

  // The embedded verified copy: redundant, hash-checked, never authoritative.
  // The member name is the interface name under the repo's standard sanitizer
  // ([^a-zA-Z0-9._-] → "_"), the same one that names the .s2sp: @demo/publisher → _demo_publisher.
  const embedded = zip.readFile("types/_demo_publisher.d.ts");
  assert.ok(embedded, "a publishing plugin embeds its contract");
  assert.equal(
    createHash("sha256").update(embedded).digest("hex"),
    decl.typesSha256,
    "the embedded copy must hash to the manifest's typesSha256",
  );
});

test("build of a non-publishing plugin has no publishes block and no types member", async () => {
  const out = await buildPlugin("test/fixtures/hello", packagesDir);
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));
  assert.equal(manifest.publishes, undefined, "no publishes block when nothing is published");
  assert.equal(zip.getEntries().filter((e) => e.entryName.startsWith("types/")).length, 0);
});

test("build rejects a RANGE — resolving one against a published contract needs the registry", async () => {
  // publisher-mapform declares {"@community/contract": "^1.0.0"} — a range means "resolve me
  // against someone else's contract and hash THEIR bytes", which has no local answer.
  await assert.rejects(
    () => buildPlugin("test/fixtures/publisher-mapform", packagesDir),
    /is a RANGE/,
  );
});

test("build accepts a CONCRETE map value naming an interface the package does not share a name with", async () => {
  // The decoupling the grammar exists for: @demo/renamer publishes @demo/other-name@1.0.0.
  // Concrete + a contract the plugin ships itself ⇒ resolvable with no registry.
  const out = await buildPlugin("test/fixtures/publisher-renamed", packagesDir);
  const zip = new AdmZip(out);
  const manifest = JSON.parse(zip.readAsText("manifest.json"));

  const decl = manifest.publishes["@demo/other-name"];
  assert.ok(decl, "the interface name, not the package name, is the manifest key");
  assert.equal(manifest.publishes["@demo/renamer"], undefined, "the package name is not a key");
  assert.equal(decl.version, "1.0.0", "the contract's version, not the package's 4.2.0");

  const expected = createHash("sha256")
    .update(readFileSync("test/fixtures/publisher-renamed/api.d.ts"))
    .digest("hex");
  assert.equal(decl.typesSha256, expected);
  assert.ok(zip.readFile("types/_demo_other-name.d.ts"), "embeds under the INTERFACE name");
});
