import { test } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { openZip } from "./zip.mjs";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { scanPluginProgram } from "../src/publish-scan.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

test("scanPluginProgram collects literal ctx.publish/use names off the PluginContext type", () => {
  const dir = fx("consumer-verified"); // Task 4's fixture: one ctx.use("@demo/greeter")
  const r = typecheckPlugin(dir, { packagesDir });
  assert.ok(r.ok && r.program, "fixture typechecks and returns its program");
  const scan = scanPluginProgram(r.program, dir);
  assert.deepEqual(scan.publishNames, []);
  assert.deepEqual(scan.useNames, ["@demo/greeter"]);
  assert.deepEqual(scan.dynamicPublishSites, []);
});

test("publishes auto-derives 'self' when code publishes exactly the package name", async () => {
  const out = await buildPlugin(fx("publisher-derived-self"), packagesDir);
  const manifest = JSON.parse(openZip(out).readAsText("manifest.json"));
  assert.ok(manifest.publishes["@demo/derived-self"], "publishes derived from ctx.publish call");
  assert.equal(manifest.publishes["@demo/derived-self"].version, "1.2.0");
  assert.match(manifest.publishes["@demo/derived-self"].typesSha256, /^[0-9a-f]{64}$/);
});

test("authored publishes that disagrees with code is a build error (drift)", async () => {
  await assert.rejects(
    () => buildPlugin(fx("publisher-drift"), packagesDir),
    /publishes drift/,
  );
});

test("a non-literal ctx.publish name is a build error", async () => {
  await assert.rejects(
    () => buildPlugin(fx("publisher-dynamic"), packagesDir),
    /string literal/,
  );
});
