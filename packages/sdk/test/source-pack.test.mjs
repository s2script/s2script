/**
 * `s2s pack` / the source tarball `s2s deploy` uploads: npm-packlist's file selection and the
 * `npm pack` tarball shape.
 *
 * Run via: node --experimental-strip-types --no-warnings --test test/source-pack.test.mjs
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { gunzipSync } from "node:zlib";

import {
  MAX_SOURCE_BYTES,
  assertSourceSize,
  listPackageFiles,
  packSourceTarball,
  tarballFilename,
} from "../src/source-pack.ts";
import { packTar, splitUstarPath } from "../src/tar.ts";
import { formatPackReport, packPackage } from "../src/commands/pack.ts";

/** A package directory holding `files` (path → content); package.json defaults to name+version. */
function mkPkg(files, pkg = {}) {
  const dir = mkdtempSync(join(tmpdir(), "s2s-pack-"));
  writeTree(dir, { "package.json": JSON.stringify({ name: "@edge/greeter", version: "1.2.3", ...pkg }), ...files });
  return dir;
}

function writeTree(dir, files) {
  for (const [p, content] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, p)), { recursive: true });
    writeFileSync(join(dir, p), content);
  }
}

/** Parse a gzipped ustar archive into { path → { content, mode, mtime } }. */
function readTar(tgz) {
  const tar = gunzipSync(tgz);
  const out = {};
  for (let off = 0; off + 512 <= tar.length; ) {
    const h = tar.subarray(off, off + 512);
    if (h.every((b) => b === 0)) break;
    const str = (a, b) => h.subarray(a, b).toString("utf8").replace(/\0.*$/s, "");
    const name = str(0, 100);
    const prefix = str(345, 500);
    const size = parseInt(str(124, 136), 8);
    const path = prefix ? `${prefix}/${name}` : name;
    out[path] = {
      content: tar.subarray(off + 512, off + 512 + size).toString("utf8"),
      mode: parseInt(str(100, 108), 8),
      mtime: parseInt(str(136, 148), 8),
    };
    off += 512 + Math.ceil(size / 512) * 512;
  }
  return out;
}

test("without files or ignore files, everything is packed except npm's never-packed set", () => {
  const dir = mkPkg({
    "src/plugin.ts": "export {}",
    "README.md": "# hi",
    "node_modules/x/index.js": "",
    "lib/node_modules/y.js": "", // npm only skips the ROOT node_modules
    ".git/HEAD": "",
    ".npmrc": "//registry/:_authToken=secret",
    "package-lock.json": "{}",
    "pnpm-lock.yaml": "",
    ".DS_Store": "",
    "src/.plugin.ts.swp": "",
    "notes.orig": "",
  });
  assert.deepEqual(listPackageFiles(dir), ["README.md", "lib/node_modules/y.js", "package.json", "src/plugin.ts"]);
});

test(".env files are never packed, but their templates are", () => {
  const dir = mkPkg({ ".env": "KEY=1", ".env.local": "KEY=2", ".env.example": "KEY=", "src/.env": "" });
  assert.deepEqual(listPackageFiles(dir), [".env.example", "package.json"]);
});

test(".gitignore applies when there is no .npmignore, in every directory", () => {
  const dir = mkPkg({
    ".gitignore": "dist/\n*.s2sp\n",
    "dist/greeter.s2sp": "zip",
    "src/plugin.ts": "",
    "src/.gitignore": "scratch.ts\n",
    "src/scratch.ts": "",
  });
  // The ignore files themselves are never packed.
  assert.deepEqual(listPackageFiles(dir), ["package.json", "src/plugin.ts"]);
});

test(".npmignore replaces .gitignore in its directory", () => {
  const dir = mkPkg({
    ".gitignore": "dist/\n",
    ".npmignore": "test/\n",
    "dist/out.js": "",
    "test/a.test.ts": "",
    "src/plugin.ts": "",
  });
  assert.deepEqual(listPackageFiles(dir), ["dist/out.js", "package.json", "src/plugin.ts"]);
});

test("a negated ignore pattern re-includes, as in git", () => {
  const dir = mkPkg({ ".npmignore": "*.md\n!CHANGELOG.md\n", "CHANGELOG.md": "", "NOTES.md": "" });
  assert.deepEqual(listPackageFiles(dir), ["CHANGELOG.md", "package.json"]);
});

test("files limits the tarball to what it lists, plus the always-packed files", () => {
  const dir = mkPkg(
    {
      "src/plugin.ts": "",
      "src/deep/util.ts": "",
      "gamedata/cs2.jsonc": "",
      "test/a.test.ts": "",
      "README.md": "",
      "LICENSE-MIT": "",
      "scripts/bin.js": "",
      ".gitignore": "src/\n", // the root ignore file is not read when files is set
    },
    { files: ["src", "gamedata/*.jsonc", "!src/deep"], bin: { greet: "./scripts/bin.js" } },
  );
  // LICENSE-MIT is not npm's LICENSE{,.ext}, so like npm it needs listing.
  assert.deepEqual(listPackageFiles(dir), [
    "README.md",
    "gamedata/cs2.jsonc",
    "package.json",
    "scripts/bin.js",
    "src/plugin.ts",
  ]);
});

