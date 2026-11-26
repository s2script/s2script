import { test } from "node:test";
import assert from "node:assert/strict";
import { parseLayout, parseCss, resolveStyles, layoutTree, boxes, applyScenario } from "../src/engine.mjs";

// Fake text: every character is half the font size wide, one line, 1.2 line height.
const measure = (text, s) => { const f = parseFloat(s["font-size"] ?? "18"); return { w: text.length * f * 0.5, h: f * 1.2, lines: [text] }; };
const run = (xml, css, scenario) => {
  const root = parseLayout(`<root><Panel class="root">${xml}</Panel></root>`);
  const state = applyScenario(root, scenario);
  resolveStyles(root, parseCss(`.root { width: 100%; height: 100%; } ${css}`), state);
  layoutTree(root, { width: 1000, height: 500, measure, state });
  return Object.fromEntries(boxes(root).filter((b) => b.id).map((b) => [b.id, [b.x, b.y, b.w, b.h]]));
};

test("flow right places children one after another, margins included", () => {
  const b = run(`<Panel id="row" class="row"><Panel id="a" class="box" /><Panel id="b" class="box gap" /></Panel>`,
    `.row { flow-children: right; } .box { width: 40px; height: 20px; } .gap { margin-left: 10px; }`);
  assert.deepEqual(b.row, [0, 0, 90, 20]);
  assert.deepEqual(b.a, [0, 0, 40, 20]);
  assert.deepEqual(b.b, [50, 0, 40, 20]);
});

test("fill-parent-flow splits what the fixed siblings leave, by weight", () => {
  const b = run(`<Panel id="row" class="row"><Panel id="a" class="fix" /><Panel id="b" class="f1" /><Panel id="c" class="f3" /></Panel>`,
    `.row { width: 500px; height: 10px; flow-children: right; } .fix { width: 100px; height: 10px; } .f1 { width: fill-parent-flow(1.0); height: 10px; } .f3 { width: fill-parent-flow(3.0); height: 10px; }`);
  assert.deepEqual(b.b, [100, 0, 100, 10]);
  assert.deepEqual(b.c, [200, 0, 300, 10]);
});

test("padding and border are inside the width", () => {
  const b = run(`<Panel id="p" class="p"><Panel id="c" class="c" /></Panel>`,
    `.p { width: 200px; height: 100px; padding: 10px 20px; border: 2px solid #ffffff; } .c { width: 100%; height: 100%; }`);
  assert.deepEqual(b.p, [0, 0, 200, 100]);
  assert.deepEqual(b.c, [22, 12, 156, 76]);
});

test("alignment without flow: centre and far edge honour margins", () => {
  const b = run(`<Panel id="c" class="box mid" /><Panel id="r" class="box far" />`,
    `.box { width: 100px; height: 50px; } .mid { horizontal-align: center; vertical-align: center; } .far { horizontal-align: right; vertical-align: bottom; margin-right: 20px; margin-bottom: 10px; }`);
  assert.deepEqual(b.c, [450, 225, 100, 50]);
  assert.deepEqual(b.r, [880, 440, 100, 50]);
});

test("a collapsed panel takes no space, and the scenario can show it", () => {
  const xml = `<Panel id="col" class="col"><Panel id="a" class="box s2-hide" /><Panel id="b" class="box" /></Panel>`;
  const css = `.col { flow-children: down; } .box { width: 10px; height: 10px; } .s2-hide { visibility: collapse; }`;
  assert.deepEqual(run(xml, css).b, [0, 0, 10, 10]);
  assert.equal(run(xml, css).a, undefined);
  assert.deepEqual(run(xml, css, { show: ["a"] }).b, [0, 10, 10, 10]);
});

test("a Label fits its text, with dialog variables and text-transform applied", () => {
  const b = run(`<Label id="t" class="t" text="{s:t}" />`, `.t { font-size: 20px; padding: 0px 5px; text-transform: uppercase; }`, { vars: { t: "abcd" } });
  assert.deepEqual(b.t, [0, 0, 50, 24]);
});

test("descendant selectors and specificity decide the winning rule", () => {
  const b = run(`<Panel id="th" class="theme"><Panel id="x" class="box" /></Panel>`,
    `.theme .box { width: 30px; } .box { width: 10px; height: 10px; }`);
  assert.equal(b.x[2], 30);
});

test("the real in-round HUD lays out on screen", async () => {
  const { readFileSync } = await import("node:fs");
  const at = (p) => new URL(p, import.meta.url);
  const sc = JSON.parse(readFileSync(at("../scenarios/gamehud.json"), "utf8"));
  const root = parseLayout(readFileSync(at("../scenarios/" + sc.layout), "utf8"));
  const state = applyScenario(root, sc);
  resolveStyles(root, parseCss(readFileSync(at("../scenarios/" + sc.stylesheet), "utf8")), state);
  layoutTree(root, { width: 1920, height: 1080, measure, state });
  const b = Object.fromEntries(boxes(root).filter((x) => x.id).map((x) => [x.id, x]));
  // The rail is a full-width bar on the top edge; the status card sits 18px in from bottom left.
  assert.deepEqual([b.gk_rail.x, b.gk_rail.y, b.gk_rail.w, b.gk_rail.h], [0, 0, 1920, 116]);
  assert.deepEqual([b.gk_status.x, b.gk_status.w, b.gk_status.y + b.gk_status.h], [18, 300, 1062]);
  assert.equal(b.gk_wpn.x + b.gk_wpn.w, 1902);
  assert.ok(Math.abs(b.gk_bar0_fill.w / b.gk_bar0_fill.node.parent.w - 0.7) < 1e-9);
  assert.equal(b.gk_sum, undefined);
  for (const x of Object.values(b)) assert.ok(x.x >= 0 && x.y >= 0 && x.x + x.w <= 1920.01 && x.y + x.h <= 1080.01, x.id);
});
