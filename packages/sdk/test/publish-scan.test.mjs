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
  // `import type { Greeter } from "@demo/greeter"` is a static import of a non-@s2script specifier.
  assert.deepEqual(scan.importNames, ["@demo/greeter"]);
});

test("scanPluginProgram collects producer-as-import specifiers (no ctx.use)", () => {
  const dir = fx("consumer-import");
  const r = typecheckPlugin(dir, { packagesDir });
  assert.ok(r.ok && r.program, `fixture typechecks: ${JSON.stringify(r.diagnostics)}`);
  const scan = scanPluginProgram(r.program, dir);
  assert.deepEqual(scan.useNames, []);
  assert.deepEqual(scan.importNames, ["@demo/greeter"]);
  assert.deepEqual(scan.publishNames, []);
});

test("scanPluginProgram collects free publish/use/tryUse from @s2script/sdk/plugin", async () => {
  const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const tmp = mkdtempSync(join(tmpdir(), "s2-scan-free-"));
  mkdirSync(join(tmp, "src"), { recursive: true });
  writeFileSync(
    join(tmp, "package.json"),
    JSON.stringify({
      name: "@demo/scan-free",
      version: "1.0.0",
      main: "src/plugin.ts",
      types: "api.d.ts",
      s2script: {
        publishes: "self",
        pluginDependencies: { "@demo/greeter": "^1.0.0" },
        optionalPluginDependencies: { "@demo/extra": "^1.0.0" },
      },
    }),
  );
  writeFileSync(join(tmp, "api.d.ts"), "export declare function ping(): number;\n");
  writeFileSync(
    join(tmp, "src", "plugin.ts"),
    `import { publish, use, tryUse } from "@s2script/sdk/plugin";
export function OnPluginStart(): void {
  publish("@demo/scan-free", { ping: () => 1 });
  use("@demo/greeter");
  tryUse("@demo/extra");
}
`,
  );
  try {
    const r = typecheckPlugin(tmp, { packagesDir });
    assert.ok(r.ok && r.program, `tmp plugin typechecks: ${JSON.stringify(r.diagnostics)}`);
    const scan = scanPluginProgram(r.program, tmp);
    assert.deepEqual(scan.publishNames, ["@demo/scan-free"]);
    assert.deepEqual(scan.useNames.sort(), ["@demo/extra", "@demo/greeter"]);
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
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
