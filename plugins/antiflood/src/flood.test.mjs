import test from "node:test";
import assert from "node:assert";
import { floodStep } from "./flood.ts";

const FT = 0.75, MAX = 3;
const near = (a, b) => Math.abs(a - b) < 1e-6;

test("disabled: floodTime<=0 never blocks and leaves state unchanged", () => {
  const r = floodStep({ tokens: 5, lastTime: 100 }, 200, 0, MAX);
  assert.strictEqual(r.block, false);
  assert.strictEqual(r.tokens, 5);
  assert.strictEqual(r.lastTime, 100);
});

test("burst: rapid messages fill the bucket; blocks once the level exceeds maxTokens", () => {
  let s = { tokens: 0, lastTime: 0 };
  let r = floodStep(s, 100.0, FT, MAX); assert.strictEqual(r.block, false); s = r; // level ~1
  r = floodStep(s, 100.1, FT, MAX); assert.strictEqual(r.block, false); s = r;      // ~1.87
  r = floodStep(s, 100.2, FT, MAX); assert.strictEqual(r.block, false); s = r;      // ~2.73
  r = floodStep(s, 100.3, FT, MAX); assert.strictEqual(r.block, true);              // ~3.6 -> block
  assert.strictEqual(r.tokens, MAX);                                                // capped at maxTokens
});

test("spaced: messages farther apart than floodTime never accumulate, never block", () => {
  let s = { tokens: 0, lastTime: 0 };
  for (let i = 1; i <= 6; i++) {
    const r = floodStep(s, i * 1.0, FT, MAX); // 1s apart > floodTime => fully leaked each time
    assert.strictEqual(r.block, false);
    assert.ok(near(r.tokens, 1));
    s = r;
  }
});

test("recovery: after blocking, a ~1s pause drains the bucket and the next message passes", () => {
  const r = floodStep({ tokens: 3, lastTime: 100 }, 101.0, FT, MAX); // 1s later
  assert.strictEqual(r.block, false);
  assert.ok(r.tokens < MAX);
});

test("cap: a sustained blocked spam holds the bucket at maxTokens (no unbounded growth)", () => {
  let s = { tokens: 3, lastTime: 100 };
  for (let i = 1; i <= 5; i++) {
    const r = floodStep(s, 100 + i * 0.1, FT, MAX); // fast, still blocking
    assert.strictEqual(r.block, true);
    assert.strictEqual(r.tokens, MAX);
    s = r;
  }
});
