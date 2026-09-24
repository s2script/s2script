import { test } from "node:test";
import assert from "node:assert/strict";
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, readFileSync, renameSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { runInNewContext } from "node:vm";
import { fileURLToPath } from "node:url";
import { buildGamePackages } from "./build-game-packages.mjs";
import { parseFunctionFile } from "../packages/sdk/src/engine-functions/parse.ts";
import { normalizeFunctions } from "../packages/sdk/src/engine-functions/normalize.ts";
import { engineFunctionsArchive, engineFunctionsManifest } from "../packages/sdk/src/engine-functions/archive.ts";

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "s2-game-package-"));
  const source = join(root, "synthetic");
  mkdirSync(join(source, "js"), { recursive: true });
  mkdirSync(join(source, "gamedata"));
  writeFileSync(join(source, "game-package.jsonc"), `{
    "schemaVersion": 1, "id": "@test/cs2", "match": {"game":"csgo","engine":"source2"},
    "gamedataOwner": "cs2", "bootstrapInputs": ["js/a.js", "js/b.js"],
    "gamedataRoot": "gamedata"
  }`);
  writeFileSync(join(source, "js/a.js"), "globalThis.a = 1;\n");
  writeFileSync(join(source, "js/b.js"), '({".":{a:globalThis.a,b:2},"./ui":{value:23}});\n');
  writeFileSync(join(source, "gamedata/master.gamedata.jsonc"),
    '{"files":[{"file":"game.cs2.jsonc","game":"csgo"}]}');
  writeFileSync(join(source, "gamedata/game.cs2.jsonc"),
    '{"z":1, /* comment */ "a":{"y":2,"x":1},"variants":[{"b":2,"a":1},{"d":4,"c":3}]}');
  return { root, source, out: join(root, "out") };
}

function buildFixture() {
  const f = fixture();
  buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true });
  return {
    manifest: JSON.parse(readFileSync(join(f.out, "game-packages.json"))),
    manifestBytes: readFileSync(join(f.out, "game-packages.json")),
    bootstrap: readFileSync(join(f.out, "game-packages/cs2/index.js")),
    gamedata: readFileSync(join(f.out, "game-packages/cs2/gamedata.json")),
  };
}

const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
const scriptsRoot = dirname(fileURLToPath(import.meta.url));
const acquireContract = '{"frame":{"post":["player:entity?","defIndex:u16-number","method:i32","result:i32","skipped:bool"],"pre":["player:entity?","defIndex:u16-number","method:i32","result:i32:mutable"]},"id":"legacy.acquire.v1","preDecision":"s2.pre-decision.v1","semantics":{"allowed":0,"deny":"nonzero","denyTie":"first","engineResult":"only-if-original-ran","implicitDeny":1,"stableWithinStrength":true,"voteOrder":["handled-stop","changed"]},"subscriberDelivery":"s2.subscriber-delivery.v1","timing":{"post":"after-effective-return","pre":"before-original"},"version":1}';
const hudContract = '{"frame":{"delivery":["player:entity?","buttonId:copied-string"]},"id":"legacy.hud-click.v1","preDecision":"s2.pre-decision.v1","semantics":{"buttonIdCopy":"before-javascript","finalAction":"continue","reentry":"synchronous-nested"},"subscriberDelivery":"s2.subscriber-delivery.v1","timing":{"deliveryLabel":"post","nativePhase":"pre","relativeToOriginal":"before"},"version":1}';

function withAdapters(f, entries = [
  ["legacy.acquire.v1", acquireContract, 'globalThis.adapterSeen = [Object.isFrozen(globalThis.__s2_adapter_contracts), globalThis.__s2_adapter_contracts["legacy.acquire.v1"]];\n'],
  ["legacy.hud-click.v1", hudContract, 'globalThis.hudSeen = globalThis.__s2_adapter_contracts["legacy.hud-click.v1"];\n'],
]) {
  mkdirSync(join(f.source, "adapters/contracts"), { recursive: true });
  mkdirSync(join(f.source, "js/adapters"), { recursive: true });
  const adapters = {};
  for (const [id, contract, source] of entries) {
    const contractPath = `adapters/contracts/${id}.json`;
    const sourcePath = `js/adapters/${id}.js`;
    writeFileSync(join(f.source, contractPath), contract);
    writeFileSync(join(f.source, sourcePath), source);
    adapters[id] = { version: 1, contract: contractPath, source: sourcePath };
  }
  const path = join(f.source, "game-package.jsonc");
  const manifest = JSON.parse(readFileSync(path, "utf8"));
  manifest.adapters = adapters;
  writeFileSync(path, JSON.stringify(manifest));
  return f;
}

