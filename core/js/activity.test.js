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

const { parseShowActivity, planActivity, describeIdentity } = require("./activity.js");

test("parseShowActivity: unset / junk / out of range fall back to 13", () => {
  assert.strictEqual(parseShowActivity(""), 13);
  assert.strictEqual(parseShowActivity("abc"), 13);
  assert.strictEqual(parseShowActivity("32"), 13);
  assert.strictEqual(parseShowActivity(undefined), 13);
  assert.strictEqual(parseShowActivity("0"), 0);
  assert.strictEqual(parseShowActivity(" 31 "), 31);
});

const people = [
  { slot: 0, isAdmin: true, isRoot: false },   // the actor
  { slot: 1, isAdmin: false, isRoot: false },  // a player
  { slot: 2, isAdmin: true, isRoot: false },   // another admin
];

test("planActivity: chat actor sees own line by real name; others per flags; no console copy", () => {
  const r = planActivity(13, { slot: 0, label: "ADMIN", real: "gkh", replySource: "chat" }, people);
  assert.strictEqual(r.actorConsole, false);
  assert.deepStrictEqual(r.chat, [
    { slot: 0, name: "gkh" }, { slot: 1, name: "ADMIN" }, { slot: 2, name: "gkh" },
  ]);
});

test("planActivity: console actor gets one console copy and no chat duplicate", () => {
  const r = planActivity(13, { slot: 0, label: "ADMIN", real: "gkh", replySource: "console" }, people);
  assert.strictEqual(r.actorConsole, true);
  assert.deepStrictEqual(r.chat.map(c => c.slot), [1, 2]);
});

test("planActivity: server console actor", () => {
  const r = planActivity(13, { slot: -1, label: "Console", real: "Console", replySource: "server" }, people);
  assert.strictEqual(r.actorConsole, true);
  assert.deepStrictEqual(r.chat, [
    { slot: 0, name: "Console" }, { slot: 1, name: "Console" }, { slot: 2, name: "Console" },
  ]);
});

test("planActivity: flags 0 still shows the chat actor their own action", () => {
  const r = planActivity(0, { slot: 0, label: "ADMIN", real: "gkh", replySource: "chat" }, people);
  assert.deepStrictEqual(r.chat, [{ slot: 0, name: "gkh" }]);
});

test("describeIdentity: SM %L shape, quotes and angle brackets stripped from the name", () => {
  assert.strictEqual(describeIdentity(null), "Console<0><Console><Console>");
  assert.strictEqual(describeIdentity({ name: 'a"<b>', userId: 4, steamId: "76561198000000000", isBot: false }),
    "ab<4><76561198000000000><>");
  assert.strictEqual(describeIdentity({ name: "Bot", userId: 9, steamId: "0", isBot: true }), "Bot<9><BOT><>");
});
