// Unit tests for scripts/check-custom-hud-layout.mjs: the checker must catch what the live
// CustomHud validator rejects, or the CI gate proves nothing.
import test from "node:test";
import assert from "node:assert/strict";
import { checkLayout } from "./check-custom-hud-layout.mjs";

const doc = (body) => `<root>\n<styles>\n<include src="file://{resources}/styles/custom_game/x.css" />\n</styles>\n<Panel class="r">\n${body}\n</Panel>\n</root>\n`;

test("a layout of only allowed markup passes", () => {
  assert.deepEqual(checkLayout(doc(`<Panel id="a" class="b" hittest="false"><Label id="l" class="c" text="{s:l}" /><Image id="i" class="d" src="{s:i}" /><Button id="b" class="e"><Label class="f" text="Go" /></Button></Panel>`)), []);
});

test("Image texture sizes and scaling are rejected (the 2026-11-22 live failure)", () => {
  const p = checkLayout(doc(`<Image id="i" class="d" src="{s:i}" textureheight="16" texturewidth="-1" scaling="stretch-to-fit-preserve-aspect" />`));
  assert.equal(p.length, 3);
  assert.match(p[0], /disallowed attribute textureheight for panel type 'Image'/);
});

test("hittest on a Button, style anywhere and unknown tags are rejected", () => {
  assert.match(checkLayout(doc(`<Button id="b" class="e" hittest="false" />`))[0], /hittest for panel type 'Button'/);
  assert.match(checkLayout(doc(`<Panel class="a" style="width: 10px;" />`))[0], /style/);
  assert.match(checkLayout(doc(`<TextEntry id="t" />`))[0], /disallowed panel type or node <TextEntry>/);
  assert.match(checkLayout(doc(`<scripts><include src="file://{resources}/x.js" /></scripts>`))[0], /<scripts>/);
});

test("the outer panel may not carry an id, and includes must be stylesheets", () => {
  assert.match(checkLayout(`<root><Panel id="x" class="r" /></root>`)[0], /outer panel/);
  assert.match(checkLayout(`<root><styles><include src="s2r://a/b.vxml_c" /></styles><Panel class="r" /></root>`)[0], /non-stylesheet/);
});

test("commented-out markup is ignored", () => {
  assert.deepEqual(checkLayout(doc(`<!-- <Image textureheight="1" /> -->`)), []);
});
