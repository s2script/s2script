#!/usr/bin/env node
// Build first-party game package artifacts from their ordered source manifests.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { stripJsonComments } from "../packages/sdk/src/gamedata/jsonc.ts";
import { parseFunctionFile } from "../packages/sdk/src/engine-functions/parse.ts";
import { normalizeFunctions } from "../packages/sdk/src/engine-functions/normalize.ts";
import { engineFunctionsArchive, engineFunctionsManifest } from "../packages/sdk/src/engine-functions/archive.ts";

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
  if (!statSync(actual).isFile()) throw new Error(`input is not a regular file: ${name}`);
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
  const packages = [], outputs = new Map();
  const ids = new Set(), owners = new Set();
  for (const source of sourceDirs) {
    const root = resolve(source);
    const manifest = parseJsonc(confined(root, "game-package.jsonc"));
    const { schemaVersion, id, match, gamedataOwner, bootstrapInputs, gamedataRoot, functionsFile } = manifest;
    if (schemaVersion !== 1 || typeof id !== "string" || !/^@[a-z0-9-]+\/[a-z0-9-]+$/.test(id) ||
        typeof match?.engine !== "string" || typeof match?.game !== "string" ||
        typeof gamedataOwner !== "string" || !/^[a-z0-9-]+$/.test(gamedataOwner) ||
        !Array.isArray(bootstrapInputs) || bootstrapInputs.length === 0 || typeof gamedataRoot !== "string" ||
        (functionsFile !== undefined && typeof functionsFile !== "string") ||
        Object.keys(manifest).some(key => !["schemaVersion", "id", "match", "gamedataOwner", "bootstrapInputs", "gamedataRoot", "functionsFile"].includes(key)) ||
        Object.keys(match).some(key => !["engine", "game"].includes(key)))
      throw new Error(`${root}: invalid game package source manifest`);
    if (ids.has(id) || owners.has(gamedataOwner)) throw new Error(`duplicate package id or gamedata owner: ${id}`);
    ids.add(id); owners.add(gamedataOwner);
    const inputPaths = new Set();
    const input = name => {
      const path = confined(root, name);
      const resolved = realpathSync(path);
      if (inputPaths.has(resolved)) throw new Error(`duplicate package input path: ${name}`);
      inputPaths.add(resolved);
      return path;
    };
    const js = Buffer.concat(bootstrapInputs.map(name => readFileSync(input(name))));
    // The final source expression supplies the package-local module export map.
    const bootstrap = Buffer.from(js.toString().replace(/\n*$/, "") + "\n");
    const gdRoot = resolve(root, gamedataRoot);
    const actualRoot = realpathSync(root), actualGdRoot = realpathSync(gdRoot);
    if (actualGdRoot !== actualRoot && !actualGdRoot.startsWith(actualRoot + sep) || !statSync(actualGdRoot).isDirectory())
      throw new Error(`invalid gamedata root: ${gamedataRoot}`);
    const masterName = "master.gamedata.jsonc";
    const master = parseJsonc(input(`${gamedataRoot}/${masterName}`));
    if (!Array.isArray(master.files)) throw new Error(`${masterName}: missing files array`);
    const names = [masterName, ...master.files.map(entry => entry.file)];
    if (new Set(names).size !== names.length) throw new Error(`${masterName}: duplicate file`);
    const files = [{ path: masterName, document: canonical(master) },
      ...names.slice(1).map(name => ({ path: name, document: canonical(parseJsonc(input(`${gamedataRoot}/${name}`))) }))];
    const gamedata = pretty({ schemaVersion: 1, owner: gamedataOwner, files });
    const prefix = `game-packages/${gamedataOwner}`;
    const bootstrapPath = `${prefix}/index.js`, gamedataPath = `${prefix}/gamedata.json`;
    outputs.set(bootstrapPath, bootstrap);
    outputs.set(gamedataPath, gamedata);
    const record = {
      id, match: { engine: match.engine, game: match.game }, gamedataOwner,
      bootstrap: { path: bootstrapPath, sha256: sha256(bootstrap) },
      gamedata: { path: gamedataPath, sha256: sha256(gamedata) },
    };
    if (functionsFile !== undefined) {
      const path = input(functionsFile);
      const parsed = parseFunctionFile(path, readFileSync(path, "utf8"));
      const bundle = normalizeFunctions(id, parsed);
      const bytes = Buffer.from(engineFunctionsArchive(bundle));
      const functionPath = `${prefix}/engine-functions.json`;
      outputs.set(functionPath, bytes);
      record.functions = { path: functionPath, sha256: sha256(bytes), ...engineFunctionsManifest(bundle) };
    }
    packages.push(record);
  }
  const deployed = { schemaVersion: 2, packages };
  rmSync(join(outDir, "game-packages"), { recursive: true, force: true });
  for (const [name, bytes] of outputs) write(outDir, name, bytes);
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
