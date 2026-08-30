/**
 * The @s2script/zones contract must match the current inter-plugin authoring
 * surface (ledgered `on`, no `off`, producer-as-import named exports) and the
 * cookbook's verified copy must stay a byte-copy of the producer file.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const producer = join(root, "plugins/zones/api.d.ts");
const cookbookCopy = join(root, "examples/cookbook/.s2script/types/@s2script/zones/index.d.ts");

test("zones contract: on is ledgered (void, no off)", () => {
  const dts = readFileSync(producer, "utf8");
  assert.match(
    dts,
    /export declare function on\(event: "enter" \| "leave" \| "stay", handler: \(p: ZoneEvent\) => void\): void;/,
  );
  assert.match(
    dts,
    /export declare function on\(event: "created", handler: \(p: ZoneCreatedEvent\) => void\): void;/,
  );
  assert.match(
    dts,
    /export declare function on\(event: "deleted", handler: \(p: ZoneDeletedEvent\) => void\): void;/,
  );
  assert.doesNotMatch(dts, /export declare function off\(/);
});

test("cookbook zones contract is a byte-copy of plugins/zones/api.d.ts", () => {
  const a = readFileSync(producer);
  const b = readFileSync(cookbookCopy);
  assert.deepEqual(a, b);
});
