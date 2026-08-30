import { test } from "node:test";
import assert from "node:assert";
import { buildHookModel, buildPluginHookList } from "../src/hookgen/model.ts";
import { emitHookDts, emitPluginHookDts } from "../src/hookgen/emit-dts.ts";

const GD = {
  onTerminateRound: {
    target: { kind: "signature", name: "CCSGameRules_TerminateRound" },
    shape: "this_f32_i32_i32_i32",
    params: ["delay", "reason", "_unused3", "_unused4"],
    mutable: ["delay", "reason"],
    bypassWith: "terminateRound",
    expose: { ctx: "gameRules" },
  },
  onRespawn: {
    target: { kind: "signature", name: "CCSPlayerController_Respawn" },
    shape: "this_void",
    receiver: { kind: "entity", as: "player" },
    bypassWith: "respawn",
    expose: { ctx: "players" },
  },
};

test("buildHookModel groups hooks by expose.ctx, sorted, with per-param mutability", () => {
  const m = buildHookModel(GD);
  assert.deepEqual(m.map((ns) => ns.ns), ["gameRules", "players"], "namespaces sorted alphabetically");

  const gameRules = m.find((ns) => ns.ns === "gameRules");
  assert.equal(gameRules.ctxIface, "CtxGameRules");
  assert.equal(gameRules.hooks.length, 1);
  const onTerminateRound = gameRules.hooks[0];
  assert.equal(onTerminateRound.name, "onTerminateRound");
  assert.equal(onTerminateRound.viewIface, "OnTerminateRoundView");
  assert.deepEqual(onTerminateRound.params, [
    { name: "delay", mutable: true },
    { name: "reason", mutable: true },
    { name: "_unused3", mutable: false },
    { name: "_unused4", mutable: false },
  ]);
  assert.equal(onTerminateRound.receiverAs, null, "receiver.kind defaults to none -> nothing surfaced");

  const players = m.find((ns) => ns.ns === "players");
  assert.equal(players.ctxIface, "CtxPlayers");
  const onRespawn = players.hooks[0];
  assert.equal(onRespawn.viewIface, "OnRespawnView");
  assert.deepEqual(onRespawn.params, [], "this_void carries no params");
  assert.equal(onRespawn.receiverAs, "player");
});

test("buildHookModel skips a handwritten hook (first-class view lives in items.d.ts)", () => {
  const gd = {
    onCanAcquire: {
      shape: "this_i64_i32_i64",
      params: ["method", "result"],
      mutable: ["result"],
      expose: { ctx: "items", handwritten: true },
    },
    onTerminateRound: {
      shape: "this_f32_i32_i32_i32",
      params: ["delay", "reason"],
      expose: { ctx: "gameRules" },
    },
  };
  const m = buildHookModel(gd);
  assert.deepEqual(m.map((ns) => ns.ns), ["gameRules"], "handwritten items namespace is omitted");
  assert.equal(m[0].hooks.length, 1);
  assert.equal(m[0].hooks[0].name, "onTerminateRound");
});

test("buildHookModel skips a hook with no expose.ctx (nothing could subscribe to it)", () => {
  const gd = { onOrphan: { target: { kind: "signature", name: "X" }, shape: "this_void" } };
  const m = buildHookModel(gd);
  assert.deepEqual(m, [], "an unexposed hook generates no ctx member and no view");
});

test("buildHookModel groups multiple hooks under the SAME ctx namespace, hooks sorted by name", () => {
  const gd = {
    onB: { shape: "this_void", expose: { ctx: "shared" } },
    onA: { shape: "this_void", expose: { ctx: "shared" } },
  };
  const m = buildHookModel(gd);
  assert.equal(m.length, 1);
  assert.deepEqual(m[0].hooks.map((h) => h.name), ["onA", "onB"], "hook names sorted within a namespace");
});

