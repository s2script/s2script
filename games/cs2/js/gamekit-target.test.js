// GameHud.target: the crosshair target label (gk_target in s2script_gamehud.xml).
const test = require("node:test");
const assert = require("node:assert/strict");
const { mount, cleanup } = require("./gamekit-fake.js");

test.afterEach(cleanup);

const FADE = "gk-fade";

function shown(m, slot) {
  return !m.hidden(slot, "gk_target") && !m.has(slot, "gk_target", FADE);
}

test("target is a region with one owner, freed on release", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  assert.ok(t);
  assert.equal(m.GameHud.target(), t, "same plugin gets the same claim back");
  assert.equal(m.claims["gamekit:target"], true);
  t.release();
  assert.equal(t.isValid(), false);
  assert.equal(m.claims["gamekit:target"], undefined);
  assert.equal(m.hidden(0, "gk_target"), true);
  assert.throws(() => t.set(0, { name: "Bob" }), /released/);
  assert.throws(() => t.hide(0), /released/);
  globalThis.__s2_ui_pool_claim("gamekit:target", 1);
  assert.equal(m.GameHud.target(), null, "another plugin's claim blocks this one");
});

test("set shows the name, un-collapsing while faded, then fades in", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(1, { name: "Bob" });
  assert.equal(m.text(1, "gk_target_n"), "Bob");
  assert.equal(m.has(1, "gk_target_n", "gk-t-default"), true);
  assert.equal(m.hidden(1, "gk_target_s"), true, "no sub line");
  assert.equal(m.hidden(1, "gk_target"), false);
  assert.equal(m.has(1, "gk_target", FADE), true, "still faded on the first frame, so the fade-in plays");
  assert.equal(m.timers.length, 1);
  m.tick();
  assert.equal(shown(m, 1), true);
  assert.equal(m.hidden(0, "gk_target"), true, "slot 0 was not in the audience");
});

test("set with the same content sends nothing", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "{red}Bob", sub: "{blue}Detective" });
  m.tick();
  const before = m.drives;
  for (let i = 0; i < 64; i++) t.set(0, { name: "{red}Bob", sub: "{blue}Detective" });
  assert.equal(m.drives, before, "64 identical sets, no HUD traffic");
  assert.equal(m.timers.length, 0);
});

test("tones come from the spec, then the leading tag; the sub line has its own", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "{red}Bob", sub: "{blue}Detective" });
  assert.equal(m.text(0, "gk_target_n"), "Bob");
  assert.equal(m.has(0, "gk_target_n", "gk-t-red"), true);
  assert.equal(m.text(0, "gk_target_s"), "Detective");
  assert.equal(m.has(0, "gk_target_s", "gk-t-blue"), true);
  assert.equal(m.hidden(0, "gk_target_s"), false);
  t.set(0, { name: "{red}Bob", tone: "green" });
  assert.equal(m.has(0, "gk_target_n", "gk-t-red"), false, "the old tone is removed");
  assert.equal(m.has(0, "gk_target_n", "gk-t-green"), true);
  assert.equal(m.hidden(0, "gk_target_s"), true, "dropping sub hides the line");
  t.set(0, { name: "Alice", sub: "Innocent" });
  assert.equal(m.has(0, "gk_target_s", "gk-t-grey"), true, "an untagged sub line is grey");
});

test("a name change while shown swaps the text without a fade", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  m.tick();
  t.set(0, { name: "Alice" });
  assert.equal(m.text(0, "gk_target_n"), "Alice");
  assert.equal(shown(m, 0), true);
  assert.equal(m.timers.length, 0);
});

test("hide fades first and collapses only after the fade", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  m.tick();
  t.hide(0);
  assert.equal(m.has(0, "gk_target", FADE), true);
  assert.equal(m.hidden(0, "gk_target"), false, "not collapsed yet: the fade is playing");
  m.tick();
  assert.equal(m.hidden(0, "gk_target"), true);
  const before = m.drives;
  t.hide(0);
  assert.equal(m.drives, before, "a second hide sends nothing");
});

test("a set during the fade-out brings it back without collapsing (no flicker)", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  m.tick();
  t.hide(0);
  t.set(0, { name: "Alice" });
  assert.equal(shown(m, 0), true, "fade dropped straight away");
  m.tick();
  assert.equal(m.hidden(0, "gk_target"), false, "the stale collapse timer does nothing");
  assert.equal(m.text(0, "gk_target_n"), "Alice");
});

test("a hide before the fade-in lands cancels it", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  t.hide(0);
  m.tick();
  assert.equal(m.has(0, "gk_target", FADE), true);
  assert.equal(m.hidden(0, "gk_target"), true);
});

test("audiences are per player", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set([0, 2], { name: "Bob" });
  m.tick();
  t.hide(2);
  m.tick();
  assert.equal(shown(m, 0), true);
  assert.equal(m.hidden(2, "gk_target"), true);
  assert.equal(m.hidden(1, "gk_target"), true);
});

test("name is required", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  assert.throws(() => t.set(0, {}), /name is required/);
  assert.throws(() => t.set(0, { name: "" }), /name is required/);
});

test("a disconnect or map change forgets the player's label", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  m.tick();
  m.mapStart();
  // A fresh layout entity starts every panel at its markup default (collapsed, faded).
  const before = m.drives;
  t.set(0, { name: "Bob" });
  assert.ok(m.drives > before, "the same name is painted again on the new entity");
  m.disconnect(0);
  const again = m.drives;
  t.set(0, { name: "Bob" });
  assert.ok(m.drives > again);
});

test("release forgets every label, so a new claim paints from scratch", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  m.tick();
  t.release();
  const t2 = m.GameHud.target();
  assert.ok(t2 && t2 !== t);
  t2.set(0, { name: "Bob" });
  assert.equal(m.hidden(0, "gk_target"), false);
  m.tick();
  assert.equal(shown(m, 0), true);
});

test("a refused paint is retried by the next set, not skipped as unchanged", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  m.refuse.add(0);
  t.set(0, { name: "Bob" });
  assert.equal(m.timers.length, 0, "nothing landed, nothing scheduled");
  m.refuse.delete(0);
  t.set(0, { name: "Bob" });
  assert.equal(m.text(0, "gk_target_n"), "Bob");
  assert.equal(m.hidden(0, "gk_target"), false);
  m.tick();
  assert.equal(shown(m, 0), true);
});

test("a refused reveal or fade-out is retried", () => {
  const m = mount({ files: [] });
  const t = m.GameHud.target();
  t.set(0, { name: "Bob" });
  m.refuse.add(0);
  m.tick();
  m.refuse.delete(0);
  t.set(0, { name: "Bob" });
  assert.equal(shown(m, 0), true, "the failed reveal is redone");
  m.refuse.add(0);
  t.hide(0);
  m.refuse.delete(0);
  t.hide(0);
  assert.equal(m.has(0, "gk_target", FADE), true, "the failed fade-out is redone");
});
