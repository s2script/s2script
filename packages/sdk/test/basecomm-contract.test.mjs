/** Compile the real BaseComm declaration through the verified protocol-2 path. */
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
const producer = join(root, "plugins/basecomm/api.d.ts");
const observer = join(root, "examples/interop-observer");
const observerCopy = join(observer, ".s2script/types/@s2script/basecomm/index.d.ts");
const hash = path => createHash("sha256").update(readFileSync(path)).digest("hex");

function consumer(t, source) {
  const dir = mkdtempSync(join(tmpdir(), "s2-basecomm-contract-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, "src"));
  mkdirSync(join(dir, ".s2script/types/@s2script/basecomm"), { recursive: true });
  cpSync(observerCopy, join(dir, ".s2script/types/@s2script/basecomm/index.d.ts"));
  writeFileSync(join(dir, "package.json"), JSON.stringify({
    name: "@test/basecomm-consumer", version: "1.0.0", main: "src/plugin.ts",
    s2script: { interfaceProtocol: 2, optionalPluginDependencies: { "@s2script/basecomm": "^1.0.0" } },
  }));
  writeFileSync(join(dir, "src/plugin.ts"), source);
  return dir;
}

const consumeAll = `
import { use, watchOptional } from "@s2script/sdk/plugin";
import type { Subscription } from "@s2script/sdk/interfaces";
import { on, isMuted, isGagged, setMuted, setGagged } from "@s2script/basecomm";
import type { BaseComm, CommunicationStateEvent } from "@s2script/basecomm";
const service = use("@s2script/basecomm");
const methods: BaseComm = service;
const muted: boolean = isMuted("76561198000000001");
const gagged: boolean = isGagged("76561198000000001");
const muteAccepted: boolean = setMuted("76561198000000001", true);
const gagAccepted: boolean = setGagged("76561198000000001", false);
const sub: Subscription = on("OnClientMuteChanged", payload => {
  const event: CommunicationStateEvent = payload;
  console.log(event.steamId, event.state);
});
sub.dispose();
on("OnClientGagChanged", payload => console.log(payload.steamId, payload.state));
watchOptional("@s2script/basecomm", (basecomm, scope) => {
  scope.own(basecomm.on("OnClientMuteChanged", payload => console.log(payload.steamId, payload.state)));
  scope.own(basecomm.on("OnClientGagChanged", payload => console.log(payload.steamId, payload.state)));
  console.log(basecomm.isMuted("76561198000000001"), basecomm.isGagged("76561198000000001"));
});
console.log(methods, muted, gagged, muteAccepted, gagAccepted);
`;

test("basecomm contract compiles methods, notifications, inferred handles, and producer-as-import exports", t => {
  const result = typecheckPlugin(consumer(t, consumeAll), { packagesDir });
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
});

for (const [label, statement, match] of [
  ["unknown forward", 'service.on("OnClientMuted", () => {});', /OnClientMuted/],
  ["mute payload", 'service.on("OnClientMuteChanged", p => { const state: string = p.state; console.log(state); });', /boolean.*string/],
  ["setter state", 'service.setMuted("76561198000000001", "yes");', /string.*boolean/],
  ["caller selected contract", 'use<{wrong():string}>("@s2script/basecomm");', /generic/],
]) test(`basecomm rejects ${label}`, t => {
  const dir = consumer(t, 'import { use } from "@s2script/sdk/plugin"; const service = use("@s2script/basecomm"); ' + statement);
  const result = typecheckPlugin(dir, { packagesDir });
  assert.equal(result.ok, false);
  assert.ok(result.diagnostics.some(d => match.test(d.message)), JSON.stringify(result.diagnostics));
});

test("basecomm metadata fixes four methods and two state-change notifications", () => {
  const { metadata } = extractContract(producer, packagesDir);
  assert.deepEqual(Object.keys(metadata.methods).sort(), ["isGagged", "isMuted", "setGagged", "setMuted"]);
  assert.deepEqual(Object.keys(metadata.forwards).sort(), ["OnClientGagChanged", "OnClientMuteChanged"]);
  for (const descriptor of Object.values(metadata.forwards)) {
    assert.equal(descriptor.kind, "notification");
    assert.deepEqual(Object.keys(descriptor.payload.fields).sort(), ["state", "steamId"]);
  }
});

test("observer contract is an exact byte-copy of the major-versioned provider", () => {
  assert.deepEqual(readFileSync(producer), readFileSync(observerCopy));
  const basecomm = JSON.parse(readFileSync(join(root, "plugins/basecomm/package.json")));
  const consumerPackage = JSON.parse(readFileSync(join(observer, "package.json")));
  assert.equal(basecomm.version, "1.0.0");
  assert.equal(basecomm.s2script.interfaceProtocol, 2);
  assert.equal(consumerPackage.s2script.optionalPluginDependencies["@s2script/basecomm"], "^1.0.0");
  assert.equal(consumerPackage.s2script.interfaceProtocol, 2);
});

test("standalone observer builds from copied types without provider source or archive", async t => {
  const standalone = mkdtempSync(join(tmpdir(), "s2-basecomm-observer-"));
  const producerDir = mkdtempSync(join(tmpdir(), "s2-basecomm-producer-"));
  t.after(() => {
    rmSync(standalone, { recursive: true, force: true });
    rmSync(producerDir, { recursive: true, force: true });
  });
  for (const file of ["src", "package.json", ".s2script"]) {
    cpSync(join(observer, file), join(standalone, file), { recursive: true });
  }
  for (const file of ["src", "api.d.ts", "package.json"]) {
    cpSync(join(root, "plugins/basecomm", file), join(producerDir, file), { recursive: true });
  }
  assert.equal(readFileSync(join(standalone, ".s2script/types/@s2script/basecomm/index.d.ts"), "utf8").includes("Contract"), true);
  const producerManifest = JSON.parse(openZip(await buildPlugin(producerDir, packagesDir)).readAsText("manifest.json"));
  const observerManifest = JSON.parse(openZip(await buildPlugin(standalone, packagesDir)).readAsText("manifest.json"));
  const published = producerManifest.publishes["@s2script/basecomm"];
  assert.equal(published.typesSha256, hash(producer));
  assert.equal(observerManifest.compiledAgainst["@s2script/basecomm"], published.typesSha256);
  assert.deepEqual(observerManifest.interfaceContracts["@s2script/basecomm"], published.contract);
  assert.equal(published.version, "1.0.0");
});
