#!/usr/bin/env node
// Build first-party game package artifacts from their ordered source manifests.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { stripJsonComments } from "../packages/sdk/src/gamedata/jsonc.ts";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const firstParty = resolve(repoRoot, "games/cs2");
const sha256 = bytes => createHash("sha256").update(bytes).digest("hex");
const pretty = value => Buffer.from(JSON.stringify(value, null, 2) + "\n");

function canonical(value) {
  if (Array.isArray(value)) return value.map(canonical);
  if (value !== null && typeof value === "object")
    return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])]));
  return value;
}

function parseJsonc(file) {
  try { return JSON.parse(stripJsonComments(readFileSync(file, "utf8"))); }
  catch (error) { throw new Error(`${file}: ${error.message}`); }
}

function confined(root, name) {
  if (typeof name !== "string" || !name || isAbsolute(name) || name.split(/[\\/]/).some(part => !part || part === "." || part === "..") || name.includes("\\"))
    throw new Error(`invalid confined path: ${name}`);
  const path = resolve(root, name);
  if (!existsSync(path)) throw new Error(`missing input: ${path}`);
  const actualRoot = realpathSync(root), actual = realpathSync(path);
  if (actual !== actualRoot && !actual.startsWith(actualRoot + sep))
    throw new Error(`path escapes package root: ${name}`);
  return path;
}

function write(outDir, name, bytes) {
  const dest = join(outDir, name);
  mkdirSync(dirname(dest), { recursive: true });
  writeFileSync(dest, bytes);
}

export function buildGamePackages({ outDir, sourceDirs = [firstParty], allowSynthetic = false } = {}) {
  if (!outDir) throw new Error("--out is required");
  if (!allowSynthetic && (sourceDirs.length !== 1 || resolve(sourceDirs[0]) !== firstParty))
    throw new Error("synthetic packages require explicit test opt-in");
  const packages = [];
  const ids = new Set(), owners = new Set();
  for (const source of sourceDirs) {
    const root = resolve(source);
    const manifest = parseJsonc(confined(root, "game-package.jsonc"));
    const { schemaVersion, id, match, gamedataOwner, bootstrapInputs, gamedataRoot } = manifest;
    if (schemaVersion !== 1 || typeof id !== "string" || !/^@[a-z0-9-]+\/[a-z0-9-]+$/.test(id) ||
        typeof match?.engine !== "string" || typeof match?.game !== "string" ||
        typeof gamedataOwner !== "string" || !/^[a-z0-9-]+$/.test(gamedataOwner) ||
        !Array.isArray(bootstrapInputs) || bootstrapInputs.length === 0 || typeof gamedataRoot !== "string")
      throw new Error(`${root}: invalid game package source manifest`);
    if (ids.has(id) || owners.has(gamedataOwner)) throw new Error(`duplicate package id or gamedata owner: ${id}`);
    ids.add(id); owners.add(gamedataOwner);
    const js = Buffer.concat(bootstrapInputs.map(name => readFileSync(confined(root, name))));
    // Keep the old concatenation bytes exactly; source files end in a newline except generated schema.
    const bootstrap = Buffer.from(js.toString().replace(/\n*$/, "") + "\n");
    const gdRoot = confined(root, gamedataRoot);
    const masterName = "master.gamedata.jsonc";
    const master = parseJsonc(confined(gdRoot, masterName));
    if (!Array.isArray(master.files)) throw new Error(`${masterName}: missing files array`);
    const names = [masterName, ...master.files.map(entry => entry.file)];
    if (new Set(names).size !== names.length) throw new Error(`${masterName}: duplicate file`);
    const files = names.map(name => ({ path: name, document: canonical(parseJsonc(confined(gdRoot, name))) }));
    const gamedata = pretty({ schemaVersion: 1, owner: gamedataOwner, files });
    const prefix = `game-packages/${gamedataOwner}`;
    const bootstrapPath = `${prefix}/index.js`, gamedataPath = `${prefix}/gamedata.json`;
    write(outDir, bootstrapPath, bootstrap);
    write(outDir, gamedataPath, gamedata);
    // Transitional deployed paths: runtime readers switch to the manifest in CORE-04.
    write(outDir, "js/pawn.js", bootstrap);
    for (const name of names) write(outDir, `gamedata/${gamedataOwner}/${name}`, readFileSync(confined(gdRoot, name)));
    packages.push({
      id, match: { engine: match.engine, game: match.game }, gamedataOwner,
      bootstrap: { path: bootstrapPath, sha256: sha256(bootstrap) },
      gamedata: { path: gamedataPath, sha256: sha256(gamedata) },
    });
  }
  const deployed = { schemaVersion: 1, packages };
  write(outDir, "game-packages.json", pretty(deployed));
  return deployed;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  let outDir, testSource;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--out") outDir = args[++i];
    else if (args[i] === "--test-source") testSource = args[++i];
    else throw new Error(`unknown argument: ${args[i]}`);
  }
  buildGamePackages({ outDir, sourceDirs: testSource ? [testSource] : [firstParty], allowSynthetic: !!testSource });
}
