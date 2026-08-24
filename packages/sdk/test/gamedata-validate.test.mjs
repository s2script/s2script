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

test("a Windows-only signature-backed call passes on any authoring host", () => {
  const gd = structuredClone(sigCall);
  gd.signatures.Foo = {
    windows64: { module: "server.dll", pattern: "48 89", resolve: "direct" },
  };
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test("both supported signature platforms are validated", () => {
  const gd = structuredClone(sigCall);
  gd.signatures.Foo.windows64 = { module: "server.dll", pattern: "48 89", resolve: "drect" };
  assert.ok(
    validatePluginGamedata(gd, { permissions: PERMS })
      .some((e) => e.includes("windows64") && e.includes("unknown resolve step")),
  );
});

test("a signature with no supported platform is rejected", () => {
  const gd = structuredClone(sigCall);
  gd.signatures.Foo = {
    macos64: { module: "server.dylib", pattern: "55 48", resolve: "direct" },
  };
  assert.ok(
    validatePluginGamedata(gd, { permissions: PERMS })
      .some((e) => e.includes("supported platform")),
  );
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

test("a Windows-only vtable target passes on any authoring host", () => {
  const gd = { calls: { f: { receiver: { kind: "entity" },
    target: { kind: "vtable", class: "C",
      windows64: { index: 5, validate: { prologue: "48 89" } } },
    args: [], returns: "void" } } };
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test("ten integer-class args are rejected (one past the budget)", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = Array(10).fill("int");
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("integer-class")));
});

test("six integer-class args are accepted (args past the sixth spill to the stack)", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["int", "int", "int", "bool", "entity", "string"];
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test('receiver.kind "none" is accepted (a static engine function)', () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.receiver = { kind: "none" };
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test('receiver.kind "none" cannot carry a via hop', () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.receiver = { kind: "none", via: { class: "C", field: "m_f" } };
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("via")));
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

// --- closed vocabularies (added after review) ---

test("an unknown resolve step is rejected", () => {
  const gd = structuredClone(sigCall);
  gd.signatures.Foo.linuxsteamrt64.resolve = "drect";
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("unknown resolve step")));
});

test("every resolve step the shim dispatches on is accepted", () => {
  for (const r of ["direct", "ctor-body-xref", "lea-disp"]) {
    const gd = structuredClone(sigCall);
    gd.signatures.Foo.linuxsteamrt64.resolve = r;
    assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), [], `expected ${r} to be accepted`);
  }
});

test("an unknown declared permission is rejected", () => {
  const errs = validatePluginGamedata(sigCall, { permissions: ["engine:calls", "totally:bogus"] });
  assert.ok(errs.some((e) => e.includes("unknown permission")));
});

// --- argNames (Tier C) ---

test("valid argNames are accepted", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["float", "int"];
  gd.calls.foo.argNames = ["lifetime", "flags"];
  assert.deepEqual(validatePluginGamedata(gd, { permissions: PERMS }), []);
});

test("argNames length must match args", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["float", "int"];
  gd.calls.foo.argNames = ["lifetime"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("positionally matched")));
});

test("a non-identifier argName is rejected", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["float"];
  gd.calls.foo.argNames = ["not a name"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("plain identifier")));
});

test("argName 'self' is reserved (it names the receiver)", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["float"];
  gd.calls.foo.argNames = ["self"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("reserved")));
});

// --- hooks (plugin-declared inbound detours) ---

const HOOK_PERMS = ["engine:calls", "engine:hooks"];
const hookGd = {
  signatures: { Foo: { linuxsteamrt64: { module: "libserver.so", pattern: "55 48", resolve: "direct" } } },
  hooks: {
    onX: {
      target: { kind: "signature", name: "Foo", validate: { prologue: "55 48" } },
      shape: "this_void",
      expose: { ctx: "custom" },
    },
  },
};

test("a valid hook descriptor passes", () => {
  assert.deepEqual(validatePluginGamedata(hookGd, { permissions: HOOK_PERMS }), []);
});

test("hooks section without engine:hooks permission is rejected", () => {
  const errs = validatePluginGamedata(hookGd, { permissions: ["engine:calls"] });
  assert.ok(errs.some((e) => e.includes("engine:hooks")));
});

test("engine:hooks is a known permission", () => {
  assert.deepEqual(validatePluginGamedata(sigCall, { permissions: ["engine:calls", "engine:hooks"] }), []);
});

test("a hook without validate is rejected", () => {
  const gd = structuredClone(hookGd);
  delete gd.hooks.onX.target.validate;
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("validate")));
});

test("an unknown hook shape is rejected", () => {
  const gd = structuredClone(hookGd);
  gd.hooks.onX.shape = "this_ptr_i32";
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("unknown hook shape")));
});

test("more params than the shape's arity is rejected", () => {
  const gd = structuredClone(hookGd);
  gd.hooks.onX.params = ["a"];
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("passes only 0")));
});

test("mutable must name a declared param", () => {
  const gd = structuredClone(hookGd);
  gd.hooks.onX.shape = "this_f32_i32_i32_i32";
  gd.hooks.onX.params = ["delay", "reason", "u3", "u4"];
  gd.hooks.onX.mutable = ["nope"];
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("mutable")));
});

test("a hook without expose.ctx is rejected", () => {
  const gd = structuredClone(hookGd);
  delete gd.hooks.onX.expose;
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("expose.ctx")));
});

test("bypassWith must name a call in the same gamedata", () => {
  const gd = structuredClone(hookGd);
  gd.hooks.onX.bypassWith = "missing";
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("bypassWith")));
});

test("bypassWith naming a real call is accepted", () => {
  const gd = {
    ...structuredClone(sigCall),
    hooks: {
      onX: {
        target: { kind: "signature", name: "Foo", validate: { prologue: "55 48" } },
        shape: "this_void",
        expose: { ctx: "custom" },
        bypassWith: "foo",
      },
    },
  };
  assert.deepEqual(validatePluginGamedata(gd, { permissions: HOOK_PERMS }), []);
});

test("a hook name that injects TypeScript is rejected", () => {
  const gd = structuredClone(hookGd);
  delete gd.hooks.onX;
  gd.hooks["y: (...a: any[]) => any; [k: string]: any; z"] = hookGd.hooks.onX;
  assert.ok(validatePluginGamedata(gd, { permissions: HOOK_PERMS }).some((e) => e.includes("plain identifier")));
});

test("duplicate argNames are rejected", () => {
  const gd = structuredClone(sigCall);
  gd.calls.foo.args = ["float", "float"];
  gd.calls.foo.argNames = ["a", "a"];
  assert.ok(validatePluginGamedata(gd, { permissions: PERMS }).some((e) => e.includes("duplicate argName")));
});
