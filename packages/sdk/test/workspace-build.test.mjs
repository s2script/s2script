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
      // The WHY, straight off the engine (§5.3.0): a dependency name is resolved through
      // `iface_published` (loader.rs:175), which a plugin publishing nothing can never make true.
      assert.match(e.message, /iface_published \(loader\.rs:175\)/);
      assert.match(e.message, /InterfaceUnavailable/);
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
function tempLintWorkspace({ marker, pluginName = "p" }) {
  const root = mkdtempSync(join(tmpdir(), "s2lint-"));
  const pluginDir = join(root, "plugins", pluginName);
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

test("lintPlugin still enforces the own config when the plugin dir name is a glob metacharacter", async () => {
  // A plugin scaffolded at e.g. `plugins/pl[ug]in` is an ordinary directory on disk but a bracket
  // EXPRESSION to a glob matcher. Building the own-config target by embedding the absolute path
  // into a pattern (`join(absDir, "**", "*.ts")`) turns `[ug]` into a character class instead of
  // a literal name, the pattern matches zero files on this exact tree, and
  // errorOnUnmatchedPattern:false means zero files is not an error — the gate would report green
  // while linting nothing.
  const { pluginDir } = tempLintWorkspace({ marker: true, pluginName: "pl[ug]in" });
  const r = await lintPlugin(pluginDir, null);
  assert.equal(r.ok, false, "the root config's no-debugger must still fire for a bracket-named dir");
  assert.match(r.output, /no-debugger/);
});

// ---------------------------------------------------------------------------
// §5.2 preflight
// ---------------------------------------------------------------------------

test("preflight passes a coherent workspace", () => {
  assert.deepEqual(preflightProblems(loadWorkspace(wsContract)), []);
  preflightWorkspace(loadWorkspace(wsContract)); // must not throw
});

/**
 * A throwaway workspace: `members` maps `plugins/<dir>` to a package.json, and any `api.d.ts`
 * value is written beside it. Used by the tests below that need a SHAPE no fixture should carry
 * permanently (a malformed sibling, two producers of one interface).
 */
function tempWorkspace(members) {
  const root = mkdtempSync(join(tmpdir(), "s2ws-"));
  writeFileSync(
    join(root, "package.json"),
    JSON.stringify({
      name: "@fixture/tmp-ws",
      private: true,
      version: "0.0.0",
      workspaces: ["plugins/*"],
      s2script: { workspace: { plugins: ["plugins/*"] } },
    }),
  );
  for (const [dir, { api, ...pkg }] of Object.entries(members)) {
    mkdirSync(join(root, "plugins", dir, "src"), { recursive: true });
    writeFileSync(join(root, "plugins", dir, "package.json"), JSON.stringify(pkg, null, 2));
    if (api !== undefined) writeFileSync(join(root, "plugins", dir, "api.d.ts"), api);
  }
  return root;
}

test("sibling resolution survives an UNRELATED malformed sibling (§4.2 single-plugin mode)", () => {
  // Routing resolution through `loadWorkspace` — which aggregates and THROWS on any workspace-wide
  // problem — made `s2s build plugins/healthy-plugin` fail because some other plugin has no entry
  // point. Degrade per-descriptor: whole-workspace shape is preflight's job, and it still runs.
  const root = tempWorkspace({
    broken: { name: "@t/broken", version: "1.0.0" }, // no entry point: §4.3 rule 2
    producer: {
      name: "@t/producer",
      version: "1.0.0",
      main: "src/plugin.ts",
      types: "api.d.ts",
      s2script: { publishes: "self" },
      api: "export interface Api { ping(): string }\n",
    },
    consumer: {
      name: "@t/consumer",
      version: "1.0.0",
      main: "src/plugin.ts",
      s2script: { pluginDependencies: { "@t/producer": "^1.0.0" } },
    },
  });

  const { siblings } = resolveSiblingContracts(join(root, "plugins", "consumer"), ["@t/producer"]);
  assert.equal(
    siblings.get("@t/producer")?.typesPath,
    join(root, "plugins", "producer", "api.d.ts"),
    "one malformed sibling must not take the healthy pair down with it",
  );
  // And nothing is swept under the rug: the workspace-mode path still refuses the same tree.
  assert.throws(() => loadWorkspace(root), /plugins\/broken: .* has no entry point/);
});

test("sibling resolution is package-manager independent (no node_modules at all)", () => {
  // §3.1 observed that npm symlinks EVERY workspace package whether or not anything depends on it,
  // and §3.2 resolved siblings through that link. It does not generalise: pnpm and bun link only
  // what is declared as an NPM dependency, and an s2script plugin dep lives under
  // `s2script.pluginDependencies` — measured on pnpm 10 and bun 1.3, neither created a link and the
  // consumer failed with TS2307. yarn PnP has no node_modules at all. `typecheck.ts` now maps every
  // sibling interface explicitly, so NO link is required. This fixture has no node_modules
  // whatsoever — the strictest case, standing in for all three.
  const root = tempWorkspace({
    producer: {
      name: "@t/producer", version: "1.0.0", main: "src/plugin.ts", types: "api.d.ts",
      s2script: { publishes: "self" },
      api: "export declare const x: number;\n",
    },
    consumer: {
      name: "@t/consumer", version: "1.0.0", main: "src/plugin.ts",
      s2script: { pluginDependencies: { "@t/producer": "^1.0.0" } },
    },
  });
  assert.equal(existsSync(join(root, "node_modules")), false, "fixture must have no node_modules");

  const { siblings } = resolveSiblingContracts(join(root, "plugins", "consumer"), ["@t/producer"]);
  assert.equal(
    siblings.get("@t/producer")?.typesPath,
    join(root, "plugins", "producer", "api.d.ts"),
    "resolution must not depend on a package manager having created a symlink",
  );
});

test("pnpm-workspace.yaml counts as workspace membership", () => {
  // pnpm does not use package.json "workspaces" at all. Reading only that field made every pnpm
  // workspace fail rule 3 with advice naming a field pnpm users deliberately do not have.
  const root = tempWorkspace({
    shop: { name: "@t/shop", version: "1.0.0", main: "src/plugin.ts" },
  });
  // Strip the npm field, leaving ONLY the pnpm manifest.
  const pkgPath = join(root, "package.json");
  const pkg = JSON.parse(readFileSync(pkgPath, "utf8"));
  delete pkg.workspaces;
  writeFileSync(pkgPath, JSON.stringify(pkg, null, 2));
  writeFileSync(join(root, "pnpm-workspace.yaml"), 'packages:\n  - "plugins/*"\n');

  const ws = loadWorkspace(root); // must not throw
  assert.deepEqual(ws.plugins.map((p) => p.name), ["@t/shop"]);
});

test("a dropped sibling that MIGHT be the producer is named, never silently stubbed", () => {
  // Regression: making the scan lenient (so an unrelated malformed sibling cannot break a targeted
  // build) also dropped the malformed member from `indexInterfaces`. A dep that member PUBLISHES
  // then missed `producers`, looked like an ordinary registry dependency, and silently fell back to
  // the `any` stub — restoring the exact degradation §5.3 exists to remove, on the documented
  // per-plugin path. The leniency stays; the silence does not.
  const root = tempWorkspace({
    // Publishes @t/iface and has `types`, but NO entry point — dropped by §4.3 rule 2.
    producer: {
      name: "@t/dropped", version: "1.0.0", types: "api.d.ts",
      s2script: { publishes: { "@t/iface": "1.0.0" } },
      api: "export interface Api { ping(): string }\n",
    },
    consumer: {
      name: "@t/consumer", version: "1.0.0", main: "src/plugin.ts",
      s2script: { pluginDependencies: { "@t/iface": "^1.0.0" } },
    },
  });

  const warnings = [];
  const realWarn = console.warn;
  console.warn = (...a) => warnings.push(a.join(" "));
  let siblings;
  try {
    ({ siblings } = resolveSiblingContracts(join(root, "plugins", "consumer"), ["@t/iface"]));
  } finally {
    console.warn = realWarn;
  }

  // Still treated as a registry dependency — failing would break §11's compatibility hinge, since
  // an unresolved dep is legitimately a registry interface.
  assert.equal(siblings.size, 0);
  // But the author is told which member went unread and what it costs them.
  const warned = warnings.join("\n");
  assert.match(warned, /@t\/iface" resolves to no workspace producer/);
  assert.match(warned, /plugins\/producer: .* has no entry point/);
  assert.match(warned, /silently typechecks against `any`/);
});

test("two siblings publishing ONE interface name is a named hard error, at both halves", () => {
  // §5.3.0: the workspace cannot say which producer a consumer gets, and the engine refuses the
  // second publish anyway ("implementations are alternatives; load only one").
  const api = "export interface Api { ping(): string }\n";
  const root = tempWorkspace({
    one: {
      name: "@t/one", version: "1.0.0", main: "src/plugin.ts", types: "api.d.ts",
      s2script: { publishes: { "@t/iface": "1.0.0" } }, api,
    },
    two: {
      name: "@t/two", version: "1.0.0", main: "src/plugin.ts", types: "api.d.ts",
      s2script: { publishes: { "@t/iface": "1.0.0" } }, api,
    },
    consumer: {
      name: "@t/consumer", version: "1.0.0", main: "src/plugin.ts",
      s2script: { pluginDependencies: { "@t/iface": "^1.0.0" } },
    },
  });

  // At the consumer's own build: it cannot compile against an ambiguous contract.
  assert.throws(
    () => resolveSiblingContracts(join(root, "plugins", "consumer"), ["@t/iface"]),
    (e) => {
      assert.match(e.message, /interface "@t\/iface" is published by 2 plugins/);
      assert.match(e.message, /@t\/one \(plugins\/one\)/);
      assert.match(e.message, /@t\/two \(plugins\/two\)/);
      return true;
    },
  );
  // And at preflight, so a workspace build never gets as far as picking a coin-flip producer.
  const problems = preflightProblems(loadWorkspace(root));
  assert.equal(problems.length, 1);
  assert.match(problems[0], /is published by 2 plugins/);
});

test("a malformed s2script.publishes is ATTRIBUTED and aggregated, never an anonymous throw", () => {
  // Verified: with one plugin carrying `"publishes": "sef"`, the expandPublishes error named no
  // plugin, no directory, no package — and aborted the whole 18-plugin build from inside the
  // range gate. §11 says report it against its author, alongside every other problem.
  const root = tempWorkspace({
    typo: { name: "@t/typo", version: "1.0.0", main: "src/plugin.ts", s2script: { publishes: "sef" } },
    ok: { name: "@t/ok", version: "1.0.0", main: "src/plugin.ts" },
  });
  const problems = preflightProblems(loadWorkspace(root));
  assert.equal(problems.length, 1);
  assert.match(problems[0], /@t\/typo \(plugins\/typo\): invalid s2script\.publishes/);
  assert.match(problems[0], /the only valid string form is "self"/);
  // The gate that used to explode still runs, and still reports its own violations.
  assert.doesNotThrow(() => preflightProblems(loadWorkspace(root)));
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
