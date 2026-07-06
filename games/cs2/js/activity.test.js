const test = require("node:test");
const assert = require("node:assert");
const { computeActivitySource } = require("./activity.js");

// signature: computeActivitySource(flags, actorLabel, actorReal, recipientIsAdmin, recipientIsRoot, recipientIsActor)
test("default flags 13: non-admin recipient sees generic label, shown", () => {
  const r = computeActivitySource(13, "ADMIN", "gkh", false, false, false);
  assert.strictEqual(r.show, true);
  assert.strictEqual(r.name, "ADMIN");
});
test("default flags 13: admin recipient sees real name", () => {
  const r = computeActivitySource(13, "ADMIN", "gkh", true, false, false);
  assert.strictEqual(r.show, true);
  assert.strictEqual(r.name, "gkh");
});
test("recipient is the actor: always real name", () => {
  const r = computeActivitySource(13, "ADMIN", "gkh", false, false, true);
  assert.strictEqual(r.name, "gkh");
});
test("flags 0: nobody is shown", () => {
  assert.strictEqual(computeActivitySource(0, "ADMIN", "gkh", false, false, false).show, false);
  assert.strictEqual(computeActivitySource(0, "ADMIN", "gkh", true, false, false).show, false);
});
test("flag 2 (kNonAdminsNames): non-admin sees real name", () => {
  const r = computeActivitySource(2, "ADMIN", "gkh", false, false, false);
  assert.strictEqual(r.show, true);
  assert.strictEqual(r.name, "gkh");
});
test("flag 16 (kRootNames): only root recipient sees real name + is shown", () => {
  assert.strictEqual(computeActivitySource(16, "ADMIN", "gkh", true, true, false).name, "gkh");
  assert.strictEqual(computeActivitySource(16, "ADMIN", "gkh", true, true, false).show, true);
  // admin-but-not-root under flags 16 alone: not shown
  assert.strictEqual(computeActivitySource(16, "ADMIN", "gkh", true, false, false).show, false);
});
test("show=false short-circuit: name is the generic label even when recipient is the actor", () => {
  // flags 0 => not shown; recipientIsActor alone would set useReal, but name must stay generic when !show
  const r = computeActivitySource(0, "ADMIN", "gkh", false, false, true);
  assert.strictEqual(r.show, false);
  assert.strictEqual(r.name, "ADMIN");
});
