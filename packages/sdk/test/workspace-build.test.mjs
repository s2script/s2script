/**
 * The three §5.3 changes inside the existing build path, plus the §5.2 preflight range gate
 * (design spec 2026-07-27).
 *
 * The load-bearing assertions here are the ones that would still pass if the feature silently
 * regressed to an `any` stub, so each is written to FAIL in that case:
 *   - a deliberate misuse of the sibling's contract must be caught (an `any` stub swallows it),
 *   - `compiledAgainst` must equal sha256 of the PRODUCER'S OWN api.d.ts (a stub emits nothing),
 *   - and the non-workspace fixture must keep byte-identical behaviour (§11's compatibility hinge).
 *
 * The two tests that plant a deliberate error edit a fixture in place and restore it in a
 * `finally`. That is safe only because tests within one file run sequentially and no other test
 * file touches `ws-contract/` or `nonws-consumer/` — the runner parallelises across FILES.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/workspace-build.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { openZip } from "./zip.mjs";
import { buildPlugin } from "../src/build.ts";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { ownConfigDir, lintPlugin } from "../src/lint/lint.ts";
import { resolveSiblingContracts } from "../src/workspace/siblings.ts";
import { loadWorkspace } from "../src/workspace/workspace.ts";
import { preflightWorkspace, preflightProblems } from "../src/workspace/preflight.ts";

const here = dirname(fileURLToPath(import.meta.url));
const packagesDir = join(here, "..", "..");
const fx = (n) => join(here, "fixtures", n);

const wsContract = fx("ws-contract");
const greeterDir = join(wsContract, "plugins", "greeter");
const consumerDir = join(wsContract, "plugins", "consumer");
const staleDir = join(wsContract, "plugins", "stale-consumer");
const producerContract = join(greeterDir, "api.d.ts");
const sha256 = (p) => createHash("sha256").update(readFileSync(p)).digest("hex");

/** Run `fn` with console.warn captured — build warnings are part of the contract here. */
async function withWarnings(fn) {
  const warnings = [];
  const original = console.warn;
  console.warn = (...args) => warnings.push(args.join(" "));
  try {
    return { value: await fn(), warnings };
  } finally {
    console.warn = original;
  }
}

// ---------------------------------------------------------------------------
// Sibling resolution (§3.2, §5.3 item 1)
// ---------------------------------------------------------------------------

test("a declared dep naming a workspace sibling resolves to the sibling's OWN types file", () => {
  const { siblings, shadowedCopies } = resolveSiblingContracts(consumerDir, ["@fixture/ws-greeter"]);
  assert.deepEqual([...siblings.keys()], ["@fixture/ws-greeter"]);
  assert.equal(siblings.get("@fixture/ws-greeter").typesPath, producerContract);
  assert.equal(siblings.get("@fixture/ws-greeter").relDir, "plugins/greeter");
  assert.deepEqual(shadowedCopies, [], "the consumer keeps no copy at all");
});

test("a dep no sibling provides is left alone — the registry path is untouched (§11)", () => {
  // Called from INSIDE a workspace: the workspace must not claim a name it does not own.
  const { siblings } = resolveSiblingContracts(consumerDir, ["@s2script/zones"]);
  assert.equal(siblings.size, 0);
});

test("outside a workspace nothing resolves — the backward-compatibility hinge", () => {
  const { siblings, shadowedCopies } = resolveSiblingContracts(fx("consumer-verified"), ["@demo/greeter"]);
  assert.equal(siblings.size, 0, "a non-member plugin keeps its verified copy / any stub");
  assert.deepEqual(shadowedCopies, []);
});

// ---------------------------------------------------------------------------
// The typecheck gate resolves the REAL contract, not an `any` stub (§3.3)
// ---------------------------------------------------------------------------

test("a workspace consumer typechecks against the sibling's real contract", () => {
  const r = typecheckPlugin(consumerDir, { packagesDir });
  assert.equal(r.ok, true, JSON.stringify(r.diagnostics, null, 2));
});

test("the sibling contract is IN FORCE: misusing it fails the typecheck (not stubbed to any)", () => {
  // g.greet(42) is fine against `declare module "@fixture/ws-greeter";`. Against the sibling's
  // real api.d.ts it is TS2345 — which is the whole proof that no stub is in play.
  const src = join(consumerDir, "src", "plugin.ts");
  const good = readFileSync(src, "utf8");
  writeFileSync(src, good.replace('g.greet("world")', "g.greet(42)"));
  try {
    const r = typecheckPlugin(consumerDir, { packagesDir });
    assert.equal(r.ok, false, "a wrong argument type against the sibling contract must fail");
    assert.ok(r.diagnostics.some((d) => d.code === 2345), "expects TS2345 argument-type error");
  } finally {
    writeFileSync(src, good);
  }
});

