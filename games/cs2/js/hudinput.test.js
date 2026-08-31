const test = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const IN_SCORE = 1n << 16n;

function mount() {
  const handlers = [];
  globalThis.__s2pkg_usercmd = {
    UserCmd: {
      onRun(fn) {
        handlers.push(fn);
      },
    },
  };

  const src = readFileSync(join(__dirname, "hudinput.js"), "utf8");
  new Function(src)();
  return { HudInput: globalThis.__s2pkg_hudinput.HudInput, onRun: handlers[0] };
}

function run(onRun, slot, buttons) {
  const cmd = { buttons };
  onRun(slot, cmd);
  return cmd;
}

test("arm then first Tab down swallows IN_SCORE and activates", () => {
  const { HudInput, onRun } = mount();
  HudInput.arm(1);

  const cmd = run(onRun, 1, IN_SCORE);

  assert.equal(cmd.buttons, 0n);
  assert.equal(HudInput.isActive(1), true);
});

test("subsequent Tab while active is not swallowed", () => {
  const { HudInput, onRun } = mount();
  HudInput.arm(1);
  run(onRun, 1, IN_SCORE);

  const cmd = run(onRun, 1, IN_SCORE);

  assert.equal(cmd.buttons, IN_SCORE);
});

test("a Tab held before arm is swallowed until release, then the next press activates", () => {
  const { HudInput, onRun } = mount();
  run(onRun, 1, IN_SCORE);
  HudInput.arm(1);

  const held = run(onRun, 1, IN_SCORE);
  assert.equal(held.buttons, 0n);
  assert.equal(HudInput.isActive(1), false);

  run(onRun, 1, 0n);
  const nextPress = run(onRun, 1, IN_SCORE);
  assert.equal(nextPress.buttons, 0n);
  assert.equal(HudInput.isActive(1), true);
});

test("disarm clears state and leaves Tab untouched", () => {
  const { HudInput, onRun } = mount();
  HudInput.arm(1);
  run(onRun, 1, IN_SCORE);
  HudInput.disarm(1);

  const cmd = run(onRun, 1, IN_SCORE);

  assert.equal(HudInput.isActive(1), false);
  assert.equal(HudInput.isArmed(1), false);
  assert.equal(cmd.buttons, IN_SCORE);
});

test("consumeActive returns true once, then false", () => {
  const { HudInput, onRun } = mount();
  HudInput.arm(1);
  run(onRun, 1, IN_SCORE);

  assert.equal(HudInput.consumeActive(1), true);
  assert.equal(HudInput.consumeActive(1), false);
  assert.equal(HudInput.isActive(1), false);
});

test("isArmed reflects arm and disarm", () => {
  const { HudInput } = mount();

  assert.equal(HudInput.isArmed(1), false);
  HudInput.arm(1);
  assert.equal(HudInput.isArmed(1), true);
  HudInput.disarm(1);
  assert.equal(HudInput.isArmed(1), false);
});
