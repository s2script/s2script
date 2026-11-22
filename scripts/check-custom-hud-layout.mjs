#!/usr/bin/env node
// Fail CI when a workshop layout uses markup CS2's CustomHud validator rejects.
//
// A custom_hud_layout file goes through a whitelist validator on the client: four panel types,
// a fixed attribute list per type, no script/snippet nodes, includes of stylesheets only. One
// disallowed attribute anywhere fails the whole file, the panel is never created and the player
// gets a blank HUD with one console line ("[custom_hud] Layout contains disallowed attribute ...").
// The resource compiler accepts all of it, so nothing short of a client load catches it, and a
// Workshop publish ships it silently.
//
// The table is the one the client binary builds, read from client.dll of CS2 build 11259529
// (PatchVersion 1.42.0.0, 2026-11-19): the whitelist initialiser adds each tag's attribute
// strings, then the tag. Reverse-engineered notes from an older build listed Image
// texturewidth/textureheight; this build dropped both (and added `languages`), and the live
// client rejected textureheight on 2026-11-22. Re-derive it from the binary when it changes.
//
//   node scripts/check-custom-hud-layout.mjs [dir...]   default: every panorama/layout under examples/
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "..");

export const ALLOWED = {
  Panel: ["id", "class", "hittest"],
  Label: ["id", "class", "hittest", "text"],
  Image: ["id", "class", "hittest", "src", "languages"],
  Button: ["id", "class"],
};
// Structural nodes the validator accepts, and the attributes each may carry.
const STRUCTURAL = { root: [], styles: [], include: ["src"] };
const STYLESHEET = /\.(v?css)(_c)?$/;

function walk(dir, out) {
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name.startsWith(".")) continue;
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) walk(p, out);
    else if (/\.xml$/.test(name) && /[\\/]panorama[\\/]layout[\\/]/.test(p)) out.push(p);
  }
  return out;
}

/** Problems in one layout's text, as strings. */
export function checkLayout(text) {
  const problems = [];
  const src = text.replace(/<!--[\s\S]*?-->/g, (c) => c.replace(/[^\n]/g, " "));
  const lineOf = (i) => src.slice(0, i).split("\n").length;
  const tagRe = /<\s*(\/?)\s*([A-Za-z_][\w.-]*)([^>]*?)(\/?)\s*>/g;
  let depthPanels = 0;
  let firstPanel = true;
  let m;
  while ((m = tagRe.exec(src))) {
    const [, closing, tag, rest] = m;
    const line = lineOf(m.index);
    if (closing) {
      if (ALLOWED[tag]) depthPanels--;
      continue;
    }
    const attrs = [];
    const attrRe = /([A-Za-z_:][\w:.-]*)\s*=\s*("([^"]*)"|'([^']*)')/g;
    let a;
    const leftover = rest.replace(attrRe, (all, name, _q, v1, v2) => { attrs.push([name, v1 ?? v2]); return ""; }).replace(/\//g, "").trim();
    if (leftover) problems.push(`line ${line}: <${tag}> has unparsed markup "${leftover}"`);
    const allowed = ALLOWED[tag] || STRUCTURAL[tag];
    if (!allowed) {
      problems.push(`line ${line}: disallowed panel type or node <${tag}>`);
      continue;
    }
    for (const [name, value] of attrs) {
      if (!allowed.includes(name)) problems.push(`line ${line}: disallowed attribute ${name} for panel type '${tag}'`);
      if (tag === "include" && name === "src" && !STYLESHEET.test(value)) problems.push(`line ${line}: <include> of a non-stylesheet resource "${value}"`);
    }
    if (ALLOWED[tag]) {
      if (firstPanel && depthPanels === 0 && attrs.some(([n]) => n === "id")) problems.push(`line ${line}: the outer panel may not carry an id`);
      firstPanel = false;
      if (!m[4]) depthPanels++;
    }
  }
  return problems;
}

const isMain = process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1];
if (isMain) {
  const dirs = process.argv.slice(2);
  const files = (dirs.length ? dirs : [join(ROOT, "examples")]).flatMap((d) => walk(d, []));
  let bad = 0;
  for (const f of files) {
    const problems = checkLayout(readFileSync(f, "utf8"));
    for (const p of problems) console.error(`${relative(ROOT, f)}: ${p}`);
    if (problems.length) bad++;
  }
  if (!files.length) { console.error("check-custom-hud-layout: no layouts found"); process.exit(1); }
  if (bad) { console.error(`FAIL: ${bad} of ${files.length} layout(s) would fail CustomHud validation`); process.exit(1); }
  console.log(`PASS: ${files.length} layout(s) use only CustomHud-allowed markup`);
}
