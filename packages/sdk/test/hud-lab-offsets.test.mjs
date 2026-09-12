import { test } from "node:test";
import assert from "node:assert/strict";

const layout = { m_strLayout: 1928, m_vecPlayerLayoutStates: 1944, m_globalLayoutState: 2048,
  m_vecPanelIds: 2456, m_vecClassNames: 2480, m_vecDialogVariableNames: 2504 };
const state = { m_playerSlot: 48, m_bInputCaptureEnabled: 52, m_vecHasClasses: 56, m_vecDialogVariableStrings: 152 };
const moduleUrl = new URL("../../../examples/hud-lab/src/offsets.ts", import.meta.url);

test("HUD diagnostics follow shifted live-schema fields and retain the verified state stride", async () => {
  try {
    for (const shift of [0, 64]) {
      globalThis.__s2_schema_offset = (cls, field) => cls === "CCSCustomHudLayout" ? layout[field] + shift : state[field];
      const offsets = await import(`${moduleUrl}?shift=${shift}`);
      assert.equal(offsets.LAYOUT.vecPlayerLayoutStates, 1944 + shift);
      assert.equal(offsets.globalStateField(offsets.STATE.inputCaptureEnabled), 2100 + shift);
      assert.equal(offsets.STATE.playerSlot, 48);
      assert.equal(offsets.STATE_SIZE, 408);
    }
  } finally { delete globalThis.__s2_schema_offset; }
});

test("HUD diagnostics reject missing schema before exposing writable offsets", async () => {
  globalThis.__s2_schema_offset = () => -1;
  try { await assert.rejects(import(`${moduleUrl}?missing`), /HUD schema unavailable/); }
  finally { delete globalThis.__s2_schema_offset; }
});
