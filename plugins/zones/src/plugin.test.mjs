import test from "node:test";
import assert from "node:assert/strict";
import vm from "node:vm";
import { readFileSync } from "node:fs";
import { transformSync } from "esbuild";

const code = file => transformSync(readFileSync(new URL(file, import.meta.url), "utf8"), { loader: "ts", format: "cjs" }).code;
const providerCode = code("./plugin.ts");
const recipeCode = code("../../../examples/cookbook/src/recipes/zones.ts");
const settle = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const min = { x: 0, y: 0, z: 0 }, max = { x: 100, y: 100, z: 100 };
const row = name => ({ name, minX: 0, minY: 0, minZ: 0, maxX: 100, maxY: 100, maxZ: 100, tags: "heal" });
function run(source, sdk, cs2, logs) {
  const context = vm.createContext({ module: { exports: {} }, console: { log: (...args) => logs.push(args.join(" ")) },
    require: name => {
      if (name === "@s2script/sdk") return sdk;
      if (name === "@s2script/cs2") return cs2;
      throw new Error(`unexpected import ${name}`);
    },
  });
  vm.runInContext(source, context);
  return context.module.exports;
}

// Only the engine/DB/interop boundary is substituted. These tests execute the actual
// provider operations and cookbook callbacks; native attachment and copy enforcement
// have their own in-isolate suite. Event arrays are snapshots, as on the real wire.
async function fixture(rows = []) {
  const events = [], logs = [], players = new Map(), outputs = new Map(), commands = new Map(), triggers = [], polls = new Set();
  const listeners = new Map(), writes = [], files = new Map();
  let service, nextRows = rows, nextTrigger = 400;
  const cs2 = {
    Player: {
      all: () => [...players.values()],
      fromSlot: slot => players.get(slot) ?? null,
      fromUserId: id => [...players.values()].find(p => p.userId === id) ?? null,
    },
    Pawn: { forSlot: slot => players.get(slot)?.pawn ?? null },
    TriggerZone: { create: () => { const t = { ref: { index: nextTrigger++ }, removed: false, remove() { this.removed = true; } }; triggers.push(t); return t; } },
    Beam: { draw: () => ({ remove() {}, update() {} }) },
  };
  const command = { admin: (name, _flags, handler) => commands.set(name, handler) };
  const sdk = {
    command, onOutput: (_class, name, handler) => outputs.set(name, handler), translations: { load() {} },
    publish: (name, implementation) => {
      assert.equal(name, "@s2script/zones");
      service = implementation;
      return { emit: (event, payload) => {
        events.push([event, structuredClone(payload)]);
        for (const listener of listeners.get(event) ?? []) listener(structuredClone(payload));
      } };
    },
    createScope: () => ({ server: { onGameFrame: f => polls.add(f) }, clear: () => polls.clear() }),
    ADMFLAG: { GENERIC: 1 }, HookResult: { Handled: 2 },
    Database: { open: async () => ({ query: async () => nextRows, execute: async (sql, args) => { writes.push([sql, structuredClone(args)]); } }) },
    Server: { mapName: "de_first" },
    config: { readFile: name => files.get(name), writeFile: (name, text) => files.set(name, text) },
    Vector: class { constructor(x, y, z) { Object.assign(this, { x, y, z }); } },
    Chat: { toSlot() {} }, Translations: { translate: (_slot, key) => key },
  };
  const api = run(providerCode, sdk, cs2, logs);
  await api.OnPluginStart();
  const call = (name, args, slot = -1) => commands.get(name)({ args, callerSlot: slot, replyT() {}, argFloat: (i, fallback) => Number(args[i] ?? fallback) });
  const touch = (name, player, trigger = triggers.at(-1)) => outputs.get(name)({ caller: trigger.ref, activator: player.pawn.ref });
  const addPlayer = (slot = 3, userId = 41) => {
    const p = { slot, userId, playerName: `player-${userId}`, pawn: { ref: { index: 100 + slot }, health: 50, origin: { ...min }, buttons: 0 } };
    players.set(slot, p); return p;
  };
  return { api, service, events, logs, players, cs2, listeners, writes, files, triggers, polls, call, touch, addPlayer,
    map: async rows => { nextRows = rows; api.OnMapStart("de_next"); await settle(); },
  };
}

