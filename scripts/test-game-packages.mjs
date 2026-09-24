import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { buildGamePackages } from "./build-game-packages.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");

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
  writeFileSync(join(source, "js/b.js"), "globalThis.b = 2;\n");
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

test("emits the frozen deployed schema and lowercase SHA-256", () => {
  const out = buildFixture();
  assert.deepEqual(Object.keys(out.manifest), ["schemaVersion", "packages"]);
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

test("default build contains only CS2 and preserves transitional deployed bytes", () => {
  const out = mkdtempSync(join(tmpdir(), "s2-game-package-default-"));
  const manifest = buildGamePackages({ outDir: out });
  assert.deepEqual(manifest.packages.map(p => p.id), ["@s2script/cs2"]);
  assert.deepEqual(readFileSync(join(out, "game-packages/cs2/index.js")),
    readFileSync(join(out, "js/pawn.js")));
  for (const name of ["master.gamedata.jsonc", "game.cs2.jsonc"])
    assert.deepEqual(readFileSync(join(out, "gamedata/cs2", name)),
      readFileSync(join(repoRoot, "games/cs2/gamedata", name)));
  assert.equal(existsSync(join(out, "gamedata/cs2/custom")), false);
});

test("same inputs produce byte-identical manifest and artifacts", () => {
  const a = buildFixture(), b = buildFixture();
  for (const key of ["manifestBytes", "bootstrap", "gamedata"]) assert.deepEqual(a[key], b[key]);
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