// ---------------------------------------------------------------------------
// compiledAgainst = the producer's own bytes (§5.3 item 2)
// ---------------------------------------------------------------------------

test("build hashes the SIBLING's own types file into compiledAgainst, and copies nothing", async () => {
  const out = await buildPlugin(consumerDir, packagesDir);
  const manifest = JSON.parse(openZip(out).readAsText("manifest.json"));
  assert.deepEqual(manifest.compiledAgainst, { "@fixture/ws-greeter": sha256(producerContract) });
  assert.ok(
    !existsSync(join(consumerDir, ".s2script", "types")),
    "no verified copy is written anywhere — one copy of the bytes is the point",
  );
});

test("the loader's drift check passes by construction: consumer hash == producer's typesSha256", async () => {
  // loader.rs:192 compares these two. In a workspace they are sha256 of the SAME file on disk,
  // so equality is structural rather than lucky (§5.3 item 2). Assert it end to end.
  const producerOut = await buildPlugin(greeterDir, packagesDir);
  const consumerOut = await buildPlugin(consumerDir, packagesDir);
  const producerManifest = JSON.parse(openZip(producerOut).readAsText("manifest.json"));
  const consumerManifest = JSON.parse(openZip(consumerOut).readAsText("manifest.json"));
  assert.equal(
    consumerManifest.compiledAgainst["@fixture/ws-greeter"],
    producerManifest.publishes["@fixture/ws-greeter"].typesSha256,
  );
});

// ---------------------------------------------------------------------------
// Precedence: the sibling beats a stale verified copy (§5.3, decision #9)
// ---------------------------------------------------------------------------

test("a stale .s2script/types copy is IGNORED in favour of the sibling, with a warning", async () => {
  const stale = join(staleDir, ".s2script", "types", "@fixture", "ws-greeter", "index.d.ts");
  assert.ok(existsSync(stale), "fixture must actually carry the pre-migration copy");

  // The copy declares greet(name: number). If it still won, this typecheck would fail.
  const tc = typecheckPlugin(staleDir, { packagesDir });
  assert.equal(tc.ok, true, JSON.stringify(tc.diagnostics, null, 2));

  const { value: out, warnings } = await withWarnings(() => buildPlugin(staleDir, packagesDir));
  const manifest = JSON.parse(openZip(out).readAsText("manifest.json"));
  assert.equal(manifest.compiledAgainst["@fixture/ws-greeter"], sha256(producerContract));
  assert.notEqual(manifest.compiledAgainst["@fixture/ws-greeter"], sha256(stale));
  const shadow = warnings.find((w) => w.includes("is IGNORED"));
  assert.ok(shadow, `expected a shadowed-copy warning, got:\n${warnings.join("\n")}`);
  assert.match(shadow, /@fixture\/ws-greeter/);
  assert.match(shadow, /can be deleted/);
});

// ---------------------------------------------------------------------------
// The two named sibling errors, at the CONSUMER's build (§11)
// ---------------------------------------------------------------------------

test("a sibling with no publishes, and one with publishes but no types, both error by name", () => {
  const badConsumer = join(fx("ws-bad-siblings"), "plugins", "consumer");
  assert.throws(
    () => resolveSiblingContracts(badConsumer, ["@fixture/bad-no-publishes", "@fixture/bad-no-types"]),
    (e) => {
      // Aggregated: BOTH offenders, not just the first (§11).
      assert.match(e.message, /@fixture\/bad-consumer cannot resolve 2 workspace sibling contracts:/);
      assert.match(e.message, /@fixture\/bad-no-publishes .* declares no s2script\.publishes/);
      // The WHY: resolution would otherwise fall through to the implementation source.
      assert.match(e.message, /IMPLEMENTATION internals/);
      assert.match(e.message, /@fixture\/bad-no-types .* declares s2script\.publishes but its contract/);
      assert.match(e.message, /"types" is missing/);
      return true;
    },
  );
});

test("that error surfaces from the consumer's own typecheck/build, not from the producer's", async () => {
  const badConsumer = join(fx("ws-bad-siblings"), "plugins", "consumer");
  await assert.rejects(() => buildPlugin(badConsumer, packagesDir), /cannot resolve 2 workspace sibling contracts/);
});

// ---------------------------------------------------------------------------
// §11's compatibility hinge: the non-workspace path is byte-identical
// ---------------------------------------------------------------------------

