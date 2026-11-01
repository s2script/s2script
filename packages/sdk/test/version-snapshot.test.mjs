import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { bundleManifest, readBundles } from "../src/workspace/bundle.ts";
import { loadWorkspace } from "../src/workspace/workspace.ts";
import { checkSiblingRanges } from "../src/workspace/graph.ts";
import { buildPlugin } from "../src/build.ts";
import { openZip } from "./zip.mjs";
import { snapshotWorkspace } from "../src/version/snapshot.ts";

const cli = resolve(dirname(fileURLToPath(import.meta.url)), "../src/cli.ts");
const json = (dir, path) => JSON.parse(readFileSync(join(dir, path), "utf8"));
function write(dir, path, value) {
  mkdirSync(dirname(join(dir, path)), { recursive: true });
  writeFileSync(join(dir, path), typeof value === "string" ? value : JSON.stringify(value, null, 2) + "\n");
}
const git = (dir, ...args) => execFileSync("git", args, { cwd: dir, encoding: "utf8" }).trim();
function commit(dir, message = "fixture") {
  git(dir, "add", ".");
  git(dir, "commit", "-qm", message);
  return git(dir, "rev-parse", "HEAD");
}
function scratch(t) {
  const dir = mkdtempSync(join(tmpdir(), "s2s-snapshot-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  write(dir, "package.json", {
    name: "fixture", private: true, workspaces: ["alpha/plugins/*", "beta/plugins/*", "libs/*"],
    s2script: { workspace: { plugins: ["alpha/plugins/*", "beta/plugins/*"], bundles: ["alpha", "beta"] } },
  });
  for (const product of ["alpha", "beta"]) write(dir, `${product}/package.json`, {
    name: `@fixture/${product}`, version: "0.0.0", publishConfig: { access: product === "alpha" ? "restricted" : "public" },
    s2script: { kind: "bundle" },
  });
  const packages = {
    "alpha/plugins/producer": { name: "@fixture/producer", version: "2.3.0", types: "api.d.ts", s2script: { publishes: "self" } },
    "alpha/plugins/consumer": { name: "@fixture/consumer", version: "1.0.0", dependencies: { "@fixture/shared": "*" }, s2script: { pluginDependencies: { "@fixture/producer": "^2.0.0" } } },
    "beta/plugins/optional": { name: "@fixture/optional", version: "3.0.0", s2script: { optionalPluginDependencies: { "@fixture/producer": "^2.0.0" }, publishes: { "@fixture/separate": "1.7.0" } } },
    "beta/plugins/independent": { name: "@fixture/independent", version: "4.0.0" },
  };
  for (const [path, pkg] of Object.entries(packages)) {
    write(dir, `${path}/package.json`, { ...pkg, main: "src/plugin.ts" });
    write(dir, `${path}/src/plugin.ts`, "export function OnPluginStart() {}\n");
  }
  write(dir, "alpha/plugins/producer/api.d.ts", "export interface Producer { greet(): string; }\n");
  write(dir, "libs/shared/package.json", { name: "@fixture/shared", version: "1.4.0", private: true, main: "index.ts" });
  write(dir, "libs/shared/index.ts", "export const shared = 1;\n");
  write(dir, ".changeset/stable-intent.md", '---\n"@fixture/producer": minor\n---\n\nStable feature.\n');
  git(dir, "init", "-q");
  git(dir, "config", "user.email", "fixture@example.com");
  git(dir, "config", "user.name", "Snapshot Fixture");
  const source = commit(dir);
  return { dir, source };
}
function run(dir, args = []) {
  return spawnSync(process.execPath, ["--experimental-strip-types", "--no-warnings", cli, "version", dir, ...args], {
    encoding: "utf8", env: { ...process.env, CI: "1" },
  });
}
function snapshot(dir, id, args = [], output = "next.json") {
  const result = run(dir, ["--snapshot", id, ...args, "--output", join(dir, output)]);
  assert.equal(result.status, 0, result.stderr + result.stdout);
  return json(dir, output);
}
function reset(dir, source) {
  git(dir, "reset", "--hard", source);
  git(dir, "clean", "-fdq");
}
const names = (report) => report.packages.map((p) => p.name).sort();

