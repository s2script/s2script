import { test } from "node:test";
import assert from "node:assert/strict";
import { copyFileSync, existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
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

});

test("same inputs produce byte-identical manifest and artifacts", () => {
  const a = buildFixture(), b = buildFixture();
  for (const key of ["manifestBytes", "bootstrap", "gamedata"]) assert.deepEqual(a[key], b[key]);
});

function withFunctions(f, text = JSON.stringify({ schemaVersion: 2, functions: {
  scalar: { target: { module: "libserver.so", pattern: "55 48", validate: { prologue: "55 48" } },
    parameters: [{ name: "enabled", type: "bool" }], returns: "void", surfaces: ["call"] },
} })) {
  writeFileSync(join(f.source, "functions.jsonc"), text);
  writeFileSync(join(f.source, "game-package.jsonc"),
    readFileSync(join(f.source, "game-package.jsonc"), "utf8").replace('"gamedataRoot": "gamedata"',
      '"gamedataRoot": "gamedata", "functionsFile": "functions.jsonc"'));
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
  writeFileSync(join(f.source, "game-package.jsonc"), readFileSync(join(f.source, "game-package.jsonc"), "utf8").replace(', "functionsFile": "functions.jsonc"', ''));
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
