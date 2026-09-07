/** Compile the real declaration through the same verified path as s2s add/build. */
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
const producer = join(root, "plugins/zones/api.d.ts");
const cookbookCopy = join(root, "examples/cookbook/.s2script/types/@s2script/zones/index.d.ts");
const hash = path => createHash("sha256").update(readFileSync(path)).digest("hex");
function consumer(t, source) {
  const dir = mkdtempSync(join(tmpdir(), "s2-zones-contract-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  mkdirSync(join(dir, "src"));
  mkdirSync(join(dir, ".s2script/types/@s2script/zones"), { recursive: true });
  cpSync(cookbookCopy, join(dir, ".s2script/types/@s2script/zones/index.d.ts"));
  writeFileSync(join(dir, "package.json"), JSON.stringify({
    name: "@test/zones-consumer", version: "1.0.0", main: "src/plugin.ts",
    s2script: { interfaceProtocol: 2, optionalPluginDependencies: { "@s2script/zones": "^1.0.0" } },
  }));
  writeFileSync(join(dir, "src/plugin.ts"), source);
  return dir;
}
const consumeAll = `
import { use, watchOptional } from "@s2script/sdk/plugin";
import type { Subscription } from "@s2script/sdk/interfaces";
import { on, createZone, deleteZone, getZones, isInZone, zonesFor, getZonesByTag, setZoneTags } from "@s2script/zones";
import type { Zone, ZoneEvent, ZoneCreatedEvent, ZoneDeletedEvent, Zones } from "@s2script/zones";
const service = use("@s2script/zones");
const methods: Zones = service;
const list: Zone[] = getZones();
const tagged: Zone[] = getZonesByTag("heal");
const made: boolean = createZone("heal", {x:0,y:0,z:0}, {x:1,y:1,z:1});
const removed: boolean = deleteZone("heal");
const inside: boolean = isInZone(0, "heal");
const names: string[] = zonesFor(0);
const updated: boolean = setZoneTags("heal", ["heal"]);
for (const event of ["enter", "leave", "stay"] as const) {
  const sub: Subscription = on(event, payload => {
    const p: ZoneEvent = payload;
    const zone: string = p.zone; const slot: number = p.slot; const id: number = p.userId;
    console.log(zone, slot, id);
  });
  sub.dispose();
}
on("created", payload => { const p: ZoneCreatedEvent = payload; console.log(p.zone, p.min.x, p.max.z, p.tags.join(",")); });
on("deleted", payload => { const p: ZoneDeletedEvent = payload; console.log(p.zone); });
watchOptional("@s2script/zones", (zones, scope) => {
  scope.own(zones.on("created", p => console.log(p.min.y, p.tags)));
  scope.own(zones.on("deleted", p => console.log(p.zone)));
  scope.own(zones.on("enter", p => console.log(p.userId)));
  scope.own(zones.on("leave", p => console.log(p.userId)));
  scope.own(zones.on("stay", p => console.log(p.userId)));
  console.log(zones.getZones());
});
console.log(methods, list, tagged, made, removed, inside, names, updated);
`;

test("zones contract compiles every payload, inferred handle, and producer-as-import export", t => {
  const result = typecheckPlugin(consumer(t, consumeAll), { packagesDir });
  assert.equal(result.ok, true, JSON.stringify(result.diagnostics));
});

for (const [label, statement, match] of [
  ["unknown forward", 'service.on("enterr", () => {});', /enterr/],
  ["enter identity type", 'service.on("enter", p => { const id: string = p.userId; console.log(id); });', /number.*string/],
  ["created bounds", 'service.on("created", p => console.log(p.userId));', /userId/],
  ["deleted is name only", 'service.on("deleted", p => console.log(p.min));', /min/],
  ["direct subscription payload", 'on("stay", p => { const id: string = p.userId; console.log(id); });', /number.*string/],
  ["caller selected contract", 'use<{wrong():string}>("@s2script/zones");', /generic/],
]) test(`zones rejects ${label}`, t => {
  const dir = consumer(t, 'import { use } from "@s2script/sdk/plugin"; import { on } from "@s2script/zones"; const service = use("@s2script/zones"); ' + statement);
  const result = typecheckPlugin(dir, { packagesDir });
  assert.equal(result.ok, false);
  assert.ok(result.diagnostics.some(d => match.test(d.message)), JSON.stringify(result.diagnostics));
});

test("zones metadata preserves the seven methods and five notification meanings", () => {
  const { metadata } = extractContract(producer, packagesDir);
  assert.deepEqual(Object.keys(metadata.methods).sort(), ["createZone", "deleteZone", "getZones", "getZonesByTag", "isInZone", "setZoneTags", "zonesFor"]);
  assert.deepEqual(Object.keys(metadata.forwards).sort(), ["created", "deleted", "enter", "leave", "stay"]);
  for (const [name, descriptor] of Object.entries(metadata.forwards)) {
    assert.equal(descriptor.kind, "notification");
    assert.deepEqual(Object.keys(descriptor.payload.fields).sort(), name === "created" ? ["max", "min", "tags", "zone"] : name === "deleted" ? ["zone"] : ["slot", "userId", "zone"]);
  }
});

test("cookbook zones contract is an exact byte-copy of the major-versioned producer", () => {
  assert.deepEqual(readFileSync(producer), readFileSync(cookbookCopy));
  const zones = JSON.parse(readFileSync(join(root, "plugins/zones/package.json")));
  const cookbook = JSON.parse(readFileSync(join(root, "examples/cookbook/package.json")));
  assert.equal(zones.version, "1.0.0");
  assert.equal(zones.s2script.interfaceProtocol, 2);
  assert.equal(cookbook.s2script.optionalPluginDependencies["@s2script/zones"], "^1.0.0");
  assert.equal(cookbook.s2script.interfaceProtocol, 2);
});

test("zones build verifies actual producer and downloaded consumer against identical bytes and metadata", async t => {
  const dir = mkdtempSync(join(tmpdir(), "s2-zones-producer-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const file of ["src", "api.d.ts", "package.json"]) cpSync(join(root, "plugins/zones", file), join(dir, file), { recursive: true });
  const producerManifest = JSON.parse(openZip(await buildPlugin(dir, packagesDir)).readAsText("manifest.json"));
  const consumerManifest = JSON.parse(openZip(await buildPlugin(consumer(t, consumeAll), packagesDir)).readAsText("manifest.json"));
  const published = producerManifest.publishes["@s2script/zones"];
  assert.equal(published.typesSha256, hash(producer));
  assert.equal(consumerManifest.compiledAgainst["@s2script/zones"], published.typesSha256);
  assert.deepEqual(consumerManifest.interfaceContracts["@s2script/zones"], published.contract);
  assert.equal(published.version, "1.0.0");
});