test("snapshot versions all native workspace members and zero-version bundles without consuming stable intent", (t) => {
  const { dir, source } = scratch(t);
  const before = readFileSync(join(dir, ".changeset/stable-intent.md"), "utf8");
  const report = snapshot(dir, "dev.12.gabc");
  assert.equal(report.schemaVersion, 1);
  assert.equal(report.source, source);
  assert.equal(report.id, "dev.12.gabc");
  assert.equal(report.packages.length, 7);
  assert.deepEqual(report.packages.find((p) => p.name === "@fixture/shared"), {
    name: "@fixture/shared", version: "0.0.0-dev.12.gabc", path: "libs/shared", kind: "library", private: true,
  });
  assert.deepEqual(report.packages.find((p) => p.name === "@fixture/alpha"), {
    name: "@fixture/alpha", version: "0.0.0-dev.12.gabc", path: "alpha", kind: "bundle", private: false, access: "restricted",
  });
  assert.equal(json(dir, "alpha/plugins/consumer/package.json").dependencies["@fixture/shared"], "0.0.0-dev.12.gabc");
  assert.deepEqual(json(dir, "alpha/plugins/producer/package.json").s2script.publishes, { "@fixture/producer": "2.3.0" });
  assert.deepEqual(json(dir, "beta/plugins/optional/package.json").s2script.publishes, { "@fixture/separate": "1.7.0" });
  assert.equal(json(dir, "alpha/plugins/consumer/package.json").s2script.pluginDependencies["@fixture/producer"], "^2.0.0");
  assert.deepEqual(checkSiblingRanges(loadWorkspace(dir).plugins), []);
  assert.equal(readFileSync(join(dir, ".changeset/stable-intent.md"), "utf8"), before);
  assert.equal(existsSync(join(dir, "alpha/plugins/producer/CHANGELOG.md")), false);
  const retry = snapshot(dir, "dev.12.gabc");
  assert.deepEqual(retry, report, "retrying the same source and id is deterministic");
});

test("snapshot selects interface consumers including optional edges and preserves unchanged bundle pins from state", (t) => {
  const { dir, source } = scratch(t);
  const prior = snapshot(dir, "dev.1.gaaa");
  reset(dir, source);
  write(dir, "alpha/plugins/producer/api.d.ts", "export interface Producer { greet(name: string): string; }\n");
  commit(dir, "producer contract");
  write(dir, "prior.json", prior);
  const report = snapshot(dir, "dev.2.gbbb", ["--since", source, "--snapshot-state", join(dir, "prior.json")]);
  assert.deepEqual(names(report), ["@fixture/alpha", "@fixture/beta", "@fixture/consumer", "@fixture/optional", "@fixture/producer"]);
  assert.equal(report.versions["@fixture/independent"], "0.0.0-dev.1.gaaa");
  assert.equal(report.versions["@fixture/shared"], "0.0.0-dev.1.gaaa");
  const beta = readBundles(loadWorkspace(dir)).find((b) => b.name === "@fixture/beta");
  assert.deepEqual(bundleManifest(beta).members, {
    "@fixture/independent": { version: "0.0.0-dev.1.gaaa" },
    "@fixture/optional": { version: "0.0.0-dev.2.gbbb" },
  });
  assert.equal(json(dir, "alpha/plugins/consumer/package.json").dependencies["@fixture/shared"], "0.0.0-dev.1.gaaa");
});

test("snapshot matches reverse runtime edges by independent published interface name", (t) => {
  const { dir } = scratch(t);
  const producer = json(dir, "alpha/plugins/producer/package.json");
  producer.s2script.publishes = { "@fixture/custom-interface": "1.7.0" };
  write(dir, "alpha/plugins/producer/package.json", producer);
  const consumer = json(dir, "alpha/plugins/consumer/package.json");
  consumer.s2script.pluginDependencies = { "@fixture/custom-interface": "^1.0.0" };
  write(dir, "alpha/plugins/consumer/package.json", consumer);
  const optional = json(dir, "beta/plugins/optional/package.json");
  optional.s2script.optionalPluginDependencies = { "@fixture/custom-interface": "^1.0.0" };
  write(dir, "beta/plugins/optional/package.json", optional);
  const source = commit(dir, "independent interface");
  write(dir, "alpha/plugins/producer/api.d.ts", "export interface Producer { revised(): string; }\n");
  commit(dir, "change producer");
  const report = snapshot(dir, "dev.2.gbbb", ["--since", source]);
  assert.deepEqual(names(report), ["@fixture/alpha", "@fixture/beta", "@fixture/consumer", "@fixture/optional", "@fixture/producer"]);
  assert.deepEqual(json(dir, "alpha/plugins/producer/package.json").s2script.publishes, { "@fixture/custom-interface": "1.7.0" });
});

