const test = require("node:test");
const assert = require("node:assert");
const colors = require("./colors.js");

const ZWSP = "\u200B";
const TABLE = { Default: "\x01", White: "\x01", Green: "\x04", LightRed: "\x0F" };

test.beforeEach(() => { colors.setTable(TABLE); colors._resetWarnings(); });

// expand() warns to console.log on an unknown tag. Three tests below deliberately hit that path
// (to prove deletion, no-table degrade, and dedup) and none of them should spam `node --test`'s
// output with the resulting "[s2script] WARN: ..." line. This installs a spy for the duration of
// `fn`, handing it the array of captured calls, and restores the original console.log in a
// `finally` so a throwing assertion inside `fn` can never leave console.log stubbed for later
// tests.
function withSpiedConsole(fn) {
  const calls = [];
  const original = console.log;
  console.log = (...args) => { calls.push(args); };
  try {
    fn(calls);
  } finally {
    console.log = original;
  }
}

test("expand: known tag becomes its byte, case-insensitively", () => {
  assert.strictEqual(colors.expand("{green}hi"), "\x04hi");
  assert.strictEqual(colors.expand("{GREEN}hi"), "\x04hi");
  assert.strictEqual(colors.expand("{lightred}hi"), "\x0Fhi");
});

test("expand: adjacent tags both resolve", () => {
  assert.strictEqual(colors.expand("{green}{white}hi"), "\x04\x01hi");
});

test("expand: unknown tag is deleted, not left literal", () => {
  withSpiedConsole(() => {
    assert.strictEqual(colors.expand("{gren}hi"), "hi");
  });
});

test("expand: unknown tag warns exactly once, even when the same tag repeats", () => {
  withSpiedConsole((calls) => {
    colors.expand("{gren}hi");
    colors.expand("{gren}bye");
    assert.strictEqual(calls.length, 1);
  });
});

test("expand: with no table at all, tags are deleted and text survives", () => {
  colors.setTable(null);
  withSpiedConsole(() => {
    assert.strictEqual(colors.expand("{green}hi"), "hi");
  });
});

test("expand: positional {1} slots are never touched", () => {
  assert.strictEqual(colors.expand("{green}{1} joined"), "\x04{1} joined");
});

test("chatLine: expansion precedes the ZWSP decision", () => {
  // The whole point: after expansion the line leads with \x04, not "{", so the
  // ZWSP is prepended and the colour byte is not swallowed by the chat box.
  assert.strictEqual(colors.chatLine("", "{green}hi"), ZWSP + "\x04hi");
});

test("chatLine: an already-led line is not double-prefixed", () => {
  assert.strictEqual(colors.chatLine("", ZWSP + "hi"), ZWSP + "hi");
  assert.strictEqual(colors.chatLine("", " hi"), " hi");
});

test("chatLine: the caller-owned prefix is expanded too", () => {
  assert.strictEqual(colors.chatLine("{green}", "hi"), ZWSP + "\x04hi");
});

test("consoleLine: tags and raw control bytes both vanish", () => {
  assert.strictEqual(colors.consoleLine("{green}hi"), "hi");
  assert.strictEqual(colors.consoleLine("\x04hi"), "hi");
  assert.strictEqual(colors.consoleLine("{green}a\x01b"), "ab");
});
