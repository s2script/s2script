/** Compile the real BaseBans declaration through the verified protocol-2 path. */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { typecheckPlugin } from "../src/typecheck/typecheck.ts";
import { extractContract } from "../src/interop.ts";
import { buildPlugin } from "../src/build.ts";
import { openZip } from "./zip.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const packagesDir = join(root, "packages");
const producer = join(root, "plugins/basebans/api.d.ts");
const observer = join(root, "examples/interop-observer");
const observerCopy = join(observer, ".s2script/types/@s2script/basebans/index.d.ts");
const hash = path => createHash("sha256").update(readFileSync(path)).digest("hex");

function consumer(t, source) {
  const dir = mkdtempSync(join(tmpdir(), "s2-basebans-contract-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, "src"));
  mkdirSync(join(dir, ".s2script/types/@s2script/basebans"), { recursive: true });
  cpSync(observerCopy, join(dir, ".s2script/types/@s2script/basebans/index.d.ts"));
  writeFileSync(join(dir, "package.json"), JSON.stringify({
    name: "@test/basebans-consumer", version: "1.0.0", main: "src/plugin.ts",
    s2script: { interfaceProtocol: 2, optionalPluginDependencies: { "@s2script/basebans": "^1.0.0" } },
  }));
  writeFileSync(join(dir, "src/plugin.ts"), source);
  return dir;
}

const consumeAll = `
import { use, watchOptional } from "@s2script/sdk/plugin";
import { HookResult } from "@s2script/sdk/events";
import type { HookResultValue } from "@s2script/sdk/events";
import { ban, unban, on } from "@s2script/basebans";
import type { BanRequest, BanResult, BanRecord, UnbanRequest } from "@s2script/basebans";
const request: BanRequest = { steamId:"76561198000000001", minutes:0, reason:"", source:"plugin", actorSteamId:null };
const result: BanResult = ban(request);
const decision: HookResultValue = result.result;
const removal: UnbanRequest = {steamId: request.steamId};
const removed: boolean = unban(removal);
const service = use("@s2script/basebans");
service.on("OnBanRequested", p => p.source === "plugin" ? HookResult.Handled : HookResult.Continue);
on("OnBanRecorded", p => { const record: BanRecord = p; console.log(record.request.reason, record.until); });
watchOptional("@s2script/basebans", (basebans, scope) => {
  scope.own(basebans.on("OnBanRemoved", p => console.log(p.steamId)));
});
console.log(decision,removed);
`;

test("basebans contract compiles typed methods, Hook results, notifications and inferred handles", t => {
  const result = typecheckPlugin(consumer(t, consumeAll), { packagesDir });
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
});

for (const [label, statement, match] of [
  ["unknown forward", 'service.on("OnBanned", () => {});', /OnBanned/],
  ["wrong record payload", 'service.on("OnBanRecorded", p => { const reason: number = p.request.reason; console.log(reason); });', /string.*number/],
  ["invalid source", 'service.ban({steamId:"1", minutes:0, reason:"", source:"console", actorSteamId:null});', /console/],
  ["malformed request", 'service.ban({steamId:"1", minutes:0, reason:"", source:"plugin"});', /actorSteamId/],
  ["malformed result", 'const result: {recorded:string} = service.ban({steamId:"1",minutes:0,reason:"",source:"plugin",actorSteamId:null});', /boolean.*string/],
  ["transform patch on Hook", 'service.on("OnBanRequested", () => ({result:1,patch:{reason:"other"}}));', /HookResult|assignable/],
  ["caller selected contract", 'use<{wrong():string}>("@s2script/basebans");', /generic/],
]) test(`basebans rejects ${label}`, t => {
  const dir = consumer(t, 'import { use } from "@s2script/sdk/plugin"; const service = use("@s2script/basebans"); ' + statement);
  const result = typecheckPlugin(dir, { packagesDir });
  assert.equal(result.ok, false);
  assert.ok(result.diagnostics.some(d => match.test(d.message)), JSON.stringify(result.diagnostics));
});

test("basebans metadata fixes shared operations, request hook and cache notifications", () => {
  const { metadata } = extractContract(producer, packagesDir);
  assert.deepEqual(Object.keys(metadata.methods).sort(), ["ban", "unban"]);
  assert.deepEqual(Object.keys(metadata.forwards).sort(), ["OnBanRecorded", "OnBanRemoved", "OnBanRequested"]);
  assert.equal(metadata.forwards.OnBanRequested.kind, "hook");
  assert.equal(metadata.forwards.OnBanRecorded.kind, "notification");
  assert.equal(metadata.forwards.OnBanRemoved.kind, "notification");
});

for (const module of ["@s2script/sdk/events", "@s2script/sdk"]) test(`existing HookResultValue contract import from ${module} is supported`, t => {
  const dir = mkdtempSync(join(tmpdir(), "s2-hookresult-import-"));
  t.after(() => rmSync(dir, {recursive:true,force:true}));
  const entry = join(dir,"api.d.ts");
  writeFileSync(entry, `import type {HookResultValue as Decision} from "${module}";
    export interface Contract {methods:{decision():Decision};forwards:{}}`);
  const {metadata} = extractContract(entry, packagesDir);
  assert.deepEqual(metadata.methods.decision.result, {kind:"union",variants:[0,1,2,3].map(value => ({kind:"literal",value}))});
});

for (const [module, symbol] of [["@s2script/sdk/events","GameEvent"], ["@s2script/sdk","BanInfo"], ["./foreign","HookResultValue"]]) test(`unsupported domain import ${module}/${symbol} stays rejected`, t => {
  const dir = mkdtempSync(join(tmpdir(), "s2-ban-import-"));
  t.after(() => rmSync(dir, {recursive:true,force:true}));
  writeFileSync(join(dir,"foreign.d.ts"), "export type HookResultValue = string;");
  const entry = join(dir,"api.d.ts");
  writeFileSync(entry, `import type {${symbol}} from "${module}";
    export interface Contract {methods:{decision():${symbol}};forwards:{}}`);
  assert.throws(() => extractContract(entry, packagesDir), /self-contained/);
});

test("observer contract is an exact byte-copy of the major-versioned provider", () => {
  assert.deepEqual(readFileSync(producer), readFileSync(observerCopy));
  const basebans = JSON.parse(readFileSync(join(root, "plugins/basebans/package.json")));
  const consumerPackage = JSON.parse(readFileSync(join(observer, "package.json")));
  assert.equal(basebans.version, "1.0.0");
  assert.equal(basebans.s2script.interfaceProtocol, 2);
  assert.equal(consumerPackage.s2script.optionalPluginDependencies["@s2script/basebans"], "^1.0.0");
  assert.equal(consumerPackage.s2script.interfaceProtocol, 2);
});

test("standalone observer builds from copied types without provider source or archive", async t => {
  const standalone = mkdtempSync(join(tmpdir(), "s2-basebans-observer-"));
  const producerDir = mkdtempSync(join(tmpdir(), "s2-basebans-producer-"));
  t.after(() => {
    rmSync(standalone, { recursive: true, force: true });
    rmSync(producerDir, { recursive: true, force: true });
  });
  for (const file of ["src", "package.json", ".s2script"]) {
    cpSync(join(observer, file), join(standalone, file), { recursive: true });
  }
  for (const file of ["src", "api.d.ts", "package.json"]) {
    cpSync(join(root, "plugins/basebans", file), join(producerDir, file), { recursive: true });
  }
  assert.equal(readFileSync(join(standalone, ".s2script/types/@s2script/basebans/index.d.ts"), "utf8").includes("Contract"), true);
  const producerManifest = JSON.parse(openZip(await buildPlugin(producerDir, packagesDir)).readAsText("manifest.json"));
  const observerManifest = JSON.parse(openZip(await buildPlugin(standalone, packagesDir)).readAsText("manifest.json"));
  const published = producerManifest.publishes["@s2script/basebans"];
  assert.equal(published.typesSha256, hash(producer));
  assert.equal(observerManifest.compiledAgainst["@s2script/basebans"], published.typesSha256);
  assert.deepEqual(observerManifest.interfaceContracts["@s2script/basebans"], published.contract);
  assert.equal(published.version, "1.0.0");
});
