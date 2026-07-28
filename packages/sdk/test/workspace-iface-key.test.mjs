/**
 * The sibling matching key (design spec 2026-07-27 §5.3.0, decision #10): a declared dependency
 * resolves to a workspace sibling **iff that sibling publishes an interface of that name** — never
 * because a sibling's PACKAGE name happens to equal it.
 *
 * The fixture is the shape that makes the two indistinguishable everywhere else: `@fixture/ik-mce`
 * publishes `@fixture/ik-mapchooser` (the map form the spec exists for), while a consumer declares
 * BOTH names — one as a registry dependency it keeps a verified copy of, one as the real sibling
 * interface. The two contracts are deliberately the same SHAPE and different BYTES, so a build that
 * resolves the wrong one is still green and the ONLY visible difference is the `compiledAgainst`
 * hash. That is exactly how this went unnoticed, so it is exactly what these tests assert.
 *
 * What the engine says, and why it is the authority here — `core/src/loader.rs:175`:
 *
 *     fn deps_satisfied(manifest: &Manifest) -> bool {
 *         manifest.plugin_dependencies.keys().all(|n| crate::v8host::iface_published(n))
 *     }
 *
 * A dependency key is an INTERFACE name at load time, so it must be one at build time.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/workspace-iface-key.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { openZip } from "./zip.mjs";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { resolveSiblingContracts } from "../src/workspace/siblings.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const root = join(here, "fixtures", "ws-iface-key");

const mceDir = join(root, "plugins", "mce");
const consumerDir = join(root, "plugins", "consumer");
const siblingContract = join(mceDir, "api.d.ts");
const verifiedCopy = join(consumerDir, ".s2script", "types", "@fixture", "ik-mce", "index.d.ts");
const sha256 = (p) => createHash("sha256").update(readFileSync(p)).digest("hex");

test("a dependency naming the PUBLISHED INTERFACE resolves to the sibling that publishes it", () => {
  const { siblings } = resolveSiblingContracts(consumerDir, ["@fixture/ik-mapchooser"]);
  const sib = siblings.get("@fixture/ik-mapchooser");
  assert.ok(sib, "the map form is what the interface-name key exists for");
  assert.equal(sib.typesPath, siblingContract, "the PRODUCER's own contract, never a copy");
  assert.equal(sib.name, "@fixture/ik-mce", "the producer's package name differs from the interface");
});

test("a dependency naming a sibling's PACKAGE name is a REGISTRY dependency, not a sibling", () => {
  // F2/§5.3.0's first silent failure: accepted as a sibling, the build goes green — but
  // `iface_published("@fixture/ik-mce")` is never true at load, so the consumer parks in `waiting`
  // forever and every ctx.use throws InterfaceUnavailable.
  const { siblings, shadowedCopies } = resolveSiblingContracts(consumerDir, ["@fixture/ik-mce"]);
  assert.equal(siblings.size, 0, "the sibling publishes @fixture/ik-mapchooser, not this name");
  assert.deepEqual(shadowedCopies, [], "and so the verified copy is NOT shadowed — it still wins");
});

test("the consumer typechecks: the copy for one dep, the sibling's own contract for the other", () => {
  const r = typecheckPlugin(consumerDir, { packagesDir });
  assert.equal(r.ok, true, JSON.stringify(r.diagnostics, null, 2));
});

test("the sibling contract is IN FORCE under its interface name (not stubbed to any)", () => {
  // There is no node_modules symlink named after the INTERFACE — npm links by package name — so
  // this also proves the explicit `paths` entry that a decoupled interface name needs.
  const src = join(consumerDir, "src", "plugin.ts");
  const good = readFileSync(src, "utf8");
  writeFileSync(src,good.replace('chooser.nominate("de_nuke")', "chooser.nominate(42)"));
  try {
    const r = typecheckPlugin(consumerDir, { packagesDir });
    assert.equal(r.ok, false, "a wrong argument type against the sibling contract must fail");
    assert.ok(r.diagnostics.some((d) => d.code === 2345), "expects TS2345 argument-type error");
  } finally {
    writeFileSync(src,good);
  }
});

test("compiledAgainst hashes the RIGHT contract for each dependency", async () => {
  const out = await buildPlugin(consumerDir, packagesDir);
  const manifest = JSON.parse(openZip(out).readAsText("manifest.json"));
  assert.deepEqual(manifest.compiledAgainst, {
    // The package-named dep: the verified copy stays authoritative (§11's compatibility hinge).
    "@fixture/ik-mce": sha256(verifiedCopy),
    // The interface-named dep: the producer's own bytes, so loader.rs:192's drift check passes for
    // structural reasons rather than by luck.
    "@fixture/ik-mapchooser": sha256(siblingContract),
  });
  assert.notEqual(
    manifest.compiledAgainst["@fixture/ik-mce"],
    sha256(siblingContract),
    "keying on the package name hashed a contract describing a DIFFERENT interface",
  );
});

test("the producer's published typesSha256 is what the consumer compiled against", async () => {
  const producerOut = await buildPlugin(mceDir, packagesDir);
  const producer = JSON.parse(openZip(producerOut).readAsText("manifest.json"));
  const consumerOut = await buildPlugin(consumerDir, packagesDir);
  const consumer = JSON.parse(openZip(consumerOut).readAsText("manifest.json"));
  assert.equal(
    consumer.compiledAgainst["@fixture/ik-mapchooser"],
    producer.publishes["@fixture/ik-mapchooser"].typesSha256,
  );
});
