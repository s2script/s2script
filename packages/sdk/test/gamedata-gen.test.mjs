import { test } from "node:test";
import assert from "node:assert/strict";
import { generateGamedataTypes } from "../src/gamedata/gen-types.ts";

const gd = {
  calls: {
    ignite: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
              args: ["float", "bool"], returns: "void" },
    lookupAttachment: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
              args: ["string"], returns: "int" },
    dropWeapon: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
              args: ["entity"], returns: "entity" },
  },
};

test("imports EntityRef so the module augmentation resolves", () => {
  const out = generateGamedataTypes(gd);
  assert.match(out, /import type \{ EntityRef \} from "@s2script\/sdk\/entity";/);
});

test("augments @s2script/sdk/unsafe with an EngineCalls interface", () => {
  assert.match(generateGamedataTypes(gd), /declare module "@s2script\/sdk\/unsafe"\s*\{[\s\S]*interface EngineCalls/);
});

test("void return stays void; non-void returns are nullable", () => {
  const out = generateGamedataTypes(gd);
  assert.match(out, /ignite: \(self: EntityRef, a0: number, a1: boolean\) => void;/);
  assert.match(out, /lookupAttachment: \(self: EntityRef, a0: string\) => number \| null;/);
});

test("entity args are nullable EntityRef and entity returns are EntityRef | null", () => {
  assert.match(generateGamedataTypes(gd),
    /dropWeapon: \(self: EntityRef, a0: EntityRef \| null\) => EntityRef \| null;/);
});

test("a gamedata with no calls still emits a valid empty augmentation", () => {
  const out = generateGamedataTypes({});
  assert.match(out, /interface EngineCalls \{\s*\}/);
});

test("output is deterministic (calls sorted by name)", () => {
  assert.equal(generateGamedataTypes(gd), generateGamedataTypes(gd));
  const idxDrop = generateGamedataTypes(gd).indexOf("dropWeapon");
  const idxIgnite = generateGamedataTypes(gd).indexOf("ignite");
  assert.ok(idxDrop < idxIgnite, "expected alphabetical ordering");
});

// --- vector shape (added after review: this was inverted and had zero coverage) ---

test("vector args generate the structural {x,y,z} shape core actually reads, not a tuple", () => {
  const out = generateGamedataTypes({
    calls: { teleport: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
                         args: ["vector"], returns: "void" } },
  });
  assert.match(out, /teleport: \(self: EntityRef, a0: \{ readonly x: number; readonly y: number; readonly z: number \}\) => void;/);
  assert.doesNotMatch(out, /readonly \[number, number, number\]/,
    "a tuple type would reject a real Vector and silently send (0,0,0) for an array");
});

// --- argNames (Tier C: on an unsafe FFI surface the param names ARE the documentation) ---

test("argNames replace the positional aN parameter names", () => {
  const out = generateGamedataTypes({
    calls: { ignite: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
                       args: ["float", "int", "entity", "float"],
                       argNames: ["flFlameLifetime", "nFlags", "pAttacker", "flSize"],
                       returns: "void" } },
  });
  assert.match(out, /ignite: \(self: EntityRef, flFlameLifetime: number, nFlags: number, pAttacker: EntityRef \| null, flSize: number\) => void;/);
  assert.doesNotMatch(out, /\ba0:/);
});

test("omitting argNames still yields positional aN", () => {
  const out = generateGamedataTypes({
    calls: { f: { receiver: { kind: "entity" }, target: { kind: "signature", name: "S" },
                  args: ["int"], returns: "void" } },
  });
  assert.match(out, /f: \(self: EntityRef, a0: number\) => void;/);
});
