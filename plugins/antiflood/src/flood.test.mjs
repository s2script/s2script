import test from "node:test";
import assert from "node:assert";
import { floodStep } from "./flood.ts";

const FT = 0.75, MAX = 3;

test("disabled: floodTime<=0 never blocks and leaves state unchanged", () => {
  const r = floodStep({ tokens: 5, lastTime: 100 }, 200, 0, MAX);
  assert.strictEqual(r.block, false);
  assert.strictEqual(r.tokens, 5);
  assert.strictEqual(r.lastTime, 100);
});

test("burst: rapid messages accrue tokens; the maxTokens-th blocks", () => {
  let s = { tokens: 0, lastTime: 0 };
  let r = floodStep(s, 10.0, FT, MAX); // spaced from 0 -> decay stays 0
  assert.strictEqual(r.tokens, 0); assert.strictEqual(r.block, false); s = r;
  r = floodStep(s, 10.1, FT, MAX); assert.strictEqual(r.tokens, 1); assert.strictEqual(r.block, false); s = r;
  r = floodStep(s, 10.2, FT, MAX); assert.strictEqual(r.tokens, 2); assert.strictEqual(r.block, false); s = r;
  r = floodStep(s, 10.3, FT, MAX); assert.strictEqual(r.tokens, 3); assert.strictEqual(r.block, true);
});

test("spaced: well-separated messages never accrue, never block", () => {
  let s = { tokens: 0, lastTime: 0 };
  for (let i = 1; i <= 5; i++) {
    const r = floodStep(s, i * 2.0, FT, MAX); // 2s apart >> floodTime
    assert.strictEqual(r.block, false);
    assert.strictEqual(r.tokens, 0);
    s = r;
  }
});

test("recovery: after the threshold, spaced messages decay tokens back down", () => {
  let s = { tokens: 3, lastTime: 10 };
  let r = floodStep(s, 12.0, FT, MAX); assert.strictEqual(r.tokens, 2); assert.strictEqual(r.block, false); s = r;
  r = floodStep(s, 14.0, FT, MAX); assert.strictEqual(r.tokens, 1); assert.strictEqual(r.block, false);
});

test("boundary: exactly maxTokens blocks; one below does not", () => {
  assert.strictEqual(floodStep({ tokens: 2, lastTime: 10 }, 10.1, FT, MAX).block, true);
  assert.strictEqual(floodStep({ tokens: 1, lastTime: 10 }, 10.1, FT, MAX).block, false);
});
