import { test } from "node:test";
import assert from "node:assert/strict";
import { validatePluginGamedata } from "../src/gamedata/validate.ts";

const PERMS = ["engine:calls"];
const sigCall = {
  signatures: { Foo: { linuxsteamrt64: { module: "libserver.so", pattern: "55 48", resolve: "direct" } } },
  calls: { foo: { receiver: { kind: "entity" }, target: { kind: "signature", name: "Foo" }, args: [], returns: "void" } },
};

test("a valid signature-backed call passes", () => {
  assert.deepEqual(validatePluginGamedata(sigCall, { permissions: PERMS }), []);
});

test("calls section without engine:calls permission is rejected", () => {
  const errs = validatePluginGamedata(sigCall, { permissions: [] });
  assert.ok(errs.some((e) => e.includes("engine:calls")));
});

test("vtable target without validate.prologue is rejected", () => {
  const gd = { calls: { f: { receiver: { kind: "entity" },
    target: { kind: "vtable", class: "C", linuxsteamrt64: { index: 5 } }, args: [], returns: "void" } } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("validate.prologue")));
});

test("six integer-class args are rejected (this occupies rdi)", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["int", "int", "int", "bool", "entity", "string"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("integer-class")));
});

test("five integer-class plus eight float args are accepted", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["int", "int", "int", "bool", "entity", ...Array(8).fill("float")];
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test("nine float args are rejected", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = Array(9).fill("float");
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("float")));
});

test("unknown arg kind is rejected", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["pointer"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("pointer")));
});

test("string return is rejected in v1", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.returns = "string";
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("returns")));
});

test("an offsets section is rejected in v1", () => {
  const gd = { ...structuredClone(sigCall), offsets: { X: { linuxsteamrt64: 8 } } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("offsets")));
});

test("a signature target naming a missing signature is rejected", () => {
  const gd = { calls: { f: { receiver: { kind: "entity" },
    target: { kind: "signature", name: "Nope" }, args: [], returns: "void" } } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("Nope")));
});

test("receiver.via requires both class and field", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.receiver.via = { class: "CBasePlayerPawn" };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("via")));
});
