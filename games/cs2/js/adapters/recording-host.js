// TEST SUPPORT ONLY — never a bootstrap input. A pointer-free recording model of the S2
// engine-function adapter facade (core/src/v8host/function_adapter.rs) that the CS2 adapters
// are written against: package adapters registered per plugin context, subscribers delivered
// through `cursor.invokeNext()` as frozen {action, returnValue, frameRevision} records, the
// typed-decision rules, per-dispatch shared scratch with per-callback staging, busy-context
// skipping for nested dispatch, PRE proposals forcing the adapter's POST with
// `proposedReturn`/`originalReturnValue`/`overrideReturn`, borrowed-record positions (per-field
// PRE-only writes staged per callback, accepted with the callback's decision, visible to later
// callbacks, committed to the native record before the original; readonly in POST), and a native
// "original" whose timing is recorded in `trace`. It models the contract the adapters rely on; it is not a proof
// of the Rust implementation, which has its own tests.
"use strict";
const vm = require("node:vm");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const CONTRACTS = {
  "legacy.acquire.v1": "69247dc63a6200f5bb8c8ff651b8dd632e8d6af9ad4a0b8d2933199800bc48c0",
  "legacy.hud-click.v1": "28c0c9833d521cadd4eb03254f63ef7dcb1cd8b728f48ebfa8835b82ff03ecdd",
  "legacy.damage.v1": "37e53fb0dfcb8d986cacaa28bba02394f9957e3600411dc5d8785e0e4bd0711e",
};

function isI32(v) { return typeof v === "number" && Number.isInteger(v) && v >= -2147483648 && v <= 2147483647; }

