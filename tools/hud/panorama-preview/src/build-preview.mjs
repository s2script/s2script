#!/usr/bin/env node
// Write a self-contained preview page for one scenario: the engine, the layout, its stylesheet and
// the scenario's state, inlined into one HTML file that opens from disk.
//
//   node src/build-preview.mjs scenarios/gamehud.json out/gamehud.html
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { ASSUMPTIONS } from "./engine.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const [scenarioPath, outPath = "out/preview.html"] = process.argv.slice(2);
if (!scenarioPath) { console.error("usage: build-preview.mjs <scenario.json> [out.html]"); process.exit(2); }
const scenario = JSON.parse(readFileSync(scenarioPath, "utf8"));
const at = (p) => resolve(dirname(scenarioPath), p);
const data = { xml: readFileSync(at(scenario.layout), "utf8"), css: readFileSync(at(scenario.stylesheet), "utf8"), scenario };
const engine = readFileSync(join(HERE, "engine.mjs"), "utf8").replace(/^export /gm, "");
const paint = readFileSync(join(HERE, "paint.js"), "utf8");
const safe = (s) => s.replace(/<\/script/gi, "<\\/script");

const html = `<!doctype html>
<meta charset="utf-8">
<title>Panorama preview</title>
<style>
  html, body { margin: 0; background: #111417; color: #c9d1d9; font: 13px/1.4 system-ui, sans-serif; }
  #bar { padding: 8px 12px; display: flex; gap: 16px; align-items: center; }
  #wrap { overflow: hidden; }
  #stage { position: relative; width: 1920px; height: 1080px; transform-origin: 0 0; overflow: hidden;
    background: linear-gradient(#5b6b78, #2c3238 55%, #3b3a33); }
  body.shot #bar { display: none; }
</style>
<div id="bar">
  <strong>Panorama preview</strong>
  <label><input type="checkbox" id="outline"> outlines</label>
  <span id="info"></span>
</div>
<div id="wrap"><div id="stage"></div></div>
<script>
${safe(engine)}
${safe(paint)}
const DATA = ${safe(JSON.stringify(data))};
const stage = document.getElementById("stage");
const shot = new URLSearchParams(location.search).has("shot");
if (shot) document.body.classList.add("shot");
function fit() {
  const k = shot ? 1 : Math.min(1, window.innerWidth / 1920);
  stage.style.transform = "scale(" + k + ")";
  document.getElementById("wrap").style.height = 1080 * k + "px";
}
function draw() {
  const root = render(DATA, stage, { outline: document.getElementById("outline").checked });
  window.__boxes = boxes(root).filter((b) => b.id).map((b) => ({ id: b.id, x: b.x, y: b.y, w: b.w, h: b.h }));
  document.getElementById("info").textContent = window.__boxes.length + " visible panels with ids, ${ASSUMPTIONS.length} layout rules still unverified against the game";
}
document.getElementById("outline").onchange = draw;
window.onresize = fit;
(document.fonts ? document.fonts.ready : Promise.resolve()).then(() => { fit(); draw(); window.__ready = true; });
</script>
`;
mkdirSync(dirname(outPath), { recursive: true });
writeFileSync(outPath, html);
console.log(`wrote ${outPath}`);
