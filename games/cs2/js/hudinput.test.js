const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const IN_SCORE = 1n << 16n;
const source = readFileSync(join(__dirname, "hudinput.js"), "utf8");

function mount() {
  const handlers = [];
  let disconnect = null;
  globalThis.__s2pkg_usercmd = {
    UserCmd: {
      onRun(fn) { handlers.push(fn); }
    }
  };
  globalThis.__s2pkg_clients = {
    Clients: {
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
    run(slot, buttons) {
      const cmd = { buttons };
      handlers[0](cmd, { slot });
      return cmd;
    }
  };
}

test.afterEach(() => {
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