test("a non-workspace plugin still hashes its verified COPY, and only that, into compiledAgainst", async () => {
  // @demo/copied has a copy (real types, hashed); @demo/stubbed has none (`any`, unhashed).
  // Both must behave exactly as they did before workspaces existed.
  const fixture = fx("nonws-consumer");
  const copy = join(fixture, ".s2script", "types", "@demo", "copied", "index.d.ts");
  const out = await buildPlugin(fixture, packagesDir);
  const manifest = JSON.parse(openZip(out).readAsText("manifest.json"));
  assert.deepEqual(manifest.compiledAgainst, { "@demo/copied": sha256(copy) });
});

test("the verified copy is still IN FORCE for a non-workspace plugin (not silently any)", () => {
  const src = join(fx("nonws-consumer"), "src", "plugin.ts");
  const good = readFileSync(src, "utf8");
  writeFileSync(src, good.replace("c.ping(1)", 'c.ping("nope")'));
  try {
    const r = typecheckPlugin(fx("nonws-consumer"), { packagesDir });
    assert.equal(r.ok, false);
    assert.ok(r.diagnostics.some((d) => d.code === 2345), "expects TS2345 from the copied contract");
  } finally {
    writeFileSync(src, good);
  }
});

test("a non-workspace plugin with no copy still gets the ambient any stub (no TS2307)", () => {
  // fixtures/consumer declares a dependency it keeps no copy of. If the stub suppression leaked
  // outside workspaces this would become an unresolved-module error.
  const r = typecheckPlugin(fx("consumer"), { packagesDir });
  assert.equal(r.ok, true, JSON.stringify(r.diagnostics, null, 2));
});

// ---------------------------------------------------------------------------
// lint: the config search walks up to the workspace root (§5.3 item 3)
// ---------------------------------------------------------------------------

/** A throwaway workspace with a root flat config and one plugin containing a `debugger`. */
function tempLintWorkspace({ marker }) {
  const root = mkdtempSync(join(tmpdir(), "s2lint-"));
  const pluginDir = join(root, "plugins", "p");
  mkdirSync(join(pluginDir, "src"), { recursive: true });
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify(
      {
        name: "@fixture/lint-ws",
        private: true,
        version: "0.0.0",
        workspaces: ["plugins/*"],
        ...(marker ? { s2script: { workspace: { plugins: ["plugins/*"] } } } : {}),
      },
      null,
      2,
    ),
  );
  // Deliberately dependency-free: espree parses this TS file, so the config needs no parser.
  writeFileSync(
    join(root, "eslint.config.mjs"),
    'export default [{ files: ["**/*.ts"], rules: { "no-debugger": "error" } }];\n',
  );
  writeFileSync(
    join(pluginDir, "package.json"),
    JSON.stringify({ name: "@fixture/lint-p", version: "0.0.0", main: "src/plugin.ts" }, null, 2),
  );
  writeFileSync(join(pluginDir, "src", "plugin.ts"), "debugger;\nexport default {};\n");
  return { root, pluginDir };
}

test("ownConfigDir finds a workspace-ROOT eslint config the old plugin-dir-only check missed", () => {
  const { root, pluginDir } = tempLintWorkspace({ marker: true });
  assert.equal(ownConfigDir(pluginDir), root);
});

test("ownConfigDir stops at the plugin dir outside a workspace (today's exact behaviour)", () => {
  const { pluginDir } = tempLintWorkspace({ marker: false });
  assert.equal(ownConfigDir(pluginDir), null, "no marker ⇒ no walk-up ⇒ the canonical config runs");
});

test("lintPlugin actually ENFORCES the workspace-root config (not a silent canonical fallback)", async () => {
  const { pluginDir } = tempLintWorkspace({ marker: true });
  // The own-config path never touches the ts.Program — the config governs resolution.
  const r = await lintPlugin(pluginDir, null);
  assert.equal(r.ok, false, "the root config's no-debugger must fire");
  assert.match(r.output, /no-debugger/);
});

// ---------------------------------------------------------------------------
// §5.2 preflight
// ---------------------------------------------------------------------------

test("preflight passes a coherent workspace", () => {
  assert.deepEqual(preflightProblems(loadWorkspace(wsContract)), []);
  preflightWorkspace(loadWorkspace(wsContract)); // must not throw
});

test("preflight refuses a workspace whose sibling ranges lie, naming every one at once", () => {
  const ws = loadWorkspace(fx("ws-ranges"));
  assert.throws(
    () => preflightWorkspace(ws),
    (e) => {
      assert.match(e.message, /^2 dependency ranges do not match this workspace:/);
      assert.match(e.message, /@me\/ranks/);
      assert.match(e.message, /@me\/warmup/);
      return true;
    },
  );
});
