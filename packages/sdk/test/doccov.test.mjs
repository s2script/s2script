import { test } from "node:test";
import assert from "node:assert/strict";
import { analyzeSource } from "../src/doccov/doccov.ts";

const names = (gaps) => gaps.map((g) => g.symbol).sort();

test("flags an undocumented interface member but not a documented one", () => {
  const src = [
    "/** @s2script/x — banner. */",
    "",
    "/** A foo. */",
    "export interface Foo {",
    "  /** the a. */",
    "  a: number;",
    "  b: string;",
    "}",
  ].join("\n");
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), ["b"]);
});

test("the file banner does NOT count as the first symbol's own doc", () => {
  const src = [
    "/** @s2script/x — banner. */",
    "export interface Bar {",
    "  /** x. */ x: number;",
    "}",
  ].join("\n");
  // Bar is flagged (only doc above it is the banner); x is documented.
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), ["Bar"]);
});

test("walks the members of an exported const object type", () => {
  const src = [
    "/** banner */",
    "",
    "/** The API. */",
    "export declare const Api: {",
    "  /** does a. */",
    "  a(): void;",
    "  b(): void;",
    "};",
  ].join("\n");
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), ["b"]);
});

test("re-exports and imports are not flagged", () => {
  const src = [
    "/** banner */",
    "",
    'import type { Client } from "./clients";',
    'export * from "./schema.generated";',
    "/** A row. */",
    "export type Row = Record<string, string>;",
  ].join("\n");
  assert.deepEqual(names(analyzeSource("x.d.ts", src)), []);
});