function built(f) {
  buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true });
  const manifest = readFileSync(join(f.out, "game-packages.json"));
  const bootstrap = readFileSync(join(f.out, "game-packages/cs2/index.js"));
  return { manifest, bootstrap, product: JSON.parse(manifest).packages[0] };
}

test("emits the frozen deployed schema and lowercase SHA-256", () => {
  const out = buildFixture();
  assert.deepEqual(Object.keys(out.manifest), ["schemaVersion", "packages"]);
  assert.equal(out.manifest.schemaVersion, 2);
  assert.deepEqual(Object.keys(out.manifest.packages[0]),
    ["id", "match", "gamedataOwner", "bootstrap", "gamedata"]);
  assert.match(out.manifest.packages[0].bootstrap.sha256, /^[0-9a-f]{64}$/);
  assert.equal(out.manifest.packages[0].bootstrap.sha256, sha256(out.bootstrap));
  assert.equal(out.manifest.packages[0].gamedata.sha256, sha256(out.gamedata));
  assert.deepEqual(JSON.parse(out.gamedata), {
    schemaVersion: 1, owner: "cs2", files: [
      { path: "master.gamedata.jsonc", document: { files: [{ file: "game.cs2.jsonc", game: "csgo" }] } },
      { path: "game.cs2.jsonc", document: { a: { x: 1, y: 2 }, variants: [{ a: 1, b: 2 }, { c: 3, d: 4 }], z: 1 } },
    ],
  });
  assert.ok(out.bootstrap.toString().endsWith("\n"));
  assert.ok(out.gamedata.toString().endsWith("\n"));
});

test("default build contains only selected artifacts and explicit package exports", () => {
  const out = mkdtempSync(join(tmpdir(), "s2-game-package-default-"));
  const manifest = buildGamePackages({ outDir: out });
  assert.deepEqual(manifest.packages.map(p => p.id), ["@s2script/cs2"]);
  assert.equal(existsSync(join(out, "js/pawn.js")), false);
  assert.equal(existsSync(join(out, "gamedata/cs2/master.gamedata.jsonc")), false);
  assert.equal(existsSync(join(out, "gamedata/cs2/game.cs2.jsonc")), false);
  const source = readFileSync(join(out, "game-packages/cs2/index.js"), "utf8");
  assert.ok(source.includes('"./ui"'));
  assert.equal(source.includes('"./econ"'), false);
  assert.equal(source.includes("__s2_adapter_contracts"), false);
  assert.equal(existsSync(join(out, "game-packages/fixture")), false);

});

test("fixture source is packaged only through explicit synthetic opt-in", () => {
  const source = join(scriptsRoot, "../games/fixture-source2");
  const out = mkdtempSync(join(tmpdir(), "s2-fixture-package-"));
  assert.throws(() => buildGamePackages({ outDir: out, sourceDirs: [source] }), /synthetic.*opt-in/);
  const manifest = buildGamePackages({ outDir: out, sourceDirs: [source], allowSynthetic: true });
  assert.deepEqual(manifest.packages.map(packageEntry => packageEntry.id), ["@fixture/source2"]);
  assert.deepEqual(manifest.packages[0].match, { engine: "source2", game: "fixture" });
  assert.ok(manifest.packages[0].functions);
  assert.equal(existsSync(join(out, "game-packages/cs2")), false);
  assert.match(readFileSync(join(out, manifest.packages[0].bootstrap.path), "utf8"),
    /fixture proves package boundary only; it is not a supported live game/);
});