test("nested ignore files still apply under files", () => {
  const dir = mkPkg({ "src/plugin.ts": "", "src/.npmignore": "*.snap\n", "src/x.snap": "" }, { files: ["src"] });
  assert.deepEqual(listPackageFiles(dir), ["package.json", "src/plugin.ts"]);
});

/** Matches npm 10 (`npm pack --dry-run` on the same tree). */
test("a workspace member honours the ignore files above it, matched relative to the member", () => {
  const root = mkdtempSync(join(tmpdir(), "s2s-pack-ws-"));
  writeTree(root, {
    "package.json": JSON.stringify({ name: "ws", private: true, workspaces: ["plugins/*"] }),
    ".gitignore": "dist/\nplugins/greeter/notes/\n",
    "plugins/.gitignore": "*.log\n",
    "plugins/greeter/package.json": JSON.stringify({ name: "greeter", version: "1.0.0" }),
    "plugins/greeter/src/plugin.ts": "",
    "plugins/greeter/dist/greeter.s2sp": "",
    "plugins/greeter/build.log": "",
    "plugins/greeter/notes/todo.md": "",
    "plugins/filed/package.json": JSON.stringify({ name: "filed", version: "1.0.0", files: ["dist"] }),
    "plugins/filed/dist/x.js": "",
    "plugins/filed/dist/x.log": "",
  });
  assert.deepEqual(listPackageFiles(join(root, "plugins/greeter")), ["notes/todo.md", "package.json", "src/plugin.ts"]);
  // With files, npm reads none of the workspace's ignore files.
  assert.deepEqual(listPackageFiles(join(root, "plugins/filed")), ["dist/x.js", "dist/x.log", "package.json"]);
});

test("symlinks are skipped", () => {
  const dir = mkPkg({ "src/plugin.ts": "" });
  symlinkSync(join(dir, "src"), join(dir, "linked"));
  symlinkSync(join(dir, "src/plugin.ts"), join(dir, "alias.ts"));
  assert.deepEqual(listPackageFiles(dir), ["package.json", "src/plugin.ts"]);
});

test("packSourceTarball lays files out under package/ with npm's modes and fixed mtime", () => {
  const dir = mkPkg({ "src/plugin.ts": "export const x = 1;\n", "bin/run.sh": "#!/bin/sh\n" });
  chmodSync(join(dir, "bin/run.sh"), 0o755);
  const a = packSourceTarball(dir);
  const entries = readTar(a.tarball);
  assert.deepEqual(Object.keys(entries).sort(), ["package/bin/run.sh", "package/package.json", "package/src/plugin.ts"]);
  assert.equal(entries["package/src/plugin.ts"].content, "export const x = 1;\n");
  assert.equal(entries["package/src/plugin.ts"].mode, 0o644);
  assert.equal(entries["package/bin/run.sh"].mode, 0o755);
  assert.equal(entries["package/package.json"].mtime, 499162500);
  assert.equal(a.files.length, 3);
  // Same files, same bytes: the tarball's hash is reproducible.
  assert.deepEqual(packSourceTarball(dir).tarball, a.tarball);
});

test("tar paths over 100 bytes use the ustar prefix; unsplittable ones are refused", () => {
  const long = `package/${"d".repeat(90)}/${"f".repeat(60)}.ts`;
  assert.deepEqual(splitUstarPath(long), { prefix: `package/${"d".repeat(90)}`, name: `${"f".repeat(60)}.ts` });
  const entries = readTar(packTar([{ path: long, content: Buffer.from("ok") }]));
  assert.equal(entries[long].content, "ok");
  assert.throws(() => splitUstarPath(`package/${"x".repeat(120)}.ts`), /path too long for a tar archive/);
});

test("assertSourceSize refuses a tarball over the registry cap", () => {
  const big = { tarball: Buffer.alloc(MAX_SOURCE_BYTES + 1), files: [], unpackedSize: 0 };
  assert.throws(() => assertSourceSize(big, "@edge/greeter"), /accepts at most 16\.0 MiB.*s2s pack --dry-run/s);
  assertSourceSize({ tarball: Buffer.alloc(10), files: [], unpackedSize: 0 }, "@edge/greeter");
});

test("tarballFilename follows npm: @scope/name → scope-name-<version>.tgz", () => {
  assert.equal(tarballFilename("@edge/greeter", "1.2.3"), "edge-greeter-1.2.3.tgz");
  assert.equal(tarballFilename("greeter", "0.1.0"), "greeter-0.1.0.tgz");
});

test("packPackage reports npm pack's details", () => {
  const dir = mkPkg({ "src/plugin.ts": "export {}" });
  const { result } = packPackage(dir);
  assert.equal(result.id, "@edge/greeter@1.2.3");
  assert.equal(result.filename, "edge-greeter-1.2.3.tgz");
  assert.equal(result.entryCount, 2);
  assert.match(result.integrity, /^sha512-/);
  const report = formatPackReport(result);
  assert.match(report, /Tarball Contents\n.*package\.json\n.*src\/plugin\.ts\nTarball Details/s);
  assert.match(report, /total files:   2/);
});