test("CRUD emits existing created/deleted payloads and getters see the updated current state", async () => {
  const f = await fixture([row("existing")]);
  assert.deepEqual(f.events, [], "initial load precedes publication and is not replayed");
  assert.deepEqual(structuredClone(f.service.getZones()), [{ name: "existing", min, max, tags: ["heal"] }]);
  const observed = [];
  f.listeners.set("created", [p => observed.push(f.service.getZones().some(z => z.name === p.zone))]);
  f.listeners.set("deleted", [p => observed.push(!f.service.getZones().some(z => z.name === p.zone))]);
  assert.equal(f.service.createZone("new", max, min), true);
  assert.deepEqual(f.events.at(-1), ["created", { zone: "new", min, max, tags: [] }]);
  assert.equal(f.service.setZoneTags("new", ["HeAL", "a b"]), true);
  assert.equal(f.service.createZone("new", min, max), true);
  assert.deepEqual(f.events.at(-1), ["created", { zone: "new", min, max, tags: ["heal", "ab"] }]);
  assert.deepEqual(structuredClone(f.service.getZonesByTag("HEAL")).map(z => z.name), ["existing", "new"]);
  assert.equal(f.service.deleteZone("new"), true);
  assert.deepEqual(f.events.at(-1), ["deleted", { zone: "new" }]);
  const count = f.events.length;
  assert.equal(f.service.deleteZone("missing"), false);
  assert.equal(f.service.createZone("flat", min, min), false);
  assert.equal(f.events.length, count);
  assert.deepEqual(observed, [true, true, true]);
});

test("operator add/import/editor save/delete emit through the real operation paths", async () => {
  const f = await fixture();
  f.call("sm_zone_add", ["console", "0", "0", "0", "100", "100", "100"]);
  await settle();
  assert.deepEqual(f.events.at(-1), ["created", { zone: "console", min, max, tags: [] }]);
  f.files.set("zones-de_first.json", JSON.stringify({ imported: { min: [0, 0, 0], max: [100, 100, 100], tags: ["HEAL"] } }));
  f.call("sm_zone_import", []); await settle();
  assert.deepEqual(f.events.at(-1), ["created", { zone: "imported", min, max, tags: ["heal"] }]);
  const p = f.addPlayer();
  f.call("sm_zone_edit", ["edited"], p.slot);
  p.pawn.buttons = 32; for (const poll of f.polls) poll();
  p.pawn.buttons = 0; for (const poll of f.polls) poll();
  p.pawn.origin = { ...max }; p.pawn.buttons = 32; for (const poll of f.polls) poll();
  await settle();
  assert.deepEqual(f.events.at(-1), ["created", { zone: "edited", min, max, tags: [] }]);
  f.call("sm_zone_delete", ["edited"]);
  assert.deepEqual(f.events.at(-1), ["deleted", { zone: "edited" }]);
});

test("engine boundaries and eight-frame stay notifications preserve connection identity", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  const p = f.addPlayer();
  f.touch("OnStartTouch", p); f.touch("OnStartTouch", p);
  assert.deepEqual(f.events, [["enter", { zone: "heal", slot: 3, userId: 41 }]]);
  assert.equal(f.service.isInZone(3, "heal"), true);
  assert.deepEqual(structuredClone(f.service.zonesFor(3)), ["heal"]);
  for (let i = 0; i < 8; i++) f.api.OnGameFrame();
  assert.deepEqual(f.events.at(-1), ["stay", { zone: "heal", slot: 3, userId: 41 }]);
  f.touch("OnEndTouch", p); f.touch("OnEndTouch", p);
  assert.deepEqual(f.events.at(-1), ["leave", { zone: "heal", slot: 3, userId: 41 }]);
  assert.equal(f.events.filter(([e]) => e === "leave").length, 1);
});

test("disconnect clears occupancy without inventing leave events or healing a replacement", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  const p = f.addPlayer(); f.touch("OnStartTouch", p);
  f.players.delete(p.slot);
  // OnClientDisconnect retains a read-only identity snapshot during this callback.
  f.api.OnClientDisconnect({ slot: p.slot, userId: p.userId, isValid: () => false });
  const replacement = f.addPlayer(3, 42);
  assert.equal(f.service.isInZone(3, "heal"), false);
  assert.deepEqual(structuredClone(f.service.zonesFor(3)), []);
  for (let i = 0; i < 8; i++) f.api.OnGameFrame();
  assert.deepEqual(f.events, [["enter", { zone: "heal", slot: 3, userId: 41 }]]);
  f.touch("OnStartTouch", replacement);
  assert.deepEqual(f.events.at(-1), ["enter", { zone: "heal", slot: 3, userId: 42 }]);
});

test("queries and stay cannot reinterpret a stale inside slot as a new user", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  const p = f.addPlayer(); f.touch("OnStartTouch", p);
  f.addPlayer(3, 42); // Identity safety also holds if a touch-end/disconnect was missed.
  assert.equal(f.service.isInZone(3, "heal"), false);
  assert.deepEqual(structuredClone(f.service.zonesFor(3)), []);
  for (let i = 0; i < 8; i++) f.api.OnGameFrame();
  assert.deepEqual(f.events, [["enter", { zone: "heal", slot: 3, userId: 41 }]]);
});