test("same inputs produce byte-identical manifest and artifacts", () => {
  const a = buildFixture(), b = buildFixture();
  for (const key of ["manifestBytes", "bootstrap", "gamedata"]) assert.deepEqual(a[key], b[key]);
});

test("separate canonical adapter contracts produce frozen hashes before deterministic source execution", () => {
  const a = built(withAdapters(fixture()));
  const b = built(withAdapters(fixture(), [
    ["legacy.hud-click.v1", hudContract, 'globalThis.hudSeen = globalThis.__s2_adapter_contracts["legacy.hud-click.v1"];\n'],
    ["legacy.acquire.v1", acquireContract, 'globalThis.adapterSeen = [Object.isFrozen(globalThis.__s2_adapter_contracts), globalThis.__s2_adapter_contracts["legacy.acquire.v1"]];\n'],
  ]));
  assert.deepEqual(a.manifest, b.manifest);
  assert.deepEqual(a.bootstrap, b.bootstrap);
  assert.deepEqual(Object.keys(a.product), ["id", "match", "gamedataOwner", "bootstrap", "gamedata"]);
  assert.equal(a.product.bootstrap.sha256, sha256(a.bootstrap));
  const context = {};
  const exports = runInNewContext(a.bootstrap.toString(), context);
  assert.deepEqual([...context.adapterSeen], [true, "69247dc63a6200f5bb8c8ff651b8dd632e8d6af9ad4a0b8d2933199800bc48c0"]);
  assert.equal(context.hudSeen, "28c0c9833d521cadd4eb03254f63ef7dcb1cd8b728f48ebfa8835b82ff03ecdd");
  assert.equal(exports["."].a, 1);
  assert.equal(exports["./ui"].value, 23);
  assert.ok(a.bootstrap.indexOf(Buffer.from("__s2_adapter_contracts")) < a.bootstrap.indexOf(Buffer.from("adapterSeen")));
  assert.ok(a.bootstrap.indexOf(Buffer.from("adapterSeen")) < a.bootstrap.indexOf(Buffer.from("globalThis.a = 1")));
  assert.equal(a.bootstrap.includes(Buffer.from('"voteOrder"')), false, "contract policy data is not executable source");
});

test("adapter source changes implementation identity; one contract change changes only its canonical digest", () => {
  const f = withAdapters(fixture());
  const baseline = built(f);
  const contractPath = join(f.source, "adapters/contracts/legacy.acquire.v1.json");
  writeFileSync(contractPath, JSON.stringify(Object.fromEntries(Object.entries(JSON.parse(acquireContract)).reverse()), null, 2));
  const reformatted = built(f);
  assert.deepEqual(reformatted.bootstrap, baseline.bootstrap, "canonical contract identity ignores key order and whitespace");
  const path = join(f.source, "js/adapters/legacy.acquire.v1.js");
  writeFileSync(path, readFileSync(path, "utf8") + "globalThis.sourceRevision = 2;\n");
  const sourceChanged = built(f);
  assert.notEqual(sourceChanged.product.bootstrap.sha256, baseline.product.bootstrap.sha256);
  const hashMap = source => {
    const context = {};
    runInNewContext(source.toString(), context);
    return { acquire: context.adapterSeen[1], hud: context.hudSeen };
  };
  assert.deepEqual(hashMap(sourceChanged.bootstrap), hashMap(baseline.bootstrap));
  writeFileSync(contractPath, JSON.stringify({ ...JSON.parse(acquireContract), semantics: { ...JSON.parse(acquireContract).semantics, implicitDeny: 2 } }));
  const contractChanged = built(f);
  assert.notEqual(contractChanged.product.bootstrap.sha256, sourceChanged.product.bootstrap.sha256);
  assert.notEqual(hashMap(contractChanged.bootstrap).acquire, hashMap(sourceChanged.bootstrap).acquire);
  assert.equal(hashMap(contractChanged.bootstrap).hud, hashMap(sourceChanged.bootstrap).hud);
  assert.equal(runInNewContext(contractChanged.bootstrap.toString(), {})["."].a, 1);
});

