#!/usr/bin/env node
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
import { performance } from "node:perf_hooks";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../../..");
const baselineGit = "e63ac07e70d6cb2e1ac94f5efd4f89c2c3719faf";
const uiSource = readFileSync(join(repo, "games/cs2/js/ui.js"), "utf8");
const candidateSource = readFileSync(join(repo, "games/cs2/js/components.js"), "utf8");
const baselineSource = execFileSync("git", ["show", `${baselineGit}:games/cs2/js/components.js`], {
  cwd: repo, encoding: "utf8", maxBuffer: 16 * 1024 * 1024,
});
const sharedSwitchSource = readFileSync(join(repo, "games/cs2/js/shared-switch-fixture.js"), "utf8");
const sourceHash = source => crypto.createHash("sha256").update(source).digest("hex");

function percentile(values, fraction) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.ceil(fraction * sorted.length) - 1];
}

// This harness evaluates the shipped ui.js and the selected components.js source in a VM. The
// host boundary is intentionally small: it accepts the same engine calls as the node integration
// fixture, records the final layout state, and leaves candidate construction/paging/diffing to the
// real game-package code.
function mount(componentSource, players) {
  const clients = new Map();
  for (let slot = 1; slot <= players; slot++) {
    clients.set(slot, { slot, generation: slot, signonState: 6, isValid: () => true });
  }
  const entities = [];
  const frameHandlers = [];
  const rendered = new Map();
  let driveAttempts = 0, engineAttempts = 0, submittedWrites = 0;
  function submit(name, args) {
    engineAttempts++; submittedWrites++;
    if (name === "setDialogVariableStringForPlayer") {
      rendered.set(`text|${args[1]}|${args[2]}|${args[3]}`, String(args[4]));
    } else if (name === "setHasClassForPlayer") {
      rendered.set(`class|${args[1]}|${args[2]}|${args[3]}`, String(args[4]));
    }
    return undefined;
  }
  const context = vm.createContext({
    console: { log() {} },
    exports: {},
    __s2pkg_cs2: {},
    __s2pkg_cs2_calls: { call: name => (...args) => submit(name, args), status: () => "available" },
    __s2pkg_entity: {
      Entity: { findByClass: () => entities },
      createEntity(_cls, kv) {
        const entity = { index: 1, id: 1, name: kv.targetname, isValid: () => true };
        entities.push(entity); return entity;
      },
    },
    __s2pkg_clients: {
      _same: (a, b) => !!a && !!b && a.slot === b.slot && a.generation === b.generation,
      Clients: { fromSlot: slot => clients.get(slot) || null, all: () => [...clients.values()],
        onActive() {}, onDisconnect() {} },
    },
    __s2pkg_server: { Server: { onMapStart() {}, getCvar: () => "3790153369" } },
    __s2pkg_frame: { OnGameFrame: { subscribe(fn) { frameHandlers.push(fn); return { dispose() {} }; } } },
    __s2pkg_timers: { after() {} },
    __s2_hook_on: () => 1,
  });
  vm.runInContext(sharedSwitchSource, context);
  const switchFixture = context.exports.sharedSwitchFixture(
    (index, id) => entities.find(entity => entity.index === index && entity.id === id),
    (name, entity, slot, on) => {
      engineAttempts++; submittedWrites++;
      rendered.set(`capture|${entity.id}|${slot}|${name}`, String(on));
      return null;
    },
  );
  context.__s2_shared_entity_switch = switchFixture.native(0);
  vm.runInContext(uiSource, context);
  vm.runInContext(componentSource, context);
  const base = context.__s2pkg_game_ctx.ui(fn => fn(), fn => fn);
  const kit = base.kit;
  for (const name of Object.keys(kit.layout._drive)) {
    const original = kit.layout._drive[name];
    kit.layout._drive[name] = function (...args) {
      driveAttempts++;
      return original.apply(this, args);
    };
  }
  return {
    kit,
    frame: () => frameHandlers.slice().forEach(fn => fn()),
    counters: () => ({ driveAttempts, engineAttempts, submittedWrites }),
    resetCounters() { driveAttempts = 0; engineAttempts = 0; submittedWrites = 0; },
    outputHash: () => sourceHash(JSON.stringify([...rendered.entries()].sort())),
  };
}

