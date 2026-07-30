/**
 * TDD test: library build → .s2lib.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/build-library.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { unzipSync } from "fflate";
import { packageKind, buildLibrary } from "../src/build-library.ts";

function libDir({ kind = "library", extraFiles = {} } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "s2lib-"));
  mkdirSync(join(dir, "src"), { recursive: true });
  writeFileSync(
    join(dir, "package.json"),
    JSON.stringify({
      name: "@edge/base64",
      version: "1.2.3",
      main: "src/index.ts",
      types: "src/index.d.ts",
      s2script: { kind },
    }, null, 2),
  );
  writeFileSync(join(dir, "src", "index.ts"), `export function encode(s: string): string { return s; }\n`);
  writeFileSync(join(dir, "src", "index.d.ts"), `export declare function encode(s: string): string;\n`);
  for (const [rel, body] of Object.entries(extraFiles)) writeFileSync(join(dir, rel), body);
  return dir;
}

test("packageKind defaults to plugin and honours an explicit library", () => {
  assert.equal(packageKind({}), "plugin");
  assert.equal(packageKind({ s2script: {} }), "plugin");
  assert.equal(packageKind({ s2script: { kind: "library" } }), "library");
  assert.throws(() => packageKind({ s2script: { kind: "lib" } }), /unknown s2script\.kind/);
});

test("buildLibrary emits a .s2lib with exactly manifest.json, index.js and index.d.ts", async () => {
  const dir = libDir();
  const out = await buildLibrary(dir);
  assert.match(out, /_edge_base64\.s2lib$/);
  const entries = unzipSync(new Uint8Array(readFileSync(out)));
  assert.deepEqual(Object.keys(entries).sort(), ["index.d.ts", "index.js", "manifest.json"]);
  const manifest = JSON.parse(Buffer.from(entries["manifest.json"]).toString("utf8"));
  assert.equal(manifest.kind, "library");
  assert.equal(manifest.id, "@edge/base64");
  assert.equal(manifest.version, "1.2.3");
  assert.equal(typeof manifest.apiVersion, "string");
  assert.match(Buffer.from(entries["index.js"]).toString("utf8"), /encode/);
});

test("a library must declare types", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  delete pkg.types;
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /types/);
});

test("a library may not declare pluginDependencies", async () => {
  const dir = libDir();
  const pkgPath = join(dir, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  pkg.s2script.pluginDependencies = { "@edge/other": "^1.0.0" };
  writeFileSync(pkgPath, JSON.stringify(pkg));
  await assert.rejects(() => buildLibrary(dir), /pluginDependencies/);
});

test("@s2script/* imports stay external — the consumer's context resolves them", async () => {
  const dir = libDir();
  writeFileSync(
    join(dir, "src", "index.ts"),
    `import { Chat } from "@s2script/sdk/chat";\nexport function shout(s: string): void { Chat.toAll(s); }\n`,
  );
  const out = await buildLibrary(dir);
  const entries = unzipSync(new Uint8Array(readFileSync(out)));
  const js = Buffer.from(entries["index.js"]).toString("utf8");
  assert.match(js, /@s2script\/sdk\/chat/);
});