test("adapter declarations reject invalid ids, versions, paths, duplicates, and bounded contract documents", () => {
  const cases = [
    { label: "invalid adapter key", edit: f => {
      const path = join(f.source, "game-package.jsonc");
      const raw = readFileSync(path, "utf8");
      writeFileSync(path, raw.replace('"legacy.acquire.v1":', '"invalid/adapter":'));
    }, match: /id|adapter/i },
    { label: "id mismatch", edit: f => writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), acquireContract.replace("legacy.acquire.v1", "wrong.id")), match: /id|contract/i },
    { label: "entry version", edit: f => updateAdapter(f, entry => { entry.version = 2; }), match: /version|adapter/i },
    { label: "document version", edit: f => writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), acquireContract.replace('"version":1', '"version":2')), match: /version|adapter/i },
    { label: "traversal", edit: f => updateAdapter(f, entry => { entry.contract = "../escape.json"; }), match: /confined|path/i },
    { label: "source traversal", edit: f => updateAdapter(f, entry => { entry.source = "../escape.js"; }), match: /confined|path/i },
    { label: "source symlink escape", edit: f => {
      const path = join(f.source, "js/adapters/legacy.acquire.v1.js");
      rmSync(path);
      writeFileSync(join(f.root, "outside.js"), "globalThis.outside = true;");
      symlinkSync(join(f.root, "outside.js"), path);
    }, match: /escapes/i },
    { label: "unknown entry field", edit: f => updateAdapter(f, entry => { entry.authority = "post"; }), match: /adapter/i },
    { label: "duplicate source", edit: f => updateAdapter(f, entry => { entry.source = "js/a.js"; }), match: /duplicate/i },
    { label: "duplicate contract", edit: f => updateAdapter(f, entry => { entry.contract = "adapters/contracts/legacy.acquire.v1.json"; }, "legacy.hud-click.v1"), match: /duplicate/i },
    { label: "duplicate JSON key", edit: f => writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), acquireContract.replace('"version":1', '"version":1,"version":1')), match: /duplicate/i },
    { label: "invalid JSON", edit: f => writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), acquireContract + ","), match: /invalid|JSON/i },
    { label: "oversized document", edit: f => writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), acquireContract + " ".repeat(65536)), match: /size|limit|large/i },
    { label: "oversized source", edit: f => writeFileSync(join(f.source, "js/adapters/legacy.acquire.v1.js"), "x".repeat(1024 * 1024 + 1)), match: /size|limit|large/i },
  ];
  for (const { label, edit, match } of cases) {
    const f = withAdapters(fixture());
    edit(f);
    assert.throws(() => built(f), match, label);
    assert.equal(existsSync(join(f.out, "game-packages.json")), false, label);
  }
});

test("duplicate adapter declarations in source JSON are rejected before any output", () => {
  const f = withAdapters(fixture());
  const path = join(f.source, "game-package.jsonc");
  const raw = readFileSync(path, "utf8");
  const duplicate = '"legacy.acquire.v1":' + JSON.stringify(JSON.parse(raw).adapters["legacy.acquire.v1"]) + ",";
  writeFileSync(path, raw.replace('"adapters":{', `"adapters":{${duplicate}`));
  assert.throws(() => built(f), /duplicate/i);
  assert.equal(existsSync(join(f.out, "game-packages.json")), false);
});

function updateAdapter(f, edit, id = "legacy.acquire.v1") {
  const path = join(f.source, "game-package.jsonc");
  const manifest = JSON.parse(readFileSync(path, "utf8"));
  edit(manifest.adapters[id]);
  writeFileSync(path, JSON.stringify(manifest));
}

function withFunctions(f, text = JSON.stringify({ schemaVersion: 2, functions: {
  scalar: { target: { module: "libserver.so", pattern: "55 48", validate: { prologue: "55 48" } },
    parameters: [{ name: "enabled", type: "bool" }], returns: "void", surfaces: ["call"] },
} })) {
  writeFileSync(join(f.source, "functions.jsonc"), text);
  const path = join(f.source, "game-package.jsonc");
  const manifest = JSON.parse(readFileSync(path, "utf8"));
  manifest.functionsFile = "functions.jsonc";
  writeFileSync(path, JSON.stringify(manifest));
  return f;
}

