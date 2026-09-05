const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

// Lifecycle tests for ui.js's paint caches (lastValue / cursorLeases / visiblePanels /
// meterClass / disabled). Every cache mirrors state that lives on the LAYOUT ENTITY, so each one
// must die with the entity it describes — a cache that survives its entity suppresses the very
// writes that would repaint the replacement (a partial paint), and a surviving cursor lease
// blocks the capture-off a new entity never had in the first place.

function evalFile(name) {
  const src = readFileSync(join(__dirname, name), "utf8");
  new Function(src)();
}

test.afterEach(() => {
  delete globalThis.__s2pkg_entity;
  delete globalThis.__s2pkg_cs2_calls;
  delete globalThis.__s2pkg_server;
  delete globalThis.__s2pkg_clients;
  delete globalThis.__s2pkg_game_ctx;
  delete globalThis.__s2_hook_on;
  delete globalThis.__s2_shared_entity_switch;
});

// A tiny fake world: entities are {index, id, name, isValid()} like real EntityRefs (id is the
// host-minted identity the implementation must key its caches on), and every engine call is
// recorded verbatim so a test can distinguish "suppressed by cache" from "sent".
function mount() {
  const calls = [];
  const entities = [];
  let nextId = 1;

  function create(targetname) {
    const e = {
      index: entities.length + 1,
      id: nextId++,
      name: targetname,
      valid: true,
      isValid() { return this.valid; },
    };
    entities.push(e);
    return e;
  }

  globalThis.__s2pkg_entity = {
    Entity: { findByClass: () => entities.filter((e) => e.valid) },
    createEntity: (_cls, kv) => create(kv.targetname),
  };
  globalThis.__s2pkg_cs2_calls = {
    call: (name) => (...args) => { calls.push({ name, args }); },
    status: () => "",
  };
  const switches = require("./shared-switch-fixture.js").sharedSwitchFixture(
    (index, id) => entities.find(e => e.index === index && e.id === id && e.valid),
    (name, entity, slot, on) => { calls.push({ name, args: [entity, slot, on] }); return null; });
  globalThis.__s2_shared_entity_switch = switches.native("test");
  const mapStartHandlers = [];
  globalThis.__s2pkg_server = {
    Server: {
      onMapStart: (fn) => mapStartHandlers.push(fn),
      // The single probe addon, so the MAM banner takes the quiet "present, none missing" path.
      getCvar: () => "3790153369",
    },
  };
  const activeHandlers = [];
  const disconnectHandlers = [];
  globalThis.__s2pkg_clients = {
    Clients: {
      all: () => [{ signonState: 6 }],
      onActive: (fn) => activeHandlers.push(fn),
      onDisconnect: (fn) => disconnectHandlers.push(fn),
    },
  };
  globalThis.__s2_hook_on = () => 0;

  evalFile("ui.js");
  // reg executes immediately: the map-start / client subscriptions are live, and the fake active
  // client makes the ui ready at once.
  const ns = globalThis.__s2pkg_game_ctx.ui((thunk) => thunk(), (fn) => fn);

  return {
    ns,
    calls,
    entities,
    callsFor(name) { return calls.filter((c) => c.name === name); },
    liveEntity() { return entities.filter((e) => e.valid)[0] || null; },
    killLive() { for (const e of entities) e.valid = false; },
    // The old entity dies with the map, resetForMap fires, and the next active client re-spawns
    // the registered layout — the real order of a map change.
    mapChange() {
      this.killLive();
      mapStartHandlers.forEach((f) => f());
      activeHandlers.forEach((f) => f());
    },
    disconnect(slot) { disconnectHandlers.forEach((f) => f({ slot })); },
    // Mid-map replacement WITHOUT any lifecycle notification: the entity is killed and an
    // identically-named one appears (as after a plugin elsewhere re-created it). The huds under
    // test are told nothing — they must notice by identity.
    replaceLiveSilently() {
      const name = this.liveEntity().name;
      this.killLive();
      return create(name);
    },
  };
}

