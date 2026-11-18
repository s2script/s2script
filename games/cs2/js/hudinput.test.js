const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const IN_SCORE = 1n << 16n;
const source = readFileSync(join(__dirname, "hudinput.js"), "utf8");

function mount() {
  const handlers = [];
  let disconnect = null;
  let nextId = 1;
  const clients = new Map();
  globalThis.__s2pkg_usercmd = {
    UserCmd: {
      onRun(fn) { fn.id = nextId++; handlers.push(fn); return fn.id; }
    }
  };
  globalThis.__s2_scope_dispose = (ids) => {
    for (const id of ids) {
      const i = handlers.findIndex((h) => h.id === id);
      if (i >= 0) handlers.splice(i, 1);
    }
  };
  globalThis.__s2pkg_clients = {
    Clients: {
      fromSlot(slot) { return clients.get(slot) || null; },
      onDisconnect(fn) { disconnect = fn; }
    }
  };
  globalThis.__s2pkg_cs2 = {};
  delete globalThis.__s2pkg_hudinput;
  new Function(source)();
  return {
    HudInput: globalThis.__s2pkg_hudinput.HudInput,
    handlers,
    disconnect,
    clients,
    run(slot, buttons) {
      const cmd = { buttons };
      if (handlers[0]) handlers[0](cmd, { slot });
      return cmd;
    }
  };
}

test.afterEach(() => {
  delete globalThis.__s2_scope_dispose;
  delete globalThis.__s2pkg_usercmd;
  delete globalThis.__s2pkg_clients;
  delete globalThis.__s2pkg_cs2;
  delete globalThis.__s2pkg_hudinput;
});

test("does not subscribe UserCmd until the first arm", () => {
  const mounted = mount();
  assert.equal(mounted.handlers.length, 0);
  mounted.HudInput.arm(1, { onActivate() {} });
  assert.equal(mounted.handlers.length, 1);
});

test("first Tab down swallows IN_SCORE, activates once, and keeps swallowing the hold", () => {
  const mounted = mount();
  let activations = 0;
  mounted.HudInput.arm(1, { onActivate() { activations++; } });

  const first = mounted.run(1, IN_SCORE);
  assert.equal(first.buttons, 0n);
  assert.equal(mounted.HudInput.isActive(1), true);
  assert.equal(activations, 1);

  const held = mounted.run(1, IN_SCORE);
  assert.equal(held.buttons, 0n);
  assert.equal(activations, 1);
});

test("after release, a later Tab is left intact for the scoreboard", () => {
  const mounted = mount();
  mounted.HudInput.arm(1, { onActivate() {} });
  mounted.run(1, IN_SCORE);
  mounted.run(1, 0n);

  const next = mounted.run(1, IN_SCORE);
  assert.equal(next.buttons, IN_SCORE);
  assert.equal(mounted.HudInput.isActive(1), true);
});

test("held Tab from before arm: swallow until release, then next press activates", () => {
  const mounted = mount();
  mounted.HudInput.arm(9, { onActivate() {} });
  mounted.run(1, IN_SCORE);
  let activated = false;
  mounted.HudInput.arm(1, { onActivate() { activated = true; } });

  const held = mounted.run(1, IN_SCORE);
  assert.equal(held.buttons, 0n);
  assert.equal(mounted.HudInput.isActive(1), false);
  assert.equal(activated, false);

  mounted.run(1, 0n);
  const next = mounted.run(1, IN_SCORE);
  assert.equal(next.buttons, 0n);
  assert.equal(mounted.HudInput.isActive(1), true);
  assert.equal(activated, true);
});

test("disarm clears state and leaves Tab untouched", () => {
  const mounted = mount();
  mounted.HudInput.arm(1, { onActivate() {} });
  mounted.run(1, IN_SCORE);
  mounted.HudInput.disarm(1);

  const cmd = mounted.run(1, IN_SCORE);
  assert.equal(mounted.HudInput.isActive(1), false);
  assert.equal(mounted.HudInput.isArmed(1), false);
  assert.equal(cmd.buttons, IN_SCORE);
});

test("disconnect disarms that slot", () => {
  const mounted = mount();
  mounted.HudInput.arm(4, { onActivate() {} });
  mounted.disconnect({ slot: 4 });
  assert.equal(mounted.HudInput.isArmed(4), false);
});

test("exports HudInput onto the cs2 package object", () => {
  const mounted = mount();
  assert.equal(globalThis.__s2pkg_cs2.HudInput, mounted.HudInput);
});

test("a deferred old disconnect preserves the replacement's Tab arm", () => {
  const mounted = mount();
  mounted.clients.set(4, { isValid: () => true });
  mounted.HudInput.arm(4, { onActivate() {} });
  mounted.disconnect({ slot: 4 });
  assert.equal(mounted.HudInput.isArmed(4), true);
});

test("slot reuse drops an old Tab callback before the disconnect replay", () => {
  const mounted = mount();
  let live = true, activated = 0;
  mounted.clients.set(4, { isValid: () => live });
  mounted.HudInput.arm(4, { onActivate() { activated++; } });
  live = false;
  mounted.clients.set(4, { isValid: () => true });
  assert.equal(mounted.run(4, IN_SCORE).buttons, IN_SCORE);
  assert.equal(activated, 0);
});

test("the usercmd hook is held only while a slot is armed", () => {
  const mounted = mount();
  mounted.HudInput.arm(1, { onActivate() {} });
  mounted.HudInput.arm(2, { onActivate() {} });
  assert.equal(mounted.handlers.length, 1, "one subscription for every armed slot");
  mounted.HudInput.disarm(1);
  assert.equal(mounted.handlers.length, 1, "slot 2 is still armed");
  mounted.HudInput.disarm(2);
  assert.equal(mounted.handlers.length, 0, "nothing armed: no per-command JS");
  mounted.HudInput.arm(3, { onActivate() {} });
  assert.equal(mounted.handlers.length, 1, "arming again subscribes again");
});

test("disarming from inside onActivate drops the hook after that command", () => {
  const mounted = mount();
  let activations = 0;
  mounted.HudInput.arm(1, { onActivate() { activations++; mounted.HudInput.disarm(1); } });
  const cmd = mounted.run(1, IN_SCORE);
  assert.equal(cmd.buttons, 0n, "the activating press is still swallowed");
  assert.equal(activations, 1);
  assert.equal(mounted.handlers.length, 0);
});

test("a slot armed again after the hook was dropped starts from Tab released", () => {
  const mounted = mount();
  mounted.HudInput.arm(1, { onActivate() {} });
  mounted.run(1, IN_SCORE);           // held while armed
  mounted.HudInput.disarm(1);         // hook dropped; the release is never seen
  let activated = false;
  mounted.HudInput.arm(1, { onActivate() { activated = true; } });
  const next = mounted.run(1, IN_SCORE);
  assert.equal(next.buttons, 0n);
  assert.equal(activated, true, "a press after re-arming activates, as on the very first arm");
});

test("the last armed slot disconnecting drops the hook", () => {
  const mounted = mount();
  mounted.HudInput.arm(4, { onActivate() {} });
  mounted.disconnect({ slot: 4 });
  assert.equal(mounted.handlers.length, 0);
});