test("old disconnect cleanup cannot remove an already-entered replacement connection", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  const old = f.addPlayer(); f.touch("OnStartTouch", old);
  const replacement = f.addPlayer(3, 42); f.touch("OnStartTouch", replacement);
  f.api.OnClientDisconnect({ slot: 3, userId: old.userId, isValid: () => false });
  assert.equal(f.service.isInZone(3, "heal"), true);
  for (let i = 0; i < 8; i++) f.api.OnGameFrame();
  assert.deepEqual(f.events.at(-1), ["stay", { zone: "heal", slot: 3, userId: 42 }]);
});

test("unassigned userId minus one cannot become zone occupancy", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  f.touch("OnStartTouch", f.addPlayer(0, -1));
  assert.equal(f.service.isInZone(0, "heal"), false);
  assert.deepEqual(f.events, []);
});

test("map change deletes old zones, clears identity, and creates the new layout without replay", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  const oldTrigger = f.triggers[0], p = f.addPlayer(); f.touch("OnStartTouch", p);
  await f.map([row("new-map")]);
  assert.equal(oldTrigger.removed, true);
  assert.deepEqual(f.events.slice(1), [["deleted", { zone: "heal" }], ["created", { zone: "new-map", min, max, tags: ["heal"] }]]);
  assert.deepEqual(structuredClone(f.service.zonesFor(3)), []);
  f.api.OnGameFrame();
  f.touch("OnStartTouch", p, oldTrigger);
  assert.equal(f.events.length, 3, "old map's trigger cannot emit into the new layout");
  f.api.OnPluginEnd();
  assert.ok(f.triggers.every(t => t.removed));
});

test("map deletion observers query the cleared layout and delayed DB loads cannot cross map generations", async () => {
  const f = await fixture([row("old")]);
  const deletedSnapshots = [];
  f.listeners.set("deleted", [() => deletedSnapshots.push(structuredClone(f.service.getZones()))]);
  let resolveOldMap;
  const oldMap = new Promise(resolve => { resolveOldMap = resolve; });
  await f.map(oldMap);
  assert.deepEqual(deletedSnapshots, [[]]);
  await f.map([row("current")]);
  resolveOldMap([row("stale")]); await settle();
  assert.deepEqual(structuredClone(f.service.getZones()).map(z => z.name), ["current"]);
  assert.deepEqual(f.events, [["deleted", { zone: "old" }], ["created", { zone: "current", min, max, tags: ["heal"] }]]);
});

test("slot zero and userId zero are valid identities", async () => {
  const f = await fixture([row("heal")]); f.api.OnGameFrame();
  const p = f.addPlayer(0, 0); f.touch("OnStartTouch", p);
  assert.equal(f.service.isInZone(0, "heal"), true);
  for (let i = 0; i < 8; i++) f.api.OnGameFrame();
  assert.deepEqual(f.events.at(-1), ["stay", { zone: "heal", slot: 0, userId: 0 }]);
  await f.map([row("heal")]); f.api.OnGameFrame();
  assert.equal(f.service.isInZone(0, "heal"), false, "even a reused map userId needs a new touch");
});

test("cookbook attaches late, queries each current layout, and responds by userId", async () => {
  const f = await fixture([row("heal")]);
  const watches = [], attachments = [];
  const recipe = run(recipeCode, {
    watchOptional: (name, attach) => { assert.equal(name, "@s2script/zones"); watches.push(attach); },
    tryUse: () => null, // A one-shot lookup must fail this late-provider regression.
  }, f.cs2, f.logs);
  recipe.OnPluginStart();
  assert.equal(watches.length, 1);
  const attach = () => {
    const owned = [];
    const service = { ...f.service, on: (name, handler) => {
      const handlers = f.listeners.get(name) ?? new Set(); f.listeners.set(name, handlers); handlers.add(handler);
      return { dispose: () => handlers.delete(handler) };
    } };
    watches[0](service, { own: resource => { owned.push(resource); return resource; } });
    attachments.push(() => { for (const resource of owned) resource.dispose(); });
  };
  attach();
  assert.ok(f.logs.some(s => /EXISTING heal/.test(s)), f.logs.join("\n"));
  assert.equal(f.events.length, 0, "getZones is the initial snapshot, never created replay");
  const old = f.addPlayer(3, 41); f.addPlayer(3, 42);
  const handlers = f.listeners.get("stay");
  for (const handler of handlers) handler({ zone: "heal", slot: 3, userId: old.userId });
  assert.equal(f.players.get(3).pawn.health, 50, "stale event cannot heal a replacement in its old slot");
  for (const handler of handlers) handler({ zone: "heal", slot: 99, userId: 42 });
  assert.equal(f.players.get(3).pawn.health, 51, "the copied userId selects the live player");
  attachments[0]();
  await f.map([row("replacement-layout")]);
  const before = f.logs.length; attach();
  assert.ok(f.logs.slice(before).some(s => /EXISTING replacement-layout/.test(s)));
  assert.equal(f.listeners.get("stay").size, 1, "a new attachment has one set of recipe handlers");
});