test("snapshot cascades private npm shared-code changes only to its consumers and their bundles", (t) => {
  const { dir, source } = scratch(t);
  write(dir, "libs/shared/index.ts", "export const shared = 2;\n");
  commit(dir, "shared change");
  const report = snapshot(dir, "dev.3.gccc", ["--since", source]);
  assert.deepEqual(names(report), ["@fixture/alpha", "@fixture/consumer", "@fixture/shared"]);
  assert.equal(report.versions["@fixture/producer"], "2.3.0");
});

test("snapshot recognizes s2script libraries and transitive npm consumers", (t) => {
  const { dir } = scratch(t);
  const consumer = json(dir, "alpha/plugins/consumer/package.json");
  delete consumer.dependencies;
  consumer.s2script.libraries = { "@fixture/shared": "*" };
  write(dir, "alpha/plugins/consumer/package.json", consumer);
  const independent = json(dir, "beta/plugins/independent/package.json");
  independent.dependencies = { "@fixture/consumer": "*" };
  write(dir, "beta/plugins/independent/package.json", independent);
  const source = commit(dir, "library edges");
  write(dir, "libs/shared/index.ts", "export const shared = 3;\n");
  commit(dir, "library change");
  assert.deepEqual(names(snapshot(dir, "dev.4.gddd", ["--since", source])), ["@fixture/alpha", "@fixture/beta", "@fixture/consumer", "@fixture/independent", "@fixture/shared"]);
  assert.equal(json(dir, "alpha/plugins/consumer/package.json").s2script.libraries["@fixture/shared"], "0.0.0-dev.4.gddd");
});

test("snapshot includes product shared source while a bundle descriptor changes only that bundle", (t) => {
  const { dir, source } = scratch(t);
  write(dir, "alpha/shared/util.ts", "export const feature = true;\n");
  commit(dir, "product shared");
  assert.deepEqual(names(snapshot(dir, "dev.5.geee", ["--since", source])), ["@fixture/alpha", "@fixture/beta", "@fixture/consumer", "@fixture/optional", "@fixture/producer"]);
  reset(dir, source);
  const bundle = json(dir, "alpha/package.json");
  bundle.description = "New metadata";
  write(dir, "alpha/package.json", bundle);
  commit(dir, "descriptor");
  assert.deepEqual(names(snapshot(dir, "dev.6.gfff", ["--since", source])), ["@fixture/alpha"]);
});

test("snapshot root bundle metadata affects only its descriptor while root dependencies affect all", (t) => {
  const { dir } = scratch(t);
  const root = json(dir, "package.json");
  root.name = "@fixture/all";
  root.version = "0.0.0";
  root.private = false;
  root.s2script.kind = "bundle";
  delete root.s2script.workspace.bundles;
  write(dir, "package.json", root);
  const source = commit(dir, "root bundle");
  write(dir, "package.json", { ...root, description: "New metadata" });
  commit(dir, "metadata");
  assert.deepEqual(names(snapshot(dir, "dev.6.gfff", ["--since", source])), ["@fixture/all"]);
  reset(dir, source);
  write(dir, "package.json", { ...root, devDependencies: { typescript: "^5.6.0" } });
  commit(dir, "build tooling");
  assert.equal(snapshot(dir, "dev.7.gggg", ["--since", source]).packages.length, 6);
});

test("snapshot docs, tests, and stable changesets produce no new versions and retain cumulative state", (t) => {
  const { dir, source } = scratch(t);
  const prior = snapshot(dir, "dev.1.gaaa");
  reset(dir, source);
  for (const path of ["docs/guide.md", "README.md", "alpha/plugins/producer/test/spec.ts", "libs/shared/index.test.ts", ".changeset/next.md"]) write(dir, path, "documentation or tests\n");
  commit(dir, "docs and tests");
  write(dir, "prior.json", prior);
  const report = snapshot(dir, "dev.7.gggg", ["--since", source, "--snapshot-state", join(dir, "prior.json")]);
  assert.deepEqual(report.packages, []);
  assert.deepEqual(report.versions, prior.versions);
});

test("snapshot root lockfile changes affect every member and bundle", (t) => {
  const { dir, source } = scratch(t);
  write(dir, "package-lock.json", { lockfileVersion: 3 });
  commit(dir, "lockfile");
  assert.equal(snapshot(dir, "dev.8.ghhh", ["--since", source]).packages.length, 7);
});