test("buildHookModel only surfaces a receiver for kind:'entity' (kind:'none' or absent surfaces nothing)", () => {
  const gd = {
    onNone: { shape: "this_void", receiver: { kind: "none" }, expose: { ctx: "g" } },
    onAbsent: { shape: "this_void", expose: { ctx: "g" } },
  };
  const m = buildHookModel(gd);
  const [g] = m;
  for (const h of g.hooks) assert.equal(h.receiverAs, null);
});

test("emitHookDts emits per-hook view interfaces with mutable params plain and the rest readonly", () => {
  const dts = emitHookDts(buildHookModel(GD));
  assert.match(dts, /export interface OnTerminateRoundView \{/);
  assert.match(dts, /^ {2}delay: number;$/m, "a mutable param is plain (writable)");
  assert.match(dts, /^ {2}reason: number;$/m);
  assert.match(dts, /^ {2}readonly _unused3: number;$/m, "a non-mutable param is readonly");
  assert.match(dts, /^ {2}readonly _unused4: number;$/m);
});

test("emitHookDts surfaces a books-gated EntityRef receiver as readonly, typed EntityRef | null", () => {
  const dts = emitHookDts(buildHookModel(GD));
  assert.match(dts, /export interface OnRespawnView \{\n {2}readonly player: EntityRef \| null;\n\}/);
});

test("emitHookDts emits one ctx interface per namespace + the PluginContext augmentation", () => {
  const dts = emitHookDts(buildHookModel(GD));
  assert.match(dts, /export interface CtxGameRules \{\n {2}onTerminateRound\(handler: \(view: OnTerminateRoundView\) => HookResultValue \| void\): void;\n\}\n\nexport declare const gameRules: CtxGameRules;/);
  assert.match(dts, /export interface CtxPlayers \{\n {2}onRespawn\(handler: \(view: OnRespawnView\) => HookResultValue \| void\): void;\n\}\n\nexport declare const players: CtxPlayers;/);
  assert.match(dts, /declare module "@s2script\/sdk\/plugin" \{\n {2}interface PluginContext \{\n {4}readonly gameRules: CtxGameRules;\n {4}readonly players: CtxPlayers;\n {2}\}\n\}/);
});

test("emitHookDts imports HookResultValue and EntityRef, and is deterministic", () => {
  const model = buildHookModel(GD);
  const a = emitHookDts(model);
  const b = emitHookDts(buildHookModel(GD));
  assert.equal(a, b, "same input -> byte-identical output");
  assert.match(a, /^import type \{ HookResultValue \} from "@s2script\/sdk\/events";$/m);
  assert.match(a, /^import type \{ EntityRef \} from "@s2script\/sdk\/entity";$/m);
  assert.match(a, /^\/\/ GENERATED by `s2script gen-hooks`.*DO NOT EDIT\.$/m);
});

test("emitHookDts on an empty model still emits a syntactically-closed (empty) PluginContext augmentation", () => {
  const dts = emitHookDts(buildHookModel({}));
  assert.match(dts, /declare module "@s2script\/sdk\/plugin" \{\n {2}interface PluginContext \{\n {2}\}\n\}/);
});

test("buildPluginHookList is flat, sorted, and does not skip an unexposed hook", () => {
  const list = buildPluginHookList({
    onB: { shape: "this_void", expose: { ctx: "x" } },
    onA: { shape: "this_void" },
  });
  assert.deepEqual(list.map((h) => h.name), ["onA", "onB"]);
});

test("emitPluginHookDts augments EngineHooks, not PluginContext", () => {
  const dts = emitPluginHookDts(GD);
  assert.match(dts, /declare module "@s2script\/sdk\/unsafe"/);
  assert.match(dts, /interface EngineHooks/);
  assert.match(dts, /onTerminateRound: \(handler: \(view: OnTerminateRoundView\) => HookResultValue \| void\) => void;/);
  assert.doesNotMatch(dts, /interface PluginContext/,
    "plugin-declared hooks must not hang off ctx — expose.ctx colliding with a built-in would clobber it");
  assert.equal(emitPluginHookDts(GD), emitPluginHookDts(GD), "deterministic");
});
