import { test } from 'node:test';
import assert from 'node:assert/strict';
import { parseFunctionFile } from '../src/engine-functions/parse.ts';
import { normalizeFunctions, stackCopyBytes, summarizeFunctions } from '../src/engine-functions/normalize.ts';
import { hashCanonical } from '../src/engine-functions/canonical-json.ts';

const target = { module: 'libserver.so', pattern: '55 48', validate: { prologue: '55 48' } };
const file = (functions, targets) => ({ schemaVersion: 2, ...(targets && { targets }), functions });
const norm = (functions, targets) => normalizeFunctions('@demo/functions', file(functions, targets));
const fn = (extra = {}) => ({ target, ...extra });

test('absent file has no normalized capability', () => {
  assert.equal(normalizeFunctions('@demo/functions', undefined), undefined);
});

test('parsed JSON rejects explicit null in optional declaration fields', () => {
  for (const field of ['resolve', 'parameters', 'returns', 'surfaces', 'requirement']) {
    const parsed = parseFunctionFile('functions.jsonc', JSON.stringify(file({ x: { ...fn(), [field]: null } })));
    assert.throws(() => normalizeFunctions('@demo/functions', parsed), new RegExp(field));
  }
});

test('parsed string-xref validators use native UTF-8 byte and int32 offset bounds', () => {
  const checked = xref => normalizeFunctions('@demo/functions', parseFunctionFile('functions.jsonc', JSON.stringify(file({
    x: fn({ target: { ...target, validate: { 'string-xref': xref } } }),
  }))));
  const base = { at: 0, dispOff: 3, instrLen: 7, expect: 'x' };
  assert.doesNotThrow(() => checked({ ...base, expect: 'é'.repeat(128), at: 2147483647, instrLen: 2147483647 }));
  assert.throws(() => checked({ ...base, expect: 'é'.repeat(129) }), /256.*bytes/i);
  assert.throws(() => checked({ ...base, at: 2147483648 }), /at.*int32|at.*range/i);
  assert.throws(() => checked({ ...base, instrLen: 2147483648 }), /instrLen.*int32|instrLen.*range/i);
});

test('defaults, canonical ids, bool projection, and derived permissions', () => {
  const b = norm({ z: fn({ parameters: [{ name: 'flag', type: 'bool' }], returns: 'bool' }), a: fn() });
  assert.deepEqual(b.functions.map(f => f.localName), ['a', 'z']);
  assert.equal(b.functions[1].canonicalId, '@demo/functions::z');
  assert.equal(b.functions[0].abi.fingerprint, 'linux-x86_64-sysv:none:void()');
  assert.equal(b.functions[1].abi.fingerprint, 'linux-x86_64-sysv:none:u8(u8)');
  assert.equal(b.functions[1].abi.parameters[0].projection.id, 'bool');
  assert.equal(b.functions[0].requirement, 'optional');
  assert.deepEqual(b.functions[0].policy.surfaces, ['call']);
  assert.equal(b.functions[0].policy.selfCall, 'bypass-own-hooks');
  assert.deepEqual(summarizeFunctions(b).permissions, ['engine:calls']);
  const { bundleHash, ...unhashed } = b;
  assert.equal(bundleHash, hashCanonical(unhashed));
});

test('local references flatten and target repair keeps contract hash stable', () => {
  const a = norm({ x: { target: { ref: 'shared' }, receiver: { type: 'entity' } } }, { shared: target });
  const b = norm({ x: { target: { ref: 'shared' }, receiver: { type: 'entity' } } }, { shared: { ...target, pattern: '90' } });
  assert.deepEqual(a.functions[0].target.targetValidate, target.validate);
  assert.equal(a.functions[0].target.pattern, target.pattern);
  assert.equal(a.functions[0].contractHash, b.functions[0].contractHash);
  assert.notEqual(a.bundleHash, b.bundleHash);
  assert.equal(a.bundleHash, norm({ x: { target: { ref: 'shared' }, receiver: { type: 'entity' } } }, { shared: target }).bundleHash);
});

test('validated-call preserves candidate and target validation stages', () => {
  const candidate = { 'string-xref': { at: 12, dispOff: 3, instrLen: 7, expect: 'only-one-site' } };
  const targetValidation = { prologue: '55 48' };
  const b = norm({ x: fn({ resolve: 'validated-call', target: { module: 'libserver.so', pattern: 'E8 ? ? ? ?', validate: candidate, targetValidate: targetValidation } }) });
  assert.deepEqual(b.functions[0].target.candidateValidate, candidate);
  assert.deepEqual(b.functions[0].target.targetValidate, targetValidation);
  assert.equal(b.functions[0].target.derivation, 'e8-rel32');
  assert.equal(b.functions[0].target.validate, undefined);
});