test("a map change drops the value caches so the new entity gets a full repaint", (t) => {
  const m = mount();
  const hud = m.ns.hud();

  assert.equal(hud.setText(0, "s2_dialog_title", "Hello"), null);
  assert.equal(hud.setText(0, "s2_dialog_title", "Hello"), null);
  let sets = m.callsFor("setDialogVariableStringForPlayer");
  assert.equal(sets.length, 1, "an unchanged value is suppressed while the SAME entity lives");
  const firstEntity = sets[0].args[0];

  m.mapChange();

  assert.equal(hud.setText(0, "s2_dialog_title", "Hello"), null);
  sets = m.callsFor("setDialogVariableStringForPlayer");
  assert.equal(sets.length, 2,
    "the unchanged value MUST be re-sent after a map change — the new entity is at markup default");
  assert.notEqual(sets[1].args[0].id, firstEntity.id, "and it must go to the NEW entity");
});

test("a cursor lease does not survive the entity it captured on", (t) => {
  const m = mount();
  const hud = m.ns.hud();
  const view = hud.forSlot(2);

  assert.equal(view.show("s2_row_0", { cursor: true }), null);
  let captures = m.callsFor("setInputCaptureEnabledForPlayer");
  assert.equal(captures.length, 1);
  assert.deepEqual(captures[0].args.slice(1), [2, true]);

  m.mapChange();

  // The stale lease would have made acquireCursor think capture is already on ("had" non-empty)
  // and skip the enable — the new entity would never capture input at all.
  assert.equal(view.show("s2_row_0", { cursor: true }), null);
  captures = m.callsFor("setInputCaptureEnabledForPlayer");
  assert.equal(captures.length, 2, "capture must be re-enabled on the new entity");
  assert.equal(captures[1].args[2], true);
});

test("caches are keyed to entity IDENTITY, not to map start: a silent mid-map replacement self-heals", (t) => {
  const m = mount();
  const hud = m.ns.hud();

  assert.equal(hud.setText(1, "s2_dialog_body", "B"), null);
  assert.equal(hud.cursor(1, true), null);
  assert.equal(m.callsFor("setDialogVariableStringForPlayer").length, 1);
  assert.equal(m.callsFor("setInputCaptureEnabledForPlayer").length, 1);

  // No resetForMap runs here — this is the case a map-start-keyed reset cannot cover.
  const replacement = m.replaceLiveSilently();

  assert.equal(hud.setText(1, "s2_dialog_body", "B"), null);
  const sets = m.callsFor("setDialogVariableStringForPlayer");
  assert.equal(sets.length, 2, "resolving a DIFFERENT entity must invalidate the caches");
  assert.equal(sets[1].args[0].id, replacement.id);

  // The lease taken on the dead entity must not satisfy the new acquire.
  assert.equal(hud.cursor(1, true), null);
  const captures = m.callsFor("setInputCaptureEnabledForPlayer");
  assert.equal(captures[captures.length - 1].args[2], true,
    "capture must be re-enabled on the replacement entity");
});

