import { test } from "node:test";
import assert from "node:assert";
import { mkdtempSync, readFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { writeZip } from "./zip.mjs";
import { configFileName } from "../src/config/config-path.ts";
import { generateDefaultJsonc, genConfigForS2sp, runConfigGen } from "../src/config/gen.ts";

// Strip `//`-to-end-of-line comments (test-local; the fixtures here carry no `//` inside strings).
function stripComments(jsonc) {
  return jsonc.split("\n").map((l) => { const i = l.indexOf("//"); return i >= 0 ? l.slice(0, i) : l; }).join("\n");
}

test("configFileName matches the runtime ConfigPath sanitizer (parity table)", () => {
  const table = [
    ["@s2script/funvotes", "_s2script_funvotes.json"],
    ["a/b c", "a_b_c.json"],
    ["plain", "plain.json"],
    ["keep.dots-and_underscores", "keep.dots-and_underscores.json"],
    ["@x/y", "_x_y.json"],
    ["weird:name*here", "weird_name_here.json"],
  ];
  for (const [id, expected] of table) assert.equal(configFileName(id), expected, id);
});

test("generateDefaultJsonc emits nested JSONC for a section (reparses to defaults)", () => {
  const jsonc = generateDefaultJsonc({
    top: { type: "bool", default: true, description: "toggle" },
    sect: { inner: { type: "int", default: 5 }, deeper: { leaf: { type: "string", default: "x" } } },
  });
  const parsed = JSON.parse(stripComments(jsonc));
  assert.equal(parsed.top, true);
  assert.deepEqual(parsed.sect, { inner: 5, deeper: { leaf: "x" } });
  // Comments carry the type + description.
  assert.match(jsonc, /\/\/ bool — toggle/);
  assert.match(jsonc, /\/\/ int/);
});

test("generateDefaultJsonc skips dotted keys", () => {
  const jsonc = generateDefaultJsonc({ "a.b": { type: "string", default: "x" }, ok: { type: "int", default: 1 } });
  const parsed = JSON.parse(stripComments(jsonc));
  assert.ok(!("a.b" in parsed));
  assert.equal(parsed.ok, 1);
});

test("genConfigForS2sp reads the staged manifest and writes the sanitized filename", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-configgen-"));
  // Build a minimal .s2sp (manifest.json + plugin.js) with a config block.
  const manifest = {
    id: "@s2script/funvotes",
    version: "1.0.0",
    apiVersion: "1.0.0",
    config: { greeting: { type: "string", default: "hi" }, sect: { n: { type: "int", default: 3 } } },
  };
  const s2sp = join(dir, "funvotes.s2sp");
  writeZip(s2sp, {
    "manifest.json": Buffer.from(JSON.stringify(manifest)),
    "plugin.js": Buffer.from("module.exports = {};"),
  });

  const written = genConfigForS2sp(s2sp, dir);
  assert.equal(written, join(dir, "_s2script_funvotes.json"));
  assert.ok(existsSync(written));
  const parsed = JSON.parse(stripComments(readFileSync(written, "utf8")));
  assert.equal(parsed.greeting, "hi");
  assert.deepEqual(parsed.sect, { n: 3 });
});

test("runConfigGen skips a manifest with no config block", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-configgen-"));
  const s2sp = join(dir, "noconfig.s2sp");
  writeZip(s2sp, {
    "manifest.json": Buffer.from(JSON.stringify({ id: "noconfig", version: "1.0.0", apiVersion: "1.0.0" })),
    "plugin.js": Buffer.from("module.exports = {};"),
  });
  const { written, skipped } = runConfigGen([s2sp], dir);
  assert.equal(written.length, 0);
  assert.deepEqual(skipped, [s2sp]);
  assert.ok(!existsSync(join(dir, "noconfig.json")));
});
