import { test } from "node:test";
import assert from "node:assert/strict";
import { extractModule, cleanBanner } from "../src/docmodel/docmodel.ts";

test("extracts a function's signature, summary, params, returns and example", () => {
  const src = [
    "/** @s2script/timers — author-time type stubs for the async timing API. NO runtime code (injected at load). */",
    "",
    "/**",
    " * Await a delay of `ms` ms before continuing.",
    " * @param ms - milliseconds to wait",
    " * @returns resolves on a later game frame",
    " * @example",
    ' * import { delay } from "@s2script/sdk/timers";',
    " * await delay(1000);",
    " */",
    "export declare function delay(ms: number): Promise<void>;",
  ].join("\n");
  const mod = extractModule("timers.d.ts", src);
  assert.equal(mod.banner, "author-time type stubs for the async timing API."); // @pkg prefix + "NO runtime code…" cut
  assert.equal(mod.exports.length, 1);
  const d = mod.exports[0];
  assert.equal(d.name, "delay");
  assert.equal(d.kind, "function");
  assert.equal(d.signature, "delay(ms: number): Promise<void>");
  assert.match(d.summary, /Await a delay/);
  assert.deepEqual(d.params, [{ name: "ms", text: "milliseconds to wait" }]);
  assert.equal(d.returns, "resolves on a later game frame");
  assert.equal(d.examples.length, 1);
  assert.match(d.examples[0], /await delay\(1000\)/);
});

test("cleanBanner strips the @pkg prefix and boilerplate", () => {
  const cleaned = cleanBanner(
    "@s2script/timers — author-time type stubs for the async timing API. NO runtime code (injected at load).",
  );
  assert.equal(cleaned, "author-time type stubs for the async timing API.");
  const cleaned2 = cleanBanner("@s2script/chat — print messages to player chat. NO runtime code (injected at load).");
  assert.equal(cleaned2, "print messages to player chat.");
});

test("extracts const-object members with their own signatures and docs", () => {
  const src = [
    "/** banner */",
    "",
    "/**",
    " * Print messages to player chat.",
    " * @example",
    ' * Chat.toAll("hi");',
    " */",
    "export declare const Chat: {",
    "  /** The color prefix. */",
    "  color: string;",
    "  /** Print to one slot. */",
    "  toSlot(slot: number, message: string): void;",
    "  toAll(message: string): void;",
    "};",
  ].join("\n");
  const mod = extractModule("chat.d.ts", src);
  assert.equal(mod.exports.length, 1);
  const chat = mod.exports[0];
  assert.equal(chat.name, "Chat");
  assert.equal(chat.kind, "const");
  assert.match(chat.summary, /Print messages to player chat/);
  assert.equal(chat.examples.length, 1);
  const names = chat.members.map((m) => m.name);
  assert.deepEqual(names, ["color", "toSlot", "toAll"]);
  const toSlot = chat.members.find((m) => m.name === "toSlot");
  assert.equal(toSlot.kind, "method");
  assert.equal(toSlot.signature, "toSlot(slot: number, message: string): void");
  assert.equal(toSlot.summary, "Print to one slot.");
  const color = chat.members.find((m) => m.name === "color");
  assert.equal(color.kind, "property");
  assert.equal(color.signature, "color: string");
});

test("flattens {@link X} references in summaries to plain text", () => {
  const src = [
    "/** banner */",
    "",
    "/** Backed by an {@link EntityRef} handle. */",
    "export interface Pawn {",
    "  /** the health. */ health: number;",
    "}",
  ].join("\n");
  const mod = extractModule("x.d.ts", src);
  assert.match(mod.exports[0].summary, /Backed by an EntityRef handle/);
});

test("imports and re-exports produce no exported symbols", () => {
  const src = [
    "/** banner */",
    'import type { Client } from "./clients";',
    'export * from "./schema.generated";',
    "/** A row. */",
    "export type Row = Record<string, string>;",
  ].join("\n");
  const mod = extractModule("x.d.ts", src);
  assert.deepEqual(mod.exports.map((e) => e.name), ["Row"]);
});
