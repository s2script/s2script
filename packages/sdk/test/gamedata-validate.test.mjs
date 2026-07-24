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

// --- call-name gate (added after review: the generated .d.ts interpolates the name verbatim) ---

test("a call name that injects TypeScript is rejected", () => {
  const gd = structuredClone(sigCall);
  delete gd.calls.foo;
  gd.calls["y: (...a: any[]) => any; [k: string]: any; z"] = {
    receiver: { kind: "entity" }, target: { kind: "signature", name: "Foo" }, args: [], returns: "void",
  };
  const errs = validatePluginGamedata(gd, { permissions: PERMS });
  assert.ok(errs.some((e) => e.includes("plain identifier")),
    "an index-signature injection must be rejected — it defeats the whole typecheck gate");
});

test("call names that are not identifiers are rejected", () => {
  for (const bad of ["burn-target", "0abc", "has space", "a.b", ""]) {
    const gd = structuredClone(sigCall);
    delete gd.calls.foo;
    gd.calls[bad] = { receiver: { kind: "entity" }, target: { kind: "signature", name: "Foo" }, args: [], returns: "void" };
    assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("plain identifier")),
      `expected ${JSON.stringify(bad)} to be rejected`);
  }
});

test("reserved call names are rejected", () => {
  // Built via JSON.parse, not assignment: `obj["__proto__"] = x` sets the PROTOTYPE rather than
  // creating an own key, so an assignment-built fixture would not reach the validator at all.
  // JSON.parse does create a real own property — and JSON.parse is how gamedata actually arrives.
  for (const bad of ["constructor", "prototype", "__proto__"]) {
    const decl = { receiver: { kind: "entity" }, target: { kind: "signature", name: "Foo" }, args: [], returns: "void" };
    const gd = JSON.parse(JSON.stringify({ signatures: sigCall.signatures, calls: {} }));
    gd.calls = JSON.parse(`{${JSON.stringify(bad)}:${JSON.stringify(decl)}}`);
    assert.ok(Object.hasOwn(gd.calls, bad), `fixture for ${bad} must have an own key`);
    assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("reserved")),
      `expected ${JSON.stringify(bad)} to be rejected`);
  }
});

test("ordinary identifier call names are still accepted", () => {
  for (const ok of ["ignite", "dropActiveWeapon", "_private", "$dollar", "a0"]) {
    const gd = structuredClone(sigCall);
    delete gd.calls.foo;
    gd.calls[ok] = { receiver: { kind: "entity" }, target: { kind: "signature", name: "Foo" }, args: [], returns: "void" };
    assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), [], `expected ${ok} to pass`);
  }
});