test("a nonempty source file emits the exact SDK normalized artifact and derived manifest", () => {
  const a = withFunctions(fixture());
  buildGamePackages({ outDir: a.out, sourceDirs: [a.source], allowSynthetic: true });
  const manifest = JSON.parse(readFileSync(join(a.out, "game-packages.json")));
  const product = manifest.packages[0].functions;
  const bytes = readFileSync(join(a.out, product.path));
  const parsed = parseFunctionFile("functions.jsonc", readFileSync(join(a.source, "functions.jsonc"), "utf8"));
  const expected = normalizeFunctions("@test/cs2", parsed);
  assert.equal(expected.functions.length, 1);
  assert.deepEqual(bytes, Buffer.from(engineFunctionsArchive(expected)));
  assert.equal(product.sha256, sha256(bytes));
  assert.deepEqual({ summary: product.summary, permissions: product.permissions }, engineFunctionsManifest(expected));
  const b = withFunctions(fixture());
  buildGamePackages({ outDir: b.out, sourceDirs: [b.source], allowSynthetic: true });
  assert.deepEqual(readFileSync(join(a.out, "game-packages.json")), readFileSync(join(b.out, "game-packages.json")));
  assert.deepEqual(bytes, readFileSync(join(b.out, product.path)));
});

test("the retained Rust fixture is byte-identical to SDK normalization of its authored source", () => {
  const source = join(scriptsRoot, "../games/fixture-source2/functions.jsonc");
  const product = join(scriptsRoot, "../games/fixture-source2/engine-functions.json");
  const bundle = normalizeFunctions("@fixture/a", parseFunctionFile(source, readFileSync(source, "utf8")));
  assert.equal(bundle.functions.length, 1);
  assert.deepEqual(Buffer.from(engineFunctionsArchive(bundle)), readFileSync(product));
});

test("absent functions omit the product; an authored empty file emits an empty bundle", () => {
  const absent = buildFixture();
  assert.equal(absent.manifest.packages[0].functions, undefined);
  const f = withFunctions(fixture(), '{"schemaVersion":2,"functions":{}}');
  buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true });
  const product = JSON.parse(readFileSync(join(f.out, "game-packages.json"))).packages[0].functions;
  assert.deepEqual(JSON.parse(readFileSync(join(f.out, product.path))).functions, []);
  rmSync(join(f.source, "functions.jsonc"));
  const path = join(f.source, "game-package.jsonc");
  const sourceManifest = JSON.parse(readFileSync(path, "utf8"));
  delete sourceManifest.functionsFile;
  writeFileSync(path, JSON.stringify(sourceManifest));
  buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true });
  assert.equal(existsSync(join(f.out, product.path)), false);
});

test("invalid or duplicate function source never writes final artifacts", () => {
  for (const text of [
    '{"schemaVersion":2,"functions":{},"functions":{}}',
    '{"schemaVersion":2,"functions":{"x":{"target":{"module":"libserver.so","pattern":"55","validate":{"prologue":"55"}},"unknown":1}}}',
    '{"schemaVersion":2,"functions":{},}',
  ]) {
    const f = withFunctions(fixture(), text);
    assert.throws(() => buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true }));
    assert.equal(existsSync(join(f.out, "game-packages.json")), false);
    assert.equal(existsSync(join(f.out, "game-packages/cs2/index.js")), false);
  }
});