test('two-site validated-call fixture validates callers before one E8 derivation', () => {
  const recipe = norm({ x: fn({ resolve: 'validated-call', target: {
    module: 'libserver.so', pattern: 'E8 ? ? ? ?',
    validate: { 'string-xref': { at: 12, dispOff: 3, instrLen: 7, expect: 'winner' } },
    targetValidate: { prologue: '55 48' },
  } }) }).functions[0].target;
  const sites = [
    { at: 0, opcode: 0xe8, xref: 'decoy', callee: { prologue: '00 00' } },
    { at: 32, opcode: 0xe8, xref: 'winner', callee: { prologue: '55 48' } },
  ];
  const xref = recipe.candidateValidate['string-xref'];
  const candidates = sites.filter(site => site.opcode === 0xe8 && site.xref === xref.expect);
  assert.deepEqual(candidates.map(site => site.at), [32]);
  let derivations = 0;
  const derived = candidates.map(site => { derivations++; return site.callee; });
  assert.equal(derivations, 1);
  assert.equal(derived[0].prologue, recipe.targetValidate.prologue);
  assert.notEqual(sites[1].xref, recipe.targetValidate.prologue);
});

test('pointer-class projections copy values without exposing raw ptr authoring', () => {
  const b = norm({ x: fn({ parameters: [
    { name: 'subject', type: 'entity?' }, { name: 'label', type: 'string', ownership: 'callee-borrowed' }, { name: 'position', type: 'vector', ownership: 'callee-retained' },
  ], returns: 'entity' }) });
  assert.equal(b.functions[0].abi.fingerprint, 'linux-x86_64-sysv:none:ptr(ptr,ptr,ptr)');
  assert.deepEqual(b.functions[0].abi.parameters.map(p => p.projection.id), ['entity?', 'string', 'vector']);
  assert.equal(b.functions[0].abi.returns.projection.id, 'entity');
  assert.deepEqual(b.functions[0].abi.parameters.map(p => p.ownership), [undefined, 'callee-borrowed', 'callee-retained']);
});

test('copied ownership changes full hashes without changing the machine fingerprint', () => {
  const make = ownership => norm({ x: fn({ parameters: [{ name: 'text', type: 'string', ownership }], returns: { type: 'vector', ownership: 'caller-borrowed' }, surfaces: ['pre', 'post'], suppression: 'none' }) });
  const borrowed = make('callee-borrowed');
  const retained = make('callee-retained');
  assert.equal(borrowed.functions[0].abi.fingerprint, retained.functions[0].abi.fingerprint);
  assert.notEqual(borrowed.functions[0].contractHash, retained.functions[0].contractHash);
  assert.notEqual(borrowed.bundleHash, retained.bundleHash);
  assert.equal(borrowed.functions[0].policy.suppression, 'none');
  assert.equal(summarizeFunctions(borrowed).functions[0].suppresses, false);
});

test('copied ownership and suppression reject unsupported directions and surfaces by name', () => {
  const copied = (ownership, extra = {}) => fn({ parameters: [{ name: 'text', type: 'string', ownership, ...extra }], surfaces: ['pre', 'post'], returns: 'void' });
  const cases = [
    [copied(undefined), /text.*ownership.*required|text.*rebuild/i],
    [copied(null), /text.*ownership/i],
    [copied('caller-borrowed'), /text.*ownership/i],
    [copied('mystery'), /text.*ownership/i],
    [fn({ parameters: [{ name: 'count', type: 'i32', ownership: 'callee-borrowed' }] }), /count.*ownership/i],
    [fn({ returns: 'string' }), /returns.*ownership.*required|returns.*rebuild/i],
    [fn({ returns: { type: 'string', ownership: 'callee-borrowed' } }), /returns.*ownership/i],
    [fn({ returns: { type: 'i32', ownership: 'caller-borrowed' } }), /returns.*ownership|returns.*copied/i],
    [fn({ returns: { type: 'string', ownership: null } }), /returns.*ownership/i],
    [fn({ returns: { type: 'string', ownership: 'caller-borrowed', extra: true } }), /returns.*extra/i],
    [fn({ returns: { ownership: 'caller-borrowed' } }), /returns.*type/i],
    [fn({ parameters: [{ name: 'text', type: 'string', ownership: 'native-observed' }], surfaces: ['call'] }), /text.*call|call.*text/i],
    [copied('native-observed', { mutable: 'pre' }), /text.*mutable|text.*PRE/i],
    [fn({ returns: { type: 'string', ownership: 'native-observed' }, surfaces: ['call'] }), /returns.*call|call.*returns/i],
    [fn({ returns: { type: 'string', ownership: 'native-observed' }, surfaces: ['pre', 'post'] }), /returns.*suppression/i],
    [fn({ returns: { type: 'string', ownership: 'native-observed' }, surfaces: ['pre', 'post'], suppression: 'none' }) , null],
    [fn({ parameters: [{ name: 'text', type: 'string', ownership: 'callee-borrowed', mutable: 'pre' }], returns: 'entity', surfaces: ['pre'] }), /text.*callee-retained/i],
    [fn({ parameters: [{ name: 'text', type: 'string', ownership: 'callee-borrowed' }], returns: 'entity', surfaces: ['call'] }), null],
  ];
  for (const [decl, error] of cases) {
    if (error) assert.throws(() => norm({ x: decl }), error, JSON.stringify(decl));
    else assert.doesNotThrow(() => norm({ x: decl }), JSON.stringify(decl));
  }
});

