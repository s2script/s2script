#!/usr/bin/env node
// Build first-party game package artifacts from their ordered source manifests.
import { createHash } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { stripJsonComments } from "../packages/sdk/src/gamedata/jsonc.ts";
import { parseFunctionFile } from "../packages/sdk/src/engine-functions/parse.ts";
import { normalizeFunctions } from "../packages/sdk/src/engine-functions/normalize.ts";
import { engineFunctionsArchive, engineFunctionsManifest } from "../packages/sdk/src/engine-functions/archive.ts";
import { canonicalJson, hashCanonical } from "../packages/sdk/src/engine-functions/canonical-json.ts";
import { stackCopyBytes } from "../packages/sdk/src/engine-functions/normalize.ts";

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

function parseJsonc(file, uniqueKeys = false) {
  try {
    const stripped = stripJsonComments(readFileSync(file, "utf8"));
    if (uniqueKeys) rejectDuplicateKeys(file, stripped);
    return JSON.parse(stripped);
  }
  catch (error) { throw new Error(`${file}: ${error.message}`); }
}

function rejectDuplicateKeys(path, text) {
  const ast = ts.parseJsonText(path, text);
  if (ast.parseDiagnostics?.length) throw new Error(`${path}: invalid JSON`);
  const visit = node => {
    if (ts.isObjectLiteralExpression(node)) {
      const seen = new Set();
      for (const property of node.properties) {
        if (!ts.isPropertyAssignment(property) || !ts.isStringLiteral(property.name))
          throw new Error(`${path}: invalid JSON property`);
        if (seen.has(property.name.text)) throw new Error(`${path}: duplicate JSON key ${property.name.text}`);
        seen.add(property.name.text);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(ast);
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

const MAX_ADAPTERS = 16;
const MAX_CONTRACT_BYTES = 64 * 1024;
const MAX_ADAPTER_SOURCE_BYTES = 1024 * 1024;
const adapterId = /^[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)+\.v1$/;
const adapterKeys = ["version", "contract", "source"];
const contractKeys = ["frame", "id", "preDecision", "semantics", "subscriberDelivery", "timing", "version"];
const utf8 = new TextDecoder("utf-8", { fatal: true });

function parseContract(path, bytes, id) {
  if (bytes.length === 0 || bytes.length > MAX_CONTRACT_BYTES)
    throw new Error(`${path}: adapter contract size limit`);
  let text;
  try { text = utf8.decode(bytes); }
  catch { throw new Error(`${path}: invalid adapter contract UTF-8`); }
  rejectDuplicateKeys(path, text);
  let document;
  try { document = JSON.parse(text); }
  catch { throw new Error(`${path}: invalid adapter contract JSON`); }
  if (document === null || typeof document !== "object" || Array.isArray(document) ||
      document.id !== id || document.version !== 1 ||
      Object.keys(document).sort().join("\0") !== contractKeys.slice().sort().join("\0") ||
      typeof document.frame !== "object" || document.frame === null || Array.isArray(document.frame) ||
      typeof document.semantics !== "object" || document.semantics === null || Array.isArray(document.semantics) ||
      typeof document.timing !== "object" || document.timing === null || Array.isArray(document.timing) ||
      typeof document.preDecision !== "string" || !document.preDecision ||
      typeof document.subscriberDelivery !== "string" || !document.subscriberDelivery)
    throw new Error(`${path}: invalid adapter contract id, version, or document shape`);
  let nodes = 0;
  const checkBounds = (value, depth) => {
    if (++nodes > 1024 || depth > 32) throw new Error(`${path}: adapter contract structure limit`);
    if (typeof value === "number" && !Number.isFinite(value)) throw new Error(`${path}: nonfinite adapter contract number`);
    if (value && typeof value === "object")
      for (const child of Object.values(value)) checkBounds(child, depth + 1);
  };
  checkBounds(document, 0);
  return sha256(Buffer.from(canonicalJson(document)));
}

// Trusted (host-verified, first-party-only) function declarations. The builder seals everything a
// build can know — physical fingerprints, stack sizes, policy and adapter contract hashes, the
// schema-catalog fallback of every record offset — and leaves the layout the HOST must choose to
// commit time: the resolved target (from the merged shipped + operator-custom gamedata, by
// signature NAME) and the selected record offsets (live schema first). Any shape the Rust decoder
// would refuse is refused here too, so a bad declaration fails the build, not a server boot.
const MAX_TRUSTED_BYTES = 1024 * 1024;
const trustedKeys = ["schemaVersion", "offsets", "functions"];
const trustedFunctionKeys = ["localName", "requirement", "signatureName", "signature", "policy", "adapter"];
const identifierPattern = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
const PLATFORM = "linux-x86_64-sysv";

function plainObject(value, where) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error(`${where} must be an object`);
  return value;
}
function exactKeys(value, allowed, where, required = allowed) {
  for (const key of Object.keys(value)) if (!allowed.includes(key)) throw new Error(`${where}: unknown field ${key}`);
  for (const key of required) if (!(key in value)) throw new Error(`${where}: missing ${key}`);
}

function catalogOffset(catalog, className, field, where) {
  const fields = catalog?.[className]?.fields;
  const row = Array.isArray(fields) ? fields.find(f => f?.name === field) : undefined;
  if (!row || !Number.isInteger(row.offset) || row.offset < 0 || row.offset > 0xffffffff)
    throw new Error(`${where}: ${className}::${field} is absent from the schema catalog`);
  return row.offset;
}

function trustedArtifact({ path, text, id, adapterHashes, signatures, catalog }) {
  if (Buffer.byteLength(text) > MAX_TRUSTED_BYTES) throw new Error(`${path}: trusted functions size limit`);
  const stripped = stripJsonComments(text);
  rejectDuplicateKeys(path, stripped);
  let source;
  try { source = JSON.parse(stripped); }
  catch (error) { throw new Error(`${path}: ${error.message}`); }
  plainObject(source, path);
  exactKeys(source, trustedKeys, path);
  if (source.schemaVersion !== 1) throw new Error(`${path}: unsupported schemaVersion`);
  const offsets = {};
  for (const [key, entry] of Object.entries(plainObject(source.offsets, `${path}.offsets`))) {
    const where = `${path}.offsets[${JSON.stringify(key)}]`;
    plainObject(entry, where);
    exactKeys(entry, ["class", "field"], where);
    if (!key || key.length > 256 || key.includes("\0") || typeof entry.class !== "string" || !entry.class ||
        typeof entry.field !== "string" || !entry.field)
      throw new Error(`${where}: invalid offset key or schema name`);
    offsets[key] = { class: entry.class, field: entry.field, catalog: catalogOffset(catalog, entry.class, entry.field, where) };
  }
  if (!Array.isArray(source.functions) || source.functions.length > 256) throw new Error(`${path}.functions must be an array of at most 256`);
  const names = new Set(), used = new Set();
  const functions = source.functions.map((raw, index) => {
    const where = `${path}.functions[${index}]`;
    const f = plainObject(raw, where);
    exactKeys(f, trustedFunctionKeys, where, trustedFunctionKeys.filter(k => k !== "adapter"));
    if (typeof f.localName !== "string" || !identifierPattern.test(f.localName) || names.has(f.localName))
      throw new Error(`${where}: invalid or duplicate localName`);
    names.add(f.localName);
    if (f.requirement !== "optional" && f.requirement !== "required") throw new Error(`${where}: invalid requirement`);
    if (typeof f.signatureName !== "string" || !plainObject(signatures, "gamedata signatures")[f.signatureName]?.linuxsteamrt64)
      throw new Error(`${where}: gamedata signature ${JSON.stringify(f.signatureName)} has no linuxsteamrt64 entry`);
    const signature = structuredClone(plainObject(f.signature, `${where}.signature`));
    if ("fingerprint" in signature || "stackCopyBytes" in signature) throw new Error(`${where}.signature: fingerprint/stackCopyBytes are derived`);
    if (signature.platform !== PLATFORM || typeof signature.memberReceiver !== "boolean" || !Array.isArray(signature.parameters) ||
        !signature.returns || typeof signature.returns.native !== "string" || !Array.isArray(signature.instances))
      throw new Error(`${where}.signature: invalid platform/receiver/parameters/returns/instances`);
    const natives = signature.parameters.map(p => p?.native);
    const stack = stackCopyBytes(signature.memberReceiver ? "entity" : "none", natives);
    signature.fingerprint = `${PLATFORM}:${signature.memberReceiver ? "entity" : "none"}:${signature.returns.native}(${natives.join(",")})`;
    signature.stackCopyBytes = stack;
    for (const [i, instance] of signature.instances.entries()) {
      const at = `${where}.signature.instances[${i}]`;
      plainObject(instance, at);
      exactKeys(instance, ["codecId", "codecVersion", "kind", "record"], at);
      exactKeys(plainObject(instance.record, `${at}.record`), ["fields"], `${at}.record`);
      if (!Array.isArray(instance.record.fields) || instance.record.fields.length === 0) throw new Error(`${at}.record.fields must be non-empty`);
      for (const field of instance.record.fields) {
        plainObject(field, `${at}.record.fields`);
        if ("offset" in field) throw new Error(`${at}: record offsets are selected by the host, not declared`);
        if (!(field.offsetKey in offsets)) throw new Error(`${at}: unknown offsetKey ${JSON.stringify(field.offsetKey)}`);
        used.add(field.offsetKey);
      }
    }
    const policySource = plainObject(f.policy, `${where}.policy`);
    exactKeys(policySource, ["surfaces", "suppression"], `${where}.policy`);
    const policyBase = { id: "generic.v2", version: 1, surfaces: policySource.surfaces,
      selfCall: "bypass-own-hooks", suppression: policySource.suppression };
    const out = { localName: f.localName, requirement: f.requirement, signatureName: f.signatureName, signature,
      policy: { ...policyBase, contractHash: hashCanonical(policyBase) } };
    if (f.adapter !== undefined) {
      const adapter = plainObject(f.adapter, `${where}.adapter`);
      exactKeys(adapter, ["id", "postOverride"], `${where}.adapter`);
      if (typeof adapter.postOverride !== "boolean" || !(adapter.id in adapterHashes))
        throw new Error(`${where}.adapter: ${JSON.stringify(adapter.id)} is not a packaged adapter`);
      out.adapter = { id: adapter.id, contractHash: adapterHashes[adapter.id], postOverride: adapter.postOverride };
    }
    return out;
  });
  for (const key of Object.keys(offsets)) if (!used.has(key)) throw new Error(`${path}: unused offset key ${key}`);
  return pretty(canonical({ schemaVersion: 1, ownerId: id, offsets, functions }));
}

function existingArtifacts(outDir) {
  const path = join(outDir, "game-packages.json");
  if (!existsSync(path)) return [];
  let manifest;
  try { manifest = JSON.parse(readFileSync(path, "utf8")); }
  catch { return []; }
  const paths = [];
  if (!Array.isArray(manifest.packages)) return [];
  for (const product of manifest.packages) {
    for (const key of ["bootstrap", "gamedata", "functions", "trustedFunctions"]) {
      const name = product?.[key]?.path;
      if (typeof name === "string" && /^game-packages\/[a-z0-9-]+\/(?:index\.js|gamedata\.json|engine-functions\.json|trusted-functions\.json)$/.test(name))
        paths.push(name);
    }
  }
  return paths;
}

function checkOutputParent(outDir, name) {
  let parent = outDir;
  for (const part of dirname(name).split(/[\\/]/)) {
    parent = join(parent, part);
    const entry = lstatSync(parent, { throwIfNoEntry: false });
    if (entry && (entry.isSymbolicLink() || !entry.isDirectory()))
      throw new Error(`invalid builder output destination parent: ${parent}`);
  }
}

export function buildGamePackages({ outDir, sourceDirs = [firstParty], allowSynthetic = false } = {}) {
  if (!outDir) throw new Error("--out is required");
  if (!allowSynthetic && (sourceDirs.length !== 1 || resolve(sourceDirs[0]) !== firstParty))
    throw new Error("synthetic packages require explicit test opt-in");
  const packages = [], outputs = new Map();
  const ids = new Set(), owners = new Set();
  for (const source of sourceDirs) {
    const root = resolve(source);
    const manifest = parseJsonc(confined(root, "game-package.jsonc"), true);
    const { schemaVersion, id, match, gamedataOwner, bootstrapInputs, gamedataRoot, functionsFile, trustedFunctionsFile, adapters } = manifest;
    if (schemaVersion !== 1 || typeof id !== "string" || !/^@[a-z0-9-]+\/[a-z0-9-]+$/.test(id) ||
        typeof match?.engine !== "string" || typeof match?.game !== "string" ||
        typeof gamedataOwner !== "string" || !/^[a-z0-9-]+$/.test(gamedataOwner) ||
        !Array.isArray(bootstrapInputs) || bootstrapInputs.length === 0 || typeof gamedataRoot !== "string" ||
        (functionsFile !== undefined && typeof functionsFile !== "string") ||
        (trustedFunctionsFile !== undefined && typeof trustedFunctionsFile !== "string") ||
        (adapters !== undefined && (adapters === null || typeof adapters !== "object" || Array.isArray(adapters))) ||
        Object.keys(manifest).some(key => !["schemaVersion", "id", "match", "gamedataOwner", "bootstrapInputs", "gamedataRoot", "functionsFile", "trustedFunctionsFile", "adapters"].includes(key)) ||
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
    const adapterIds = Object.keys(adapters ?? {}).sort();
    if (adapterIds.length > MAX_ADAPTERS) throw new Error(`${root}: adapter count limit`);
    const adapterHashes = {};
    const adapterSources = [];
    for (const adapter of adapterIds) {
      const entry = adapters[adapter];
      if (!adapterId.test(adapter) || !entry || typeof entry !== "object" || Array.isArray(entry) ||
          entry.version !== 1 || Object.keys(entry).sort().join("\0") !== adapterKeys.slice().sort().join("\0") ||
          typeof entry.contract !== "string" || !entry.contract.endsWith(".json") || entry.contract.length > 256 ||
          typeof entry.source !== "string" || !entry.source.endsWith(".js") || entry.source.length > 256)
        throw new Error(`${root}: invalid adapter id, version, or source entry: ${adapter}`);
      const contractPath = input(entry.contract);
      adapterHashes[adapter] = parseContract(contractPath, readFileSync(contractPath), adapter);
      const sourcePath = input(entry.source);
      const sourceBytes = readFileSync(sourcePath);
      if (sourceBytes.length === 0 || sourceBytes.length > MAX_ADAPTER_SOURCE_BYTES)
        throw new Error(`${sourcePath}: adapter source size limit`);
      try { utf8.decode(sourceBytes); }
      catch { throw new Error(`${sourcePath}: invalid adapter source UTF-8`); }
      adapterSources.push(sourceBytes, Buffer.from("\n"));
    }
    // The final source expression supplies the package-local module export map.
    const sourcePrelude = adapterIds.length === 0 ? [] : [Buffer.from(
      `Object.defineProperty(globalThis,"__s2_adapter_contracts",{value:Object.freeze(${JSON.stringify(adapterHashes)}),writable:false,enumerable:false,configurable:false});\n`
    ), ...adapterSources];
    const bootstrap = Buffer.concat([...sourcePrelude, Buffer.from(js.toString().replace(/\n*$/, "") + "\n")]);
    if (adapterIds.length && bootstrap.length > 16 * 1024 * 1024)
      throw new Error(`${root}: adapter bootstrap size limit`);
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
    if (trustedFunctionsFile !== undefined) {
      const path = input(trustedFunctionsFile);
      const signatures = {};
      for (const file of files) Object.assign(signatures, file.document.signatures ?? {});
      const catalogPath = confined(root, `${gamedataRoot}/schema-catalog.json`);
      let catalog;
      try { catalog = JSON.parse(readFileSync(catalogPath, "utf8")); }
      catch (error) { throw new Error(`${catalogPath}: ${error.message}`); }
      const bytes = trustedArtifact({ path, text: readFileSync(path, "utf8"), id, adapterHashes, signatures, catalog });
      const trustedPath = `${prefix}/trusted-functions.json`;
      outputs.set(trustedPath, bytes);
      record.trustedFunctions = { path: trustedPath, sha256: sha256(bytes) };
    }
    packages.push(record);
  }
  const deployed = { schemaVersion: 2, packages };
  mkdirSync(outDir, { recursive: true });
  const stage = mkdtempSync(join(outDir, ".game-package-stage-"));
  try {
    for (const [name, bytes] of outputs) write(stage, name, bytes);
    write(stage, "game-packages.json", pretty(deployed));
    const obsolete = existingArtifacts(outDir).filter(name => !outputs.has(name));
    // A direct rebuild may inherit existing output directories. Refuse redirected parents
    // before invalidating the old manifest, including parents used only by stale-file removal.
    for (const name of [...outputs.keys(), ...obsolete]) checkOutputParent(outDir, name);
    // Staging is complete before publication. Once any artifact can change, the old manifest
    // must no longer describe a valid build. The new manifest is the final published file.
    rmSync(join(outDir, "game-packages.json"), { force: true });
    for (const name of outputs.keys()) {
      const dest = join(outDir, name);
      mkdirSync(dirname(dest), { recursive: true });
      checkOutputParent(outDir, name);
      renameSync(join(stage, name), dest);
    }
    for (const name of obsolete) {
      checkOutputParent(outDir, name);
      rmSync(join(outDir, name), { force: true });
    }
    renameSync(join(stage, "game-packages.json"), join(outDir, "game-packages.json"));
  } finally {
    rmSync(stage, { recursive: true, force: true });
  }
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