test("a failed preparation preserves the previous complete build; failed publication removes its manifest", () => {
  const f = withAdapters(fixture());
  const old = built(f);
  writeFileSync(join(f.out, "operator-note.txt"), "keep");
  writeFileSync(join(f.out, "game-packages/cs2/operator-note.txt"), "keep");
  writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), "{broken");
  assert.throws(() => built(f), /invalid|JSON/i);
  assert.deepEqual(readFileSync(join(f.out, "game-packages.json")), old.manifest);
  assert.deepEqual(readFileSync(join(f.out, "game-packages/cs2/index.js")), old.bootstrap);

  writeFileSync(join(f.source, "adapters/contracts/legacy.acquire.v1.json"), acquireContract);
  withFunctions(f);
  mkdirSync(join(f.out, "game-packages/cs2/engine-functions.json"));
  writeFileSync(join(f.out, "game-packages/cs2/engine-functions.json/sentinel"), "block");
  assert.throws(() => built(f));
  assert.equal(existsSync(join(f.out, "game-packages.json")), false);
  assert.equal(existsSync(join(f.out, "game-packages/cs2/index.js")), true);
  assert.equal(readFileSync(join(f.out, "operator-note.txt"), "utf8"), "keep");
  assert.equal(readFileSync(join(f.out, "game-packages/cs2/operator-note.txt"), "utf8"), "keep");
});

test("direct rebuild rejects symlinked package output parent before invalidating a previous build", () => {
  const f = withAdapters(fixture());
  const prior = built(f);
  const outside = join(f.root, "outside-packages");
  renameSync(join(f.out, "game-packages"), outside);
  writeFileSync(join(outside, "sentinel"), "keep");
  symlinkSync(outside, join(f.out, "game-packages"));
  assert.throws(() => built(f), /symlink|output|destination/i);
  assert.deepEqual(readFileSync(join(f.out, "game-packages.json")), prior.manifest);
  assert.deepEqual(readFileSync(join(outside, "cs2/index.js")), prior.bootstrap);
  assert.equal(readFileSync(join(outside, "sentinel"), "utf8"), "keep");
});

test("direct rebuild rejects symlinked owner output parent before obsolete artifact removal", () => {
  const f = withFunctions(fixture());
  const prior = built(f);
  const functionPath = join(f.out, "game-packages/cs2/engine-functions.json");
  const oldFunctions = readFileSync(functionPath);
  const outside = join(f.root, "outside-owner");
  renameSync(join(f.out, "game-packages/cs2"), outside);
  writeFileSync(join(outside, "sentinel"), "keep");
  symlinkSync(outside, join(f.out, "game-packages/cs2"));
  const sourceManifest = join(f.source, "game-package.jsonc");
  const declared = JSON.parse(readFileSync(sourceManifest, "utf8"));
  delete declared.functionsFile;
  writeFileSync(sourceManifest, JSON.stringify(declared));
  assert.throws(() => built(f), /symlink|output|destination/i);
  assert.deepEqual(readFileSync(join(f.out, "game-packages.json")), prior.manifest);
  assert.deepEqual(readFileSync(join(outside, "index.js")), prior.bootstrap);
  assert.deepEqual(readFileSync(join(outside, "engine-functions.json")), oldFunctions);
  assert.equal(readFileSync(join(outside, "sentinel"), "utf8"), "keep");
});

test("function source must be a distinct confined regular input", () => {
  for (const name of ["../escape.jsonc", "js/a.js", "gamedata/game.cs2.jsonc"]) {
    const f = withFunctions(fixture());
    writeFileSync(join(f.source, "game-package.jsonc"), readFileSync(join(f.source, "game-package.jsonc"), "utf8").replace("functions.jsonc", name));
    assert.throws(() => buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true }));
  }
  const f = withFunctions(fixture());
  symlinkSync(join(f.root, "outside.jsonc"), join(f.source, "escape.jsonc"));
  writeFileSync(join(f.root, "outside.jsonc"), '{"schemaVersion":2,"functions":{}}');
  writeFileSync(join(f.source, "game-package.jsonc"), readFileSync(join(f.source, "game-package.jsonc"), "utf8").replace("functions.jsonc", "escape.jsonc"));
  assert.throws(() => buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true }), /escapes/);
});