test("forget releases owned capture and resets classes without unconditional duplicate capture-off", (t) => {
  const m = mount();
  const hud = m.ns.hud();
  const view = hud.forSlot(3);

  assert.equal(view.show("s2_row_1", { cursor: true }), null);
  assert.equal(hud.setMeter(3, "meter", 30), null);
  assert.equal(hud.setDisabled(3, "s2_btn_0", true), null);
  const before = m.calls.length;

  m.disconnect(3);

  const after = m.calls.slice(before);
  const ent = m.liveEntity();
  const captureOff = after.filter((c) =>
    c.name === "setInputCaptureEnabledForPlayer" && c.args[1] === 3 && c.args[2] === false);
  assert.equal(captureOff.length, 1, "disconnect must force input capture off for the slot");
  assert.ok(after.some((c) => c.name === "setHasClassForPlayer"
    && c.args[0] === ent && c.args[1] === 3 && c.args[2] === "s2_row_1"
    && c.args[3] === "s2-hidden" && c.args[4] === 1),
    "a panel the slot had visible is hidden again");
  assert.ok(after.some((c) => c.name === "setHasClassForPlayer"
    && c.args[1] === 3 && c.args[2] === "s2_meter_fill" && c.args[3] === "s2-w3" && c.args[4] === 0),
    "the painted meter width class is removed (or the next occupant's width would fight it)");
  assert.ok(after.some((c) => c.name === "setHasClassForPlayer"
    && c.args[1] === 3 && c.args[2] === "s2_btn_0" && c.args[3] === "s2-btn-disabled" && c.args[4] === 0),
    "the disabled class is removed for the next occupant");

  // Repeated local cleanup has no authority over other owners. The native host disconnect
  // path is separately responsible for clearing all owners even without JS subscribers.
  const mark = m.calls.length;
  hud.forget(3);
  const again = m.calls.slice(mark).filter((c) =>
    c.name === "setInputCaptureEnabledForPlayer" && c.args[1] === 3 && c.args[2] === false);
  assert.equal(again.length, 0, "no lease means no engine capture-off; other owners may still hold it");

  // And the slot's next occupant starts clean: everything re-sends.
  const mark2 = m.calls.length;
  assert.equal(view.show("s2_row_1", { cursor: true }), null);
  const fresh = m.calls.slice(mark2);
  assert.ok(fresh.some((c) => c.name === "setHasClassForPlayer" && c.args[4] === 0),
    "the show repaints (unhides) for the new occupant");
  assert.ok(fresh.some((c) => c.name === "setInputCaptureEnabledForPlayer" && c.args[2] === true),
    "the cursor re-acquires for the new occupant");
});

test("setDisabled's book survives the identity reset its own paint triggers", (t) => {
  const m = mount();
  const hud = m.ns.hud();
  hud.onClick("s2_btn_0", () => {});

  assert.equal(hud.setDisabled(4, "s2_btn_0", true), null);
  assert.equal(hud.dispatchClick(4, "s2_btn_0"), false);

  // Replace the entity silently, then re-disable: the paint inside setDisabled is what DETECTS
  // the replacement and resets the books. If the book entry were written before the paint, the
  // reset would swallow it and the greyed-out button would still dispatch.
  m.replaceLiveSilently();
  assert.equal(hud.setDisabled(4, "s2_btn_0", true), null);
  assert.equal(hud.dispatchClick(4, "s2_btn_0"), false,
    "a button painted disabled must not dispatch after an entity replacement");
});


test("missing host capture support is named and show rolls back its visible class", () => {
  const m = mount(), hud = m.ns.hud();
  delete globalThis.__s2_shared_entity_switch;
  assert.match(hud.show(2, "panel", { cursor: true }), /unavailable: shared entity switch/);
  const paint = m.callsFor("setHasClassForPlayer").filter(c => c.args[2] === "panel");
  assert.deepEqual(paint.map(c => c.args[4]), [0, 1]);
  assert.equal(m.callsFor("setInputCaptureEnabledForPlayer").length, 0, "no raw fallback");
});

test("host acquire errors roll back show and a later retry can succeed", () => {
  const m = mount(), hud = m.ns.hud();
  const native = globalThis.__s2_shared_entity_switch;
  globalThis.__s2_shared_entity_switch = () => "unavailable: test capture failure";
  assert.match(hud.show(2, "panel", { cursor: true }), /test capture failure/);
  globalThis.__s2_shared_entity_switch = native;
  assert.equal(hud.show(2, "panel", { cursor: true }), null);
  assert.deepEqual(m.callsFor("setInputCaptureEnabledForPlayer").map(c => c.args[2]), [true]);
});
