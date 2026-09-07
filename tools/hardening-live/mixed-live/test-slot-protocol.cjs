// Execute the actual bundled fixture against a deterministic slot-generation model.
const assert = require("node:assert/strict");
const path = require("node:path");
const vm = require("node:vm");
const { buildSync } = require("esbuild");
const code = buildSync({
  entryPoints: [path.join(__dirname, "src/plugin.ts")], bundle: true, write: false,
  platform: "node", format: "cjs", external: ["@s2script/sdk"],
}).outputFiles[0].text;
let now = 0, nextSlot = 1, nextUser = 1;
const timers = new Set(), clients = new Map(), commands = new Map(), probes = [], serverCommands = [];
const logs = [];
let plugin;
const longStats = JSON.stringify({
  cache: { accounts: 0, bytes: 0, entries: 0 },
  padding: "x".repeat(5000),
});
function clientAt(slot) {
  const id = nextUser++;
  let live = true, muted = false;
  const stale = (action) => { if (!live) probes.push({ slot, id, action }); };
  const client = {
    slot, isValid: () => live,
    get userId() { return live ? id : -1; },
    get name() { return live ? `bot-${id}` : ""; },
    get steamId() { return "0"; },
    get signonState() { return live ? 5 : -1; },
    get isBot() { return live; }, get ip() { return ""; },
    get voiceMuted() { return live && muted; },
    set voiceMuted(value) { stale("voice"); if (live) muted = value; },
    fakeCommand() { stale("command"); return live; },
    chat() { stale("chat"); }, print() { stale("print"); },
    kick() {
      stale("kick");
      if (live) { live = false; clients.delete(slot); plugin.OnClientDisconnect?.(client); }
    },
  };
  clients.set(slot, client);
  return client;
}
const sdk = {
  after(ms, callback) {
    const timer = { at: now + ms, callback, kill() { return timers.delete(timer); } };
    timers.add(timer); return timer;
  },
  Clients: { all: () => [...clients.values()], fromSlot: (slot) => clients.get(slot) ?? null },
  command: { server: (name, callback) => commands.set(name, callback) },
  config: { getInt: () => 0, onChange() {} }, HookResult: { Handled: 1 },
  Server: { command(text) { serverCommands.push(text); } },
};
const context = { module: { exports: {} }, __s2_async_stats: () => longStats, require: (name) => {
  assert.equal(name, "@s2script/sdk"); return sdk;
}, console: { log: (text) => logs.push(text), error: (text) => logs.push(text) } };
vm.runInNewContext(code, context);
plugin = context.module.exports;
plugin.OnPluginStart();
function command(name) {
  const replies = [];
  commands.get(name)({ argInt: () => 1, reply: (text) => replies.push(text.slice(0, 2048)) });
  return replies.join("\n");
}
function advance(ms) {
  const end = now + ms;
  while (true) {
    const next = [...timers].filter((timer) => timer.at <= end).sort((a, b) => a.at - b.at)[0];
    if (!next) break;
    timers.delete(next); now = next.at; next.callback();
  }
  now = end;
}
function field(name) {
  return Number(command("sm_mixed_status").match(new RegExp(`${name}=(\\d+)`))[1]);
}
command("sm_mixed_slotreset");
const statsLines = command("sm_mixed_stats").split("\n");
assert.ok(statsLines.length > 1, "long stats must use multiple bounded command replies");
assert.ok(statsLines.every((line) => line.length < 2048), "each stats reply must survive the engine line cap");
const statsParts = statsLines.map((line) => {
  const match = line.match(/^\[mixed-live\] STATS_PART snapshot=(\d+) part=(\d+)\/(\d+) bytes=(\d+) data=(.*)$/);
  assert.ok(match, `malformed stats part: ${line.slice(0, 120)}`);
  return { snapshot: match[1], part: Number(match[2]), count: Number(match[3]), bytes: Number(match[4]), data: match[5] };
});
assert.equal(new Set(statsParts.map((part) => part.snapshot)).size, 1);
assert.equal(statsParts.length, statsParts[0].count);
assert.deepEqual(statsParts.map((part) => part.part), Array.from({ length: statsParts.length }, (_, i) => i + 1));
assert.equal(statsParts.map((part) => part.data).join("").length, statsParts[0].bytes);
assert.deepEqual(JSON.parse(statsParts.map((part) => part.data).join("")), JSON.parse(longStats));
clientAt(0);
for (let cycle = 0; cycle < 16; cycle++) {
  nextSlot = (cycle + 1) % 16;
  assert.match(command("sm_mixed_slotcycle"), /SLOT armed/);
  assert.equal(timers.size, 0, "quota-managed replacement must not race a delayed bot_add timer");
  assert.deepEqual(serverCommands, [], "the fixture must leave replacement scheduling to bot_quota");
  advance(60_000); // Real collector cadence; expiry must not erase reuse evidence.
  assert.equal(timers.size, 0);
  plugin.OnClientActive(clientAt(nextSlot));
  if (cycle < 15) {
    assert.equal(field("slotPending"), cycle + 1);
    assert.equal(field("slotReuses"), 0);
    assert.equal(probes.length, 0, "different-slot replacement is not a stale probe");
  } else {
    assert.equal(timers.size, 1, "same-slot candidate gets one bounded stability timer");
    const provisional = clients.get(nextSlot);
    provisional.kick();
    plugin.OnClientActive(clientAt(nextSlot));
    assert.equal(probes.length, 0, "no stale action may run against a provisional quota candidate");
    advance(250);
    assert.equal(timers.size, 1, "a replacement arriving during settling is observed in turn");
    assert.equal(field("slotFailures"), 0, "provisional quota churn is not a stale-fence failure");
    assert.equal(field("slotPending"), 16, "provisional churn retains the old handles for real proof");
    advance(250);
    assert.equal(timers.size, 1, "stable reuse gets one bounded post-action verification timer");
    advance(100);
  }
}
assert.equal(field("slotReuses"), 1);
assert.equal(field("slotFailures"), 0);
assert.equal(field("slotPending"), 0, "successful proof releases all old handles");
assert.deepEqual(probes.map((probe) => probe.action), ["command", "voice", "chat", "print", "kick"]);
assert.ok(probes.every((probe) => probe.slot === 0 && probe.id === 1));
assert.ok(clients.get(0).isValid());
assert.equal(clients.get(0).voiceMuted, false);
assert.ok(logs.some((line) => /SLOT_REUSE slot=0 .*pass=true/.test(line)));
assert.ok(logs.some((line) =>
  /SLOT_CHECK slot=0 oldValid=false oldUserId=-1 oldName="" oldSteamId=0 oldSignon=-1 oldBot=false oldIp="" oldVoice=false commandResult=false freshLookup=live freshValid=true freshUserId=18 freshName="bot-18" freshVoice=false pass=true/.test(line)),
"reuse evidence must identify every stale default, command result, and fresh liveness/identity check");
// End cleanup drops retained handles and preserves this run's proof counters.
nextSlot = 16;
command("sm_mixed_slotcycle");
assert.equal(timers.size, 0);
assert.equal(field("slotPending"), 1);
assert.match(command("sm_mixed_slotcleanup"), /pending=0 checking=0/);
assert.equal(timers.size, 0);
assert.equal(field("slotPending"), 0);
assert.equal(field("slotReuses"), 1);
// A fresh pilot/soak cannot inherit that proof.
command("sm_mixed_slotreset");
assert.equal(field("slotReuses"), 0);
assert.equal(field("slotAttempts"), 0);
clients.clear(); clientAt(0);
for (let slot = 1; slot <= 64; slot++) {
  nextSlot = slot;
  assert.match(command("sm_mixed_slotcycle"), /SLOT armed/);
  advance(60_000);
  plugin.OnClientActive(clientAt(nextSlot));
}
assert.equal(field("slotPending"), 64);
assert.equal(field("slotReuses"), 0);
assert.match(command("sm_mixed_slotcycle"), /SLOT failed .*reason=pending-cap cap=64/);
assert.equal(field("slotFailures"), 1);
assert.equal(field("slotPending"), 64);
assert.equal(timers.size, 0);
assert.deepEqual(serverCommands, []);
command("sm_mixed_slotcleanup");
assert.equal(field("slotPending"), 0);
assert.equal(field("slotReuses"), 0, "cleanup/skips cannot manufacture proof");
console.log("PASS: actual stale probe after 16 different slots; bounded retention, timer cleanup and proof reset");
