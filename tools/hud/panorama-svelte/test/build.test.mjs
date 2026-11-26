import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "../src/build.mjs";
import { checkCss } from "../src/css.mjs";
// The gate CI runs over committed layouts must agree with what this builds.
import { ALLOWED } from "../src/rules.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const fx = (n) => join(HERE, "fixtures", n);

test("the example builds to whitelist-only markup", async () => {
  const r = await build(join(HERE, "../example/Hud.svelte"));
  assert.equal(r.manifest.ids.length, 35);
  assert.deepEqual(r.manifest.buttons, ["gk_close"]);
  for (const m of r.xml.matchAll(/<(Panel|Label|Image|Button)((?:\s+[\w-]+="[^"]*")*)/g)) {
    for (const a of m[2].matchAll(/([\w-]+)=/g)) assert.ok(ALLOWED[m[1]].includes(a[1]), `${m[1]} ${a[1]}`);
  }
  assert.match(r.xml, /<Label id="gk_kf5_v" class="name" hittest="false" text="\{s:gk_kf5_v\}" \/>/);
  assert.doesNotMatch(r.xml, /<(?!\/?(root|styles|include|Panel|Label|Image|Button)\b)/);
});

test("scoped styles come out without :where()", async () => {
  const r = await build(fx("Scoped.svelte"));
  assert.match(r.css, /\.root\.svelte-[\w-]+ \.t\.svelte-[\w-]+ \{/);
  assert.doesNotMatch(r.css, /:where\(/);
  assert.match(checkCss(".a :where(.b) { color: #ffffff; }")[0], /syntax Panorama ignores/);
});

const rejects = [
  ["BadAttr.svelte", /disallowed attribute textureheight for panel type 'Image'/],
  ["BadTag.svelte", /<div> is not a CustomHud panel type/],
  ["BadEvent.svelte", /no client events/],
  ["OuterId.svelte", /outer panel may not carry an id/],
  ["DupId.svelte", /duplicate id a/],
  ["BadVar.svelte", /dialog variable \{s:undefined\} must be named after its panel id/],
  ["BadCss.svelte", /property "display" does not exist[\s\S]*rgba/],
];
for (const [file, re] of rejects) {
  test(`${file} fails the build`, async () => {
    await assert.rejects(build(fx(file)), re);
  });
}

test("css check accepts Panorama's own vocabulary", () => {
  assert.deepEqual(checkCss(".a { flow-children: right; width: fill-parent-flow( 1.0 ); background-color: gradient( linear, 0% 0%, 0% 100%, from( #000000 ), to( #ffffff ) ); }\n@keyframes k { from { opacity: 0; } to { opacity: 1; } }"), []);
});

test("an attribute off the whitelist on a kit component is a type error, and never reaches the XML", async () => {
  const r = await build(fx("BadProp.svelte"));
  assert.doesNotMatch(r.xml, /textureheight/);
  const run = spawnSync("npx", ["svelte-check", "--tsconfig", join(HERE, "tsconfig.badprop.json"), "--diagnostic-sources", "js,svelte"], { cwd: join(HERE, ".."), encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.match(run.stdout, /BadProp\.svelte[\s\S]*textureheight[\s\S]*does not exist/);
});