test('hook permissions and inspectable summary derive from normalized contract', () => {
  const b = norm({ x: fn({ surfaces: ['post', 'pre', 'call'], parameters: [{ name: 'force', type: 'bool', mutable: 'pre' }] }) });
  const s = summarizeFunctions(b);
  assert.deepEqual(s.permissions, ['engine:calls', 'engine:hooks']);
  assert.deepEqual(s.functions[0].surfaces, ['call', 'pre', 'post']);
  assert.equal(s.functions[0].mutates, true);
  assert.equal(s.functions[0].suppresses, true);
});

test('rejects unsafe or unsupported declarations with names', () => {
  const cases = [
    [fn({ parameters: [{ name: 'self', type: 'i32' }] }), /self.*reserved/i],
    [fn({ parameters: [{ name: 'returnValue', type: 'i32' }] }), /returnValue.*reserved/i],
    [fn({ parameters: [{ name: 'x', type: 'i32' }, { name: 'x', type: 'i32' }] }), /duplicate.*x/i],
    [fn({ target: { module: 'libserver.so', pattern: '55' } }), /validate/i],
    [fn({ target: { ...target, validate: {} } }), /validator.*empty/i],
    [fn({ target: { ...target, validate: { 'mystery-validator': true } } }), /mystery-validator/i],
    [fn({ resolve: 'mystery' }), /resolve|derivation/i],
    [fn({ target: { ...target, module: '' } }), /module/i],
    [fn({ parameters: [{ name: 'x', type: 'u8' }] }), /u8/i],
    [fn({ parameters: [{ name: 'x', type: 'struct' }] }), /struct/i],
    [fn({ parameters: [{ name: 'x', type: '...' }] }), /\.\.\./i],
    [fn({ parameters: [{ name: 'x', type: 'ptr' }] }), /ptr/i],
    [fn({ projection: 'legacy.acquire.v1' }), /projection/i],
    [fn({ adapter: 'legacy.acquire.v1' }), /adapter/i],
    [fn({ parameters: [{ name: 'x', type: 'i32', mutable: 'post' }] }), /mutable/i],
    [fn({ surfaces: ['post'], suppression: 'generic' }), /suppression/i],
    [fn({ returns: 'i32', suppression: 'bare-handled' }), /suppression/i],
    [fn({ platform: 'windows-x64' }), /platform/i],
    [fn({ target: { ref: '@other/pkg::shared' } }), /local target ref/i],
    [fn({ resolve: 'validated-call', target }), /string-xref/i],
  ];
  for (const [decl, error] of cases) assert.throws(() => norm({ x: decl }), error);
});

test('all declared targets are validated, even if no function references one', () => {
  assert.throws(() => norm({}, { unused: { module: 'libserver.so', pattern: '55' } }), /validate/i);
});

test('virtual target keeps class and slot with target-stage prologue validation', () => {
  const virtual = { kind: 'virtual', module: 'libserver.so', class: 'CBasePlayerPawn', index: 400, validate: { prologue: '55 48' } };
  const b = norm({ x: fn({ target: virtual, receiver: { type: 'entity' } }) });
  assert.equal(b.functions[0].target.kind, 'virtual');
  assert.equal(b.functions[0].target.class, 'CBasePlayerPawn');
  assert.equal(b.functions[0].target.index, 400);
  assert.deepEqual(b.functions[0].target.candidateValidate, {});
  assert.deepEqual(b.functions[0].target.targetValidate, virtual.validate);
});

test('virtual targets reject missing prologue, invalid slot, and non-direct derivation', () => {
  const v = { kind: 'virtual', module: 'libserver.so', class: 'CBasePlayerPawn', index: 400, validate: { prologue: '55' } };
  assert.throws(() => norm({ x: fn({ target: { ...v, validate: { 'vtable-member': 'CBasePlayerPawn' } } }) }), /prologue/i);
  assert.throws(() => norm({ x: fn({ target: { ...v, index: 512 } }) }), /index|slot/i);
  assert.throws(() => norm({ x: fn({ target: v, resolve: 'validated-call' }) }), /virtual.*direct/i);
});

test('rejects count and computes bounded stack-copy bytes', () => {
  assert.throws(() => norm({ x: fn({ parameters: Array.from({ length: 33 }, (_, i) => ({ name: `p${i}`, type: 'i32' })) }) }), /32/);
  const b = norm({ x: fn({ receiver: { type: 'entity' }, parameters: Array.from({ length: 32 }, (_, i) => ({ name: `p${i}`, type: 'i32' })) }) });
  assert.equal(b.functions[0].abi.stackCopyBytes, 224);
  assert.equal(stackCopyBytes('none', ['i32', 'f32']), 128);
  assert.equal(stackCopyBytes('entity', [...Array(32).fill('i32')]), 224);
  assert.equal(stackCopyBytes('none', [...Array(9).fill('f64'), ...Array(7).fill('i32')]), 128);
  assert.ok(stackCopyBytes('entity', [...Array(40).fill('i32')]) > 256);
  assert.throws(() => stackCopyBytes('none', ['struct']), /unsupported ABI atom/);
});
