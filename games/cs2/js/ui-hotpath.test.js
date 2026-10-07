const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// Use the existing whole ui/components/menuhud world, then replace only its Client double
// with the real engine prelude Client implementation and recording host lifetime natives.
const stock = fs.readFileSync(path.join(__dirname, 'hudkit-prelude.test.js'), 'utf8');
const start = stock.indexOf('function pluginWorld(options = {})');
const end = stock.indexOf('test("legacy and explicit singleton ownership');
assert.ok(start >= 0 && end > start);
const pluginWorld = new Function('require', 'assert', 'readFileSync', 'join', '__dirname',
  stock.slice(start, end) + '\nreturn pluginWorld;')(require, assert, fs.readFileSync, path.join, __dirname);
const prelude = fs.readFileSync(path.join(__dirname, '../../../core/js/prelude.js'), 'utf8');
const clientStart = prelude.indexOf('  var __s2_client_tokens = new WeakMap();');
const clientEnd = prelude.indexOf('  // --- @s2script/sound', clientStart);
assert.ok(clientStart >= 0 && clientEnd > clientStart);
function world() {
  const w = pluginWorld(), p = w.plugin(), calls = [];
  p.ctx.__s2_client_generation = slot => { calls.push('generation'); const c = w.client(slot); return c ? String(c.generation) : '0'; };
  p.ctx.__s2_client_matches = (slot, token) => { calls.push('matches'); const c = w.client(slot); return !!c && String(c.generation) === token; };
  p.ctx.__s2_client_steamid = () => '0';
  p.ctx.__s2_client_subscribe = () => 0;
  vm.runInContext(prelude.slice(clientStart, clientEnd), p.ctx);
  const hud = p.base.kit.layout;
  hud.setText(1, 's2_banner_text', 'initial');
  const binding = hud._captureBinding(1);
  assert.equal(hud._bindingIsValid(binding), true);
  calls.length = 0;
  return { w, p, hud, binding, calls };
}

test('retained view validity does not construct a fresh Client after authoritative lifetime check', () => {
  const { binding, calls } = world();
  assert.equal(binding.view.isValid(), true);
  assert.deepEqual(calls, ['matches']);
});
test('component binding validity keeps both retained lifetimes and does not reconstruct Clients', () => {
  const { hud, binding, calls } = world();
  assert.equal(hud._bindingIsValid(binding), true);
  assert.deepEqual(calls, ['matches', 'matches']);
});
test('same-SteamID bot reconnect without JS notification retires the view and binding', () => {
  const { w, hud, binding } = world();
  assert.equal(binding.client.steamId, '0');
  w.replace(1);
  assert.equal(binding.view.isValid(), false);
  assert.equal(hud._bindingIsValid(binding), false);
  const before = w.writes.length;
  assert.equal(binding.view.setText('s2_banner_text', 'obsolete'), 'stale client');
  assert.equal(w.writes.length, before);
  assert.equal(hud._bindingIsValid(hud._captureBinding(1)), true);
});
test('component guard reconnect is checked before any primitive drive', () => {
  const { w, hud, binding } = world();
  binding._componentIsValid = () => { w.replace(1); return true; };
  const before = w.writes.length;
  assert.equal(hud._withBinding(binding, () => hud._drive.setText(1, 's2_banner_text', 'obsolete')), 'stale client');
  assert.equal(w.writes.length, before);
});
test('layout replacement still invalidates the captured entity epoch', () => {
  const { w, hud, binding } = world();
  w.replaceLayoutEntity();
  assert.equal(hud._bindingIsValid(binding), false);
  assert.equal(hud._bindingIsValid(hud._captureBinding(1)), true);
});
test('retained view forget invalidates the component epoch while an unrelated slot stays live', () => {
  const { hud, binding } = world();
  const other = hud._captureBinding(2);
  binding.view.forget();
  assert.equal(hud._bindingIsValid(binding), false);
  assert.equal(hud._bindingIsValid(other), true);
});
test('value coercion reconnect cannot write or publish a stale cache', () => {
  const { w, hud, binding } = world();
  const before = w.writes.length;
  const value = { toString() { w.replace(1); return 'obsolete'; } };
  assert.equal(hud._withBinding(binding, () => hud._drive.setText(1, 's2_banner_text', value)), 'stale client');
  assert.equal(w.writes.length, before);
  const next = hud._captureBinding(1);
  assert.equal(hud._withBinding(next, () => hud._drive.setText(1, 's2_banner_text', 'obsolete')).ok, true);
  assert.equal(w.writes.length, before + 1);
});
test('successful native drive followed by reconnect does not publish the obsolete cache', () => {
  const { w, hud, binding } = world();
  const push = w.writes.push; let changed = false;
  w.writes.push = function (...rows) {
    const result = push.apply(this, rows);
    if (!changed && rows.some(row => row.name === 'setDialogVariableStringForPlayer')) {
      changed = true; w.replace(1);
    }
    return result;
  };
  assert.equal(hud._withBinding(binding, () => hud._drive.setText(1, 's2_banner_text', 'same')), 'stale client');
  const before = w.writes.length;
  assert.equal(hud._withBinding(hud._captureBinding(1), () => hud._drive.setText(1, 's2_banner_text', 'same')).ok, true);
  assert.equal(w.writes.length, before + 1);
});
test('throwing nested different-slot binding restores the outer drive fence', () => {
  const { w, hud, binding } = world();
  const other = hud._captureBinding(2), marker = new Error('nested');
  const result = hud._withBinding(binding, () => {
    assert.throws(() => hud._withBinding(other, () => { throw marker; }), error => error === marker);
    return hud._drive.setText(1, 's2_banner_text', 'outer');
  });
  assert.equal(result.ok, true);
  assert.equal(w.writes.at(-1).args[1], 1);
});

