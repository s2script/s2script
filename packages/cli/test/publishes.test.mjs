/**
 * TDD test: the publishes grammar — "self" sugar, map form, hashing.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/publishes.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert";
import { createHash } from "node:crypto";
import { writeFileSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expandPublishes, hashContract, derivePublishes } from "../src/publishes.ts";

test("expandPublishes: 'self' becomes a single self-named entry at the package version", () => {
  const out = expandPublishes("self", "@s2script/zones", "1.2.0");
  assert.deepEqual(out, { "@s2script/zones": "1.2.0" });
});

test("expandPublishes: map form passes through unchanged", () => {
  const out = expandPublishes({ "@community/mapchooser": "^1.2.0" }, "@edge/mce", "3.1.0");
  assert.deepEqual(out, { "@community/mapchooser": "^1.2.0" });
});

test("expandPublishes: absent or empty yields no entries", () => {
  assert.deepEqual(expandPublishes(undefined, "@a/b", "1.0.0"), {});
  assert.deepEqual(expandPublishes({}, "@a/b", "1.0.0"), {});
});

test("expandPublishes: a string other than 'self' is a named error", () => {
  assert.throws(
    () => expandPublishes("mine", "@a/b", "1.0.0"),
    /publishes: the only valid string form is "self"/
  );
});

test("expandPublishes: a non-string entry value is a named error", () => {
  assert.throws(
    () => expandPublishes({ "@x/y": { version: "1.0.0" } }, "@a/b", "1.0.0"),
    /publishes\["@x\/y"\] must be a version range string/
  );
});

test("hashContract: sha256 of raw bytes, no normalization", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  // CRLF + trailing whitespace must survive: hashing the RAW bytes is the contract.
  const body = "export declare function a(): void;\r\n  \r\n";
  writeFileSync(p, body);
  const expected = createHash("sha256").update(readFileSync(p)).digest("hex");
  assert.equal(hashContract(p), expected);
  // And prove no normalization happened: an LF twin must hash differently.
  const q = join(dir, "api2.d.ts");
  writeFileSync(q, body.replace(/\r\n/g, "\n"));
  assert.notEqual(hashContract(p), hashContract(q));
});

test("derivePublishes: attaches the contract hash to every entry", () => {
  const dir = mkdtempSync(join(tmpdir(), "s2pub-"));
  const p = join(dir, "api.d.ts");
  writeFileSync(p, "export declare function z(): void;\n");
  const out = derivePublishes("self", "@s2script/zones", "1.2.0", p);
  assert.deepEqual(Object.keys(out), ["@s2script/zones"]);
  assert.equal(out["@s2script/zones"].version, "1.2.0");
  assert.equal(out["@s2script/zones"].typesSha256, hashContract(p));
});

test("derivePublishes: entries without a contract file is a named error", () => {
  assert.throws(
    () => derivePublishes("self", "@a/b", "1.0.0", null),
    /publishes is set but no contract \.d\.ts was resolved/
  );
});

test("derivePublishes: no entries yields an empty block and needs no contract", () => {
  assert.deepEqual(derivePublishes(undefined, "@a/b", "1.0.0", null), {});
});