test("snapshot runtime packages named test are not mistaken for test-only changes", (t) => {
  const { dir } = scratch(t);
  write(dir, "alpha/plugins/test/package.json", { name: "@fixture/runtime-test", version: "1.0.0", main: "src/plugin.ts" });
  write(dir, "alpha/plugins/test/src/plugin.ts", "export function OnPluginStart() {}\n");
  const source = commit(dir, "runtime package named test");
  write(dir, "alpha/plugins/test/src/plugin.ts", "export function OnPluginStart() { return; }\n");
  commit(dir, "runtime behavior");
  assert.deepEqual(names(snapshot(dir, "dev.9.giii", ["--since", source])), ["@fixture/alpha", "@fixture/runtime-test"]);
});

test("snapshot rejects removed package manifests rather than leaving stale bundle member pins", (t) => {
  const { dir, source } = scratch(t);
  rmSync(join(dir, "beta/plugins/independent"), { recursive: true });
  commit(dir, "remove member");
  const result = run(dir, ["--snapshot", "dev.9.giii", "--since", source, "--output", join(dir, "next.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /removed.*package|deleted.*package/i);
  assert.equal(json(dir, "alpha/plugins/producer/package.json").version, "2.3.0");
  assert.equal(existsSync(join(dir, "next.json")), false);
});

for (const args of [
  ["--snapshot", "../unsafe"], ["--snapshot", "dev.01.gabc"], ["--snapshot"],
  ["--snapshot", "dev.1.gabc", "--since", "nonexistent-ref"],
  ["--snapshot", "dev.1.gabc", "--snapshot-state", "missing.json"],
  ["--snapshot", "dev.1.gabc", "--output"], ["--snapshot-state", "missing.json"],
]) test(`snapshot invalid options fail before manifest edits: ${args.join(" ")}`, (t) => {
  const { dir } = scratch(t);
  const before = readFileSync(join(dir, "alpha/plugins/producer/package.json"), "utf8");
  const result = run(dir, args);
  assert.notEqual(result.status, 0);
  assert.equal(readFileSync(join(dir, "alpha/plugins/producer/package.json"), "utf8"), before);
});

test("snapshot rejects malformed state and output collisions without consuming changesets", (t) => {
  const { dir } = scratch(t);
  write(dir, "prior.json", { schemaVersion: 99, versions: {} });
  let result = run(dir, ["--snapshot", "dev.1.gabc", "--snapshot-state", join(dir, "prior.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /snapshot state/i);
  result = run(dir, ["--snapshot", "dev.1.gabc", "--output", join(dir, "alpha/plugins/producer/package.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /output/i);
  assert.equal(json(dir, "alpha/plugins/producer/package.json").version, "2.3.0");
  assert.equal(existsSync(join(dir, ".changeset/stable-intent.md")), true);
});

test("snapshot rebuilds changed interface consumers with matching compiledAgainst and authored interface versions", async (t) => {
  const dir = mkdtempSync(join(tmpdir(), "s2s-snapshot-contract-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(resolve(dirname(cli), "../test/fixtures/ws-contract"), dir, { recursive: true });
  git(dir, "init", "-q");
  git(dir, "config", "user.email", "fixture@example.com");
  git(dir, "config", "user.name", "Snapshot Fixture");
  const source = commit(dir);
  const prior = snapshot(dir, "dev.1.gaaa");
  reset(dir, source);
  const contract = readFileSync(join(dir, "plugins/greeter/api.d.ts"), "utf8") + "\n/** A compatible contract revision. */\n";
  write(dir, "plugins/greeter/api.d.ts", contract);
  commit(dir, "contract revision");
  write(dir, "prior.json", prior);
  const report = snapshot(dir, "dev.2.gbbb", ["--since", source, "--snapshot-state", join(dir, "prior.json")]);
  assert.deepEqual(names(report), ["@fixture/ws-consumer", "@fixture/ws-greeter", "@fixture/ws-stale-consumer"]);
  const packagesDir = resolve(dirname(cli), "../..");
  const producer = JSON.parse(openZip(await buildPlugin(join(dir, "plugins/greeter"), packagesDir)).readAsText("manifest.json"));
  const consumer = JSON.parse(openZip(await buildPlugin(join(dir, "plugins/consumer"), packagesDir)).readAsText("manifest.json"));
  const hash = createHash("sha256").update(contract).digest("hex");
  assert.equal(producer.version, "0.0.0-dev.2.gbbb");
  assert.equal(consumer.version, "0.0.0-dev.2.gbbb");
  assert.deepEqual(producer.publishes["@fixture/ws-greeter"], { version: "1.2.0", typesSha256: hash });
  assert.equal(consumer.compiledAgainst["@fixture/ws-greeter"], hash);
});

test("snapshot same-source retries restore every published version with no new release entries", (t) => {
  const { dir, source } = scratch(t);
  const prior = snapshot(dir, "dev.1.gaaa");
  reset(dir, source);
  write(dir, "prior.json", prior);
  const report = snapshot(dir, "dev.1.gaaa", ["--since", source, "--snapshot-state", join(dir, "prior.json")]);
  assert.deepEqual(report.packages, []);
  assert.deepEqual(report.versions, prior.versions);
  assert.equal(json(dir, "beta/plugins/independent/package.json").version, "0.0.0-dev.1.gaaa");
  assert.equal(json(dir, "alpha/package.json").version, "0.0.0-dev.1.gaaa");
});

test("snapshot rejects cumulative state missing an unchanged package version", (t) => {
  const { dir, source } = scratch(t);
  const prior = snapshot(dir, "dev.1.gaaa");
  reset(dir, source);
  delete prior.versions["@fixture/independent"];
  prior.packages = [];
  write(dir, "prior.json", prior);
  const result = run(dir, ["--snapshot", "dev.2.gbbb", "--since", source, "--snapshot-state", join(dir, "prior.json")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /snapshot state.*missing.*@fixture\/independent/i);
  assert.equal(json(dir, "beta/plugins/independent/package.json").version, "4.0.0");
  assert.equal(json(dir, "alpha/package.json").version, "0.0.0");
});

test("snapshot failure to write report restores every manifest and prior output", (t) => {
  const { dir } = scratch(t);
  const before = readFileSync(join(dir, "alpha/plugins/producer/package.json"), "utf8");
  mkdirSync(join(dir, "output-directory"));
  const result = run(dir, ["--snapshot", "dev.1.gaaa", "--output", join(dir, "output-directory")]);
  assert.notEqual(result.status, 0);
  assert.equal(readFileSync(join(dir, "alpha/plugins/producer/package.json"), "utf8"), before);
  assert.equal(json(dir, "alpha/package.json").version, "0.0.0");
});

test("snapshot refuses mismatched state source, reused ids, and incomplete state before writes", (t) => {
  const { dir, source } = scratch(t);
  const prior = snapshot(dir, "dev.1.gaaa");
  reset(dir, source);
  write(dir, "alpha/plugins/producer/src/plugin.ts", "export function OnPluginStart() { return; }\n");
  const next = commit(dir, "new source");
  for (const [state, id, since] of [
    [{ ...prior, source: next }, "dev.2.gbbb", source],
    [prior, "dev.1.gaaa", source],
    [{ ...prior, id: undefined }, "dev.2.gbbb", source],
  ]) {
    write(dir, "prior.json", state);
    const result = run(dir, ["--snapshot", id, "--since", since, "--snapshot-state", join(dir, "prior.json")]);
    assert.notEqual(result.status, 0, result.stdout);
    assert.equal(json(dir, "alpha/plugins/producer/package.json").version, "2.3.0");
  }
});

test("snapshot output cannot overwrite tracked source files", (t) => {
  const { dir } = scratch(t);
  const result = run(dir, ["--snapshot", "dev.1.gaaa", "--output", join(dir, "alpha/plugins/producer/src/plugin.ts")]);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /output/i);
  assert.equal(readFileSync(join(dir, "alpha/plugins/producer/src/plugin.ts"), "utf8"), "export function OnPluginStart() {}\n");
  assert.equal(json(dir, "alpha/plugins/producer/package.json").version, "2.3.0");
});

test("snapshot does not remove a preexisting temporary output when exclusive creation fails", (t) => {
  const { dir } = scratch(t);
  const outputPath = join(dir, "next.json");
  const existing = `${outputPath}.${process.pid}.tmp`;
  writeFileSync(existing, "prior temporary output\n");
  assert.throws(() => snapshotWorkspace(dir, { id: "dev.1.gaaa", outputPath }), /EEXIST/);
  assert.equal(existsSync(existing), true);
  assert.equal(readFileSync(existing, "utf8"), "prior temporary output\n");
  assert.equal(json(dir, "alpha/package.json").version, "0.0.0");
});