function runOne(source, mode, scenario) {
  const host = mount(source, scenario.players);
  let revision = 0, providerCalls = 0;
  const rowsByModal = Array.from({ length: scenario.visibleModals }, (_, modalIndex) =>
    Array.from({ length: scenario.rows }, (_, rowIndex) => ({
      id: `m${modalIndex}-r${rowIndex}`,
      a: `revision:${revision}:modal:${modalIndex}:row:${rowIndex}`,
      b: `slot-independent:${rowIndex % 17}`,
    })));
  const modals = rowsByModal.map((rows, modalIndex) => host.kit.modal({
    title: slot => `revision:${revision}:modal:${modalIndex}:slot:${slot}`,
    rows: slot => {
      providerCalls++;
      return rows.map(row => ({ ...row, a: `revision:${revision}:${row.id}:slot:${slot}` }));
    },
  }));
  for (const modal of modals) for (let slot = 1; slot <= scenario.players; slot++) modal.open(slot);
  revision = 1;
  providerCalls = 0;
  host.resetCounters();
  const started = performance.now();
  for (let intent = 0; intent < scenario.intents; intent++) {
    for (const modal of modals) {
      if (mode === "baseline") modal.refresh();
      else modal.invalidate();
    }
  }
  const queuePeak = mode === "candidate" ? host.kit._pendingInvalidationCount() : 0;
  if (mode === "candidate") host.frame();
  const elapsedMs = performance.now() - started;
  const afterDrain = mode === "candidate" ? host.kit._pendingInvalidationCount() : 0;
  const outputHash = host.outputHash();
  const updateCounters = host.counters();
  for (const modal of modals) for (let slot = 1; slot <= scenario.players; slot++) modal.close(slot);
  const cleanupQueue = mode === "candidate" ? host.kit._pendingInvalidationCount() : 0;
  return { elapsedMs, providerCalls, ...updateCounters, queuePeak, afterDrain, cleanupQueue, outputHash };
}

const quick = process.argv.includes("--quick");
const dimensions = quick ? { players: [1], visibleModals: [1], rows: [10], intents: [1, 10] } :
  { players: [1, 8, 32], visibleModals: [1, 2], rows: [10, 100, 1000], intents: [1, 10, 100] };
const scenarios = [];
for (const players of dimensions.players) for (const visibleModals of dimensions.visibleModals)
  for (const rows of dimensions.rows) for (const intents of dimensions.intents)
    scenarios.push({ players, visibleModals, rows, intents });

const raw = [];
for (const scenario of scenarios) {
  for (let pair = 0; pair < 5; pair++) {
    const order = pair % 2 === 0 ? ["baseline", "candidate"] : ["candidate", "baseline"];
    const pairResults = {};
    for (const mode of order) {
      pairResults[mode] = runOne(mode === "baseline" ? baselineSource : candidateSource, mode, scenario);
      raw.push({ scenario, pair, order: order.join("-then-"), mode, ...pairResults[mode] });
    }
    assert.equal(pairResults.baseline.outputHash, pairResults.candidate.outputHash,
      `final output differs for ${JSON.stringify(scenario)} pair ${pair}`);
  }
}

const summary = scenarios.map(scenario => {
  const matches = raw.filter(row => Object.keys(scenario).every(key => row.scenario[key] === scenario[key]));
  const byMode = mode => {
    const rows = matches.filter(row => row.mode === mode);
    const elapsed = rows.map(row => row.elapsedMs);
    const stable = key => rows.every(row => row[key] === rows[0][key]) ? rows[0][key] : rows.map(row => row[key]);
    return { medianMs: percentile(elapsed, 0.5), p95Ms: percentile(elapsed, 0.95),
      providerCalls: stable("providerCalls"), driveAttempts: stable("driveAttempts"),
      engineAttempts: stable("engineAttempts"), submittedWrites: stable("submittedWrites"),
      queuePeak: stable("queuePeak"), afterDrain: stable("afterDrain"), cleanupQueue: stable("cleanupQueue") };
  };
  return { ...scenario, baseline: byMode("baseline"), candidate: byMode("candidate") };
});

const result = {
  generatedAt: new Date().toISOString(), node: process.version, platform: `${process.platform}-${process.arch}`,
  baselineGit, baselineComponentsSha256: sourceHash(baselineSource),
  candidateComponentsSha256: sourceHash(candidateSource), uiSha256: sourceHash(uiSource),
  pairsPerScenario: 5, alternatingOrder: true, definitions: {
    providerCalls: "Invocations of the real modal row provider during the measured update window.",
    driveAttempts: "Calls from real components.js into real ui.js _drive methods during the measured update window, including cache-suppressed calls.",
    engineAttempts: "Calls during the measured update window that passed ui.js diffing and reached a resolved engine/native stub.",
    submittedWrites: "Successful mutating calls accepted by that stub during the measured update window; failures are not injected, so this equals engineAttempts.",
    queuePeak: "Actual components.js dirty-record queue length after intents and before the candidate frame drain.",
    cleanupQueue: "Actual dirty-record queue length after closing every measured modal.",
  },
  summary, raw,
};
const output = process.argv.find(arg => arg.startsWith("--output="))?.slice("--output=".length);
if (output) writeFileSync(resolve(process.cwd(), output), JSON.stringify(result, null, 2) + "\n");
else process.stdout.write(JSON.stringify(result, null, 2) + "\n");