test('a valid Client from another slot cannot authorize a mutated binding', () => {
  const { hud, binding } = world();
  assert.equal(hud._bindingIsValid({ ...binding, slot: 2 }), false);
});

function shopSheet() {
  const ctx = world(), { p, calls, w } = ctx;
  const rows = Array.from({ length: 8 }, (_, i) => ({ id: 'item' + i, a: 'Item ' + i, b: i + 'c', c: '', tone: i % 2 ? 'good' : undefined }));
  const modal = p.base.kit.modal({
    title: 'Shop', subtitle: () => '10 credits', rows: () => rows,
    detail: (slot, row) => row ? [row.a, 'Costs ' + row.b] : [],
    buttons: [{ text: 'Buy', variant: 'good', onClick() {} }, { text: 'Close', onClick() {} }],
  });
  modal.open(1);
  calls.length = 0;
  const before = w.writes.length;
  return { ...ctx, modal, rows, writes: () => w.writes.slice(before) };
}
test('a row pick sends only the changed primitives and skips guards for cached ones', () => {
  const { p, modal, calls, writes } = shopSheet();
  p.click(1, 's2_m0_r3');
  assert.equal(modal.cursor(1), 3);
  const sent = writes().filter(row => !row.host);
  // Cursor class off row 0 and on row 3, plus the two detail lines that changed.
  assert.equal(sent.length, 4, JSON.stringify(sent.map(row => row.args.slice(2))));
  // Every unchanged primitive used to pay its full guard chain before the diff cache dropped it:
  // 2581 Client lifetime checks for this one pick. The cache probe leaves 241.
  assert.ok(calls.length <= 300, `native lifetime checks per selection: ${calls.length}`);
});
test('a pick after the layout entity is replaced repaints every primitive', () => {
  const { p, w, writes } = shopSheet();
  w.replaceLayoutEntity();
  p.click(1, 's2_m0_r3');
  assert.equal(writes().filter(row => !row.host).length, 0, 'the stale sheet is not interactive');
});
test('an unchanged repaint after a panel-tree invalidation re-sends its values', () => {
  const { hud, modal, writes } = shopSheet();
  hud.invalidatePanelTree('s2_m0');
  modal.refresh(1);
  assert.ok(writes().filter(row => !row.host).length > 40);
});