test("source gamedata roots stay relative and function inputs must be files", () => {
  const f = fixture();
  writeFileSync(join(f.source, "game-package.jsonc"), readFileSync(join(f.source, "game-package.jsonc"), "utf8").replace('"gamedataRoot": "gamedata"', `"gamedataRoot": "${join(f.source, "gamedata")}"`));
  assert.throws(() => buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true }), /confined|root|path/);
  const g = withFunctions(fixture());
  mkdirSync(join(g.source, "directory"));
  writeFileSync(join(g.source, "game-package.jsonc"), readFileSync(join(g.source, "game-package.jsonc"), "utf8").replace("functions.jsonc", "directory"));
  assert.throws(() => buildGamePackages({ outDir: g.out, sourceDirs: [g.source], allowSynthetic: true }), /regular file/);
});

test("direct addon packaging fails before touching dist when locked TypeScript is missing", () => {
  const root = mkdtempSync(join(tmpdir(), "s2-package-deps-"));
  mkdirSync(join(root, "scripts/lib"), { recursive: true });
  copyFileSync(join(scriptsRoot, "package-addon.sh"), join(root, "scripts/package-addon.sh"));
  copyFileSync(join(scriptsRoot, "lib/check-game-package-deps.mjs"), join(root, "scripts/lib/check-game-package-deps.mjs"));
  mkdirSync(join(root, "dist/addons"), { recursive: true });
  writeFileSync(join(root, "dist/addons/sentinel"), "keep");
  const result = spawnSync("bash", [join(root, "scripts/package-addon.sh")], { cwd: root, encoding: "utf8" });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /npm ci.*repo root|repo root.*npm ci/i);
  assert.equal(readFileSync(join(root, "dist/addons/sentinel"), "utf8"), "keep");
});

test("native CI installs locked parser dependencies before its real-package core test", () => {
  const script = readFileSync(join(scriptsRoot, "ci-native.sh"), "utf8");
  const installs = [...script.matchAll(/^\s*npm ci\s*$/gm)];
  assert.equal(installs.length, 1);
  const coreTests = [...script.matchAll(/^cargo test -p s2script-core\s*$/gm)];
  assert.equal(coreTests.length, 1);
  const coreTest = coreTests[0].index;
  assert.ok(installs[0].index < coreTest);
  const dependencyCheck = script.indexOf("node scripts/lib/check-game-package-deps.mjs");
  assert.ok(installs[0].index < dependencyCheck && dependencyCheck < coreTest);
});

test("rejects absent inputs, escapes, malformed JSONC, and unapproved synthetic packages", () => {
  const f = fixture();
  assert.throws(() => buildGamePackages({ outDir: f.out, sourceDirs: [f.source] }), /synthetic|first-party/);
  writeFileSync(join(f.source, "game-package.jsonc"), readFileSync(join(f.source, "game-package.jsonc"), "utf8").replace('"js/b.js"', '"../outside.js"'));
  assert.throws(() => buildGamePackages({ outDir: f.out, sourceDirs: [f.source], allowSynthetic: true }), /path|confined/);
  const g = fixture();
  writeFileSync(join(g.source, "gamedata/game.cs2.jsonc"), '{"broken": }');
  assert.throws(() => buildGamePackages({ outDir: g.out, sourceDirs: [g.source], allowSynthetic: true }), /game.cs2.jsonc/);
  const h = fixture();
  writeFileSync(join(h.source, "game-package.jsonc"), readFileSync(join(h.source, "game-package.jsonc"), "utf8").replace('"js/b.js"', '"js/missing.js"'));
  assert.throws(() => buildGamePackages({ outDir: h.out, sourceDirs: [h.source], allowSynthetic: true }), /missing.js/);
  const i = fixture();
  writeFileSync(join(i.root, "outside.js"), "outside");
  symlinkSync(join(i.root, "outside.js"), join(i.source, "js/escape.js"));
  writeFileSync(join(i.source, "game-package.jsonc"), readFileSync(join(i.source, "game-package.jsonc"), "utf8").replace('"js/b.js"', '"js/escape.js"'));
  assert.throws(() => buildGamePackages({ outDir: i.out, sourceDirs: [i.source], allowSynthetic: true }), /escapes package root/);
});
