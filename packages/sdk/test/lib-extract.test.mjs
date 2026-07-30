/**
 * TDD test: .s2lib extraction into the vendored tree.
 *
 * Entry names come from a possibly-compromised registry and are never trusted as paths —
 * the same doctrine registry/install.ts applies to plan filenames.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/lib-extract.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { zipSync, strToU8 } from "fflate";
import { extractLibArchive } from "../src/registry/lib-extract.ts";

const good = () =>
  Buffer.from(
    zipSync({
      "manifest.json": strToU8(JSON.stringify({ kind: "library", id: "@edge/base64", version: "1.0.0" })),
      "index.js": strToU8("module.exports.encode = (s) => s;\n"),
      "index.d.ts": strToU8("export declare function encode(s: string): string;\n"),
    }),
  );

test("writes index.js, index.d.ts and a package.json describing the vendored copy", () => {
  const out = mkdtempSync(join(tmpdir(), "s2libx-"));
  extractLibArchive(good(), out, { name: "@edge/base64", version: "1.0.0" });
  assert.match(readFileSync(join(out, "index.js"), "utf8"), /encode/);
  assert.match(readFileSync(join(out, "index.d.ts"), "utf8"), /encode/);
  const pkg = JSON.parse(readFileSync(join(out, "package.json"), "utf8"));
  assert.equal(pkg.name, "@edge/base64");
  assert.equal(pkg.version, "1.0.0");
  assert.equal(pkg.main, "index.js");
  assert.equal(pkg.types, "index.d.ts");
});

test("refuses an archive missing index.d.ts", () => {
  const bad = Buffer.from(zipSync({ "manifest.json": strToU8("{}"), "index.js": strToU8("x") }));
  assert.throws(() => extractLibArchive(bad, mkdtempSync(join(tmpdir(), "s2libx-")), { name: "x", version: "1" }), /index\.d\.ts/);
});

test("ignores unexpected entries instead of writing them", () => {
  const out = mkdtempSync(join(tmpdir(), "s2libx-"));
  const hostile = Buffer.from(
    zipSync({
      "manifest.json": strToU8("{}"),
      "index.js": strToU8("x"),
      "index.d.ts": strToU8("y"),
      "../../evil.js": strToU8("pwned"),
      "nested/thing.js": strToU8("nope"),
    }),
  );
  extractLibArchive(hostile, out, { name: "x", version: "1" });
  assert.ok(existsSync(join(out, "index.js")));
  assert.ok(!existsSync(join(out, "nested")));
  assert.ok(!existsSync(join(out, "..", "..", "evil.js")));
});