// binding: {name, returns: "i32"|"void", suppression: "generic"|"none", scratch: [names], pre: bool, post: bool}
function createHost(binding) {
  const adapters = [];      // {context, id, hash, pre, post}
  const subscriptions = []; // {context, phase, wrapper, adapter}
  const busy = new Set();
  const trace = [];
  const logs = [];
  let revision = 0;

  function mount(name, sources, extra) {
    const context = { name };
    const natives = {
      register(id, hash, callbacks) {
        if (hash !== CONTRACTS[id]) throw new Error("semantic contract hash conflict");
        if (adapters.some(a => a.context === context && a.id === id)) throw new Error("duplicate package adapter");
        adapters.push({ context, id, hash, pre: callbacks.pre || null, post: callbacks.post || null });
        return {};
      },
      subscribe(functionName, id, phase, wrapper) {
        if (functionName !== binding.name) throw new Error("undeclared engine function");
        if (binding.unavailable) throw new Error("binding unavailable");
        if (!binding[phase]) throw new Error("undeclared subscription surface");
        const row = adapters.find(a => a.context === context && a.id === id);
        if (!row) throw new Error("current package adapter unavailable");
        const sub = { context, phase, wrapper, id, disposed: false };
        subscriptions.push(sub);
        return { dispose() { sub.disposed = true; }, get status() { return sub.disposed ? "disposed" : "active"; } };
      },
    };
    const sandbox = {
      console: { log: message => logs.push(`${name}: ${message}`) },
      __s2_adapter_contracts: Object.freeze({ ...CONTRACTS }),
      __s2_function_adapter_register: natives.register,
      __s2_function_adapter_subscribe: natives.subscribe,
      ...(extra || {}),
    };
    sandbox.globalThis = sandbox;
    vm.createContext(sandbox);
    for (const file of sources) vm.runInContext(readFileSync(file, "utf8"), sandbox, { filename: file });
    // Bootstrap-only natives disappear once the package source has run.
    delete sandbox.__s2_function_adapter_register;
    delete sandbox.__s2_function_adapter_subscribe;
    context.sandbox = sandbox;
    return context;
  }

  function decision(value, phase, forAdapter) {
    if (phase === "post") {
      if (value !== undefined) throw new Error("POST callback must return void");
      return { action: 0 };
    }
    if (value === undefined) return { action: 0 };
    if (typeof value === "number") {
      if (!Number.isInteger(value)) throw new Error("invalid decision");
      if (binding.suppression === "none" && (value === 2 || value === 3)) throw new Error("suppression:none forbids Handled/Stop");
      if (value === 0 || value === 1 || (binding.returns === "void" && (value === 2 || value === 3))) return { action: value };
      throw new Error("suppression requires typed decision object");
    }
    if (value === null || typeof value !== "object") throw new Error("invalid adapter decision");
    if (!isI32(value.action)) throw new Error("suppression action must be an int32");
    if (forAdapter && value.action === 1) {
      if (!isI32(value.returnValue)) throw new Error("typed scalar required");
      return { action: 1, proposal: value.returnValue };
    }
    if (value.action !== 2 && value.action !== 3) throw new Error("invalid suppression action");
    if (binding.suppression === "none") throw new Error("suppression:none forbids suppression decision objects");
    if (binding.returns !== "void" && !isI32(value.returnValue)) throw new Error("typed scalar required");
    return { action: value.action, returnValue: binding.returns === "void" ? undefined : value.returnValue };
  }

  // One leased view. `lease.live` flips false when the callback returns (block-scoped views).
  function view(state, phase, lease, adapterLease) {
    const frame = {};
    const guard = () => { if (!lease.live) throw new Error("expired function frame lease"); };
    for (const [key, read] of Object.entries(state.fields)) {
      Object.defineProperty(frame, key, { enumerable: true, get() { guard(); return read(); } });
    }
    for (const [key, record] of Object.entries(state.records || {})) {
      Object.defineProperty(frame, key, { enumerable: true, get() { guard(); return recordView(state, key, record, phase, lease); } });
    }
    if (phase === "pre") {
      for (const slot of binding.scratch || []) {
        Object.defineProperty(frame, slot, {
          enumerable: true,
          get() { guard(); return slot in lease.pending ? lease.pending[slot] : state.scratch[slot]; },
          set(v) { guard(); if (!isI32(v)) { lease.poisoned = true; throw new TypeError(`scratch ${slot}: typed scalar required`); } lease.pending[slot] = v; },
        });
      }
    } else {
      Object.defineProperty(frame, "returnValue", { enumerable: true, get() { guard(); return state.current; } });
      frame.skipped = state.skipped;
      if (adapterLease) {
        Object.defineProperty(frame, "originalReturnValue", { get() { guard(); return state.original; } });
        frame.overrideReturn = v => { guard(); if (!isI32(v)) throw new Error("typed scalar required"); state.current = v; trace.push(`override:${v}`); return v; };
        if (state.proposal !== undefined) frame.proposedReturn = state.proposal;
      }
    }
    if (state.hiddenReferencedBy) frame.hiddenReferencedBy = (...args) => { guard(); return state.hiddenReferencedBy(...args); };
    return Object.freeze(frame);
  }

  // A borrowed record: null when the native pointer is null; otherwise per-field accessors.
  function recordView(state, key, record, phase, lease) {
    if (!record.present) return null;
    const view = {};
    for (const field of Object.keys(record.values)) {
      const slot = `${key}.${field}`;
      Object.defineProperty(view, field, {
        enumerable: true,
        get() {
          if (!lease.live) throw new Error("expired function frame lease");
          if (slot in lease.pendingRecords) return lease.pendingRecords[slot];
          if (slot in state.recordEdits) return state.recordEdits[slot];
          return record.values[field];
        },
        set(v) {
          if (!lease.live) throw new Error("expired function frame lease");
          if (phase !== "pre" || !(record.writable || []).includes(field)) {
            lease.poisoned = true;
            throw new Error("record field is readonly");
          }
          if (typeof v !== "number") { lease.poisoned = true; throw new TypeError("typed scalar required"); }
          lease.pendingRecords[slot] = record.storage && record.storage[field] === "f32" ? Math.fround(v) : v;
        },
      });
    }
    return Object.freeze(view);
  }

  function accept(state, lease) {
    if (lease.poisoned) return false;
    Object.assign(state.scratch, lease.pending);
    Object.assign(state.recordEdits, lease.pendingRecords);
    lease.pending = {}; lease.pendingRecords = {};
    return true;
  }

  function runSubscriber(state, phase, sub) {
    const lease = { live: true, pending: {}, pendingRecords: {}, poisoned: false };
    busy.add(sub.context);
    let result;
    try {
      const value = sub.wrapper(view(state, phase, lease, false));
      lease.live = false;
      result = decision(value, phase, false);
      if (!accept(state, lease)) throw new Error("rejected staged batch");
    } catch (e) {
      lease.live = false;
      logs.push(`host: function subscriber decision: ${e.message}`);
      result = { action: 0 };
    } finally {
      busy.delete(sub.context);
    }
    return result;
  }

  function runAdapter(state, phase, adapter, subscribers) {
    const lease = { live: true, pending: {}, pendingRecords: {}, poisoned: false };
    let index = 0;
    const cursor = Object.freeze({
      invokeNext() {
        if (!lease.live) throw new Error("expired cursor");
        if (lease.poisoned) throw new Error("rejected whole record edit batch");
        accept(state, lease);
        while (index < subscribers.length) {
          const sub = subscribers[index++];
          // The adapter's own context is busy by construction and still delivered (S2 js_cursor).
          if (sub.disposed || (sub.context !== adapter.context && busy.has(sub.context))) continue;
          const d = runSubscriber(state, phase, sub);
          if (d.action === 3) index = subscribers.length;
          revision += 1;
          return Object.freeze({ action: d.action, returnValue: d.returnValue, frameRevision: revision });
        }
        return null;
      },
    });
    busy.add(adapter.context);
    try {
      const value = adapter[phase](Object.freeze({ frame: view(state, phase, lease, true), phase, cursor }));
      lease.live = false;
      const result = decision(value, phase, true);
      if (!accept(state, lease)) throw new Error("rejected whole record edit batch");
      return result;
    } finally {
      lease.live = false;
      busy.delete(adapter.context);
    }
  }

  function pick(phase) {
    return adapters.find(a => a[phase] && !busy.has(a.context)) || null;
  }

  // native: {fields: {name: () => value}, records?: {name: {present, values, writable?, storage?}},
  //          original: () => engine return, hiddenReferencedBy?, peer?}
  // peer: {skip: true, returnValue} models a higher KHook peer superseding the original.
  // Record values are the NATIVE storage: PRE's accepted edits are written into them before the
  // original, so the original and POST observe committed values.
  function dispatch(native) {
    const state = {
      fields: native.fields, records: native.records || {}, recordEdits: {}, scratch: {},
      hiddenReferencedBy: native.hiddenReferencedBy,
      skipped: false, current: undefined, original: undefined, proposal: undefined,
    };
    for (const slot of binding.scratch || []) state.scratch[slot] = 0;
    trace.push(`native-pre:${binding.name}`);
    const pre = subscriptions.filter(s => s.phase === "pre" && !s.disposed && !busy.has(s.context));
    const post = subscriptions.filter(s => s.phase === "post" && !s.disposed && !busy.has(s.context));
    let action = 0, suppressReturn;
    const adapterPre = pre.length ? pick("pre") : null;
    if (pre.length && !adapterPre) throw new Error("no eligible synchronous package adapter instance");
    const postAdapter = pick("post");
    if (adapterPre) {
      const d = runAdapter(state, "pre", adapterPre, pre);
      action = d.action;
      if (d.proposal !== undefined) state.proposal = d.proposal;
      if (action >= 2) suppressReturn = d.returnValue;
    }
    // PRE commit: accepted record edits reach native storage before the original.
    for (const [slot, value] of Object.entries(state.recordEdits)) {
      const [key, field] = slot.split(".");
      state.records[key].values[field] = value;
      trace.push(`commit:${slot}=${value}`);
    }
    state.recordEdits = {};
    if (action >= 2) {
      state.skipped = true;
      state.current = suppressReturn;
      trace.push("original-skipped");
    } else if (native.peer && native.peer.skip) {
      state.skipped = true;
      state.current = native.peer.returnValue;
      trace.push("original-skipped-by-peer");
    } else {
      trace.push("original");
      state.original = native.original ? native.original() : undefined;
      state.current = state.original;
    }
    const forced = state.proposal !== undefined && postAdapter;
    if ((post.length || forced) && postAdapter) {
      runAdapter(state, "post", postAdapter, post);
    }
    trace.push(`native-return:${state.current}`);
    return { returnValue: state.current, skipped: state.skipped };
  }

  return { mount, dispatch, trace, logs, adapters, subscriptions };
}

const ADAPTER_DIR = __dirname;
module.exports = { createHost, CONTRACTS, adapterSource: name => join(ADAPTER_DIR, name) };
