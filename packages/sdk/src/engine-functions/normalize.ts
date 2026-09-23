import { ATOM_WIDTHS, PLATFORM, RECEIVERS, gpRegisters, maxCifArguments, maxParameters, maxStackBytes, sseRegisters, stackSafetyBuffer } from './abi.generated.ts';
import { hashCanonical } from './canonical-json.ts';
import type { AuthorFunction, AuthorTarget, AuthorType, EngineFunctionsManifestSummary, FunctionFileV2, NativeAtom, NormalizedBundle, NormalizedFunction, NormalizedTarget, ProjectionSpec, Receiver, ValidatorSpec } from './model.ts';

const IDENTIFIER = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
const FORBIDDEN_NAMES = new Set(['self', 'returnValue', 'constructor', 'prototype', '__proto__']);
const SURFACES = ['call', 'pre', 'post'] as const;
const RESOLVERS = ['direct', 'ctor-body-xref', 'lea-disp', 'validated-call'] as const;
const TYPES = ['bool', 'i32', 'u32', 'i64', 'u64', 'f32', 'f64', 'entity', 'entity?', 'string', 'vector'] as const;

function object(value: unknown, where: string): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new Error(`${where} must be an object`);
  return value as Record<string, unknown>;
}

function keys(value: Record<string, unknown>, allowed: readonly string[], where: string): void {
  for (const key of Object.keys(value)) if (!allowed.includes(key)) throw new Error(`${where}: unsupported field ${JSON.stringify(key)}`);
}

function nonempty(value: unknown, where: string): string {
  if (typeof value !== 'string' || value.length === 0 || value.includes('\0')) throw new Error(`${where} must be a non-empty string without NUL`);
  return value;
}

function validator(value: unknown, where: string): ValidatorSpec {
  const v = object(value, where);
  keys(v, ['prologue', 'string-xref', 'vtable-member'], where);
  if (!Object.keys(v).length) throw new Error(`${where}: validator must not be empty`);
  if (v.prologue !== undefined) nonempty(v.prologue, `${where}.prologue`);
  if (v['vtable-member'] !== undefined) nonempty(v['vtable-member'], `${where}.vtable-member`);
  if (v['string-xref'] !== undefined) {
    const x = object(v['string-xref'], `${where}.string-xref`);
    keys(x, ['at', 'dispOff', 'instrLen', 'expect'], `${where}.string-xref`);
    for (const k of ['at', 'dispOff', 'instrLen']) if (!Number.isSafeInteger(x[k]) || (x[k] as number) < (k === 'instrLen' ? 1 : 0)) throw new Error(`${where}.string-xref.${k} must be a valid offset`);
    if ((x.dispOff as number) + 4 > (x.instrLen as number)) throw new Error(`${where}.string-xref displacement exceeds instruction`);
    for (const k of ['at', 'instrLen']) if ((x[k] as number) > 2147483647) throw new Error(`${where}.string-xref.${k} exceeds int32 range`);
    const expect = nonempty(x.expect, `${where}.string-xref.expect`);
    if (Buffer.byteLength(expect, 'utf8') > 256) throw new Error(`${where}.string-xref.expect exceeds 256 UTF-8 bytes`);
  }
  return v as ValidatorSpec;
}

function target(value: unknown, resolve: string, where: string): NormalizedTarget {
  const t = object(value, where);
  if (t.kind === 'virtual') {
    keys(t, ['kind', 'module', 'class', 'index', 'validate'], where);
    if (resolve !== 'direct') throw new Error(`${where}: virtual target requires direct resolution`);
    const module = nonempty(t.module, `${where}.module`);
    const className = nonempty(t.class, `${where}.class`);
    if (!Number.isInteger(t.index) || (t.index as number) < 0 || (t.index as number) >= 512) throw new Error(`${where}.index must be a virtual slot from 0 through 511`);
    const validate = validator(t.validate, `${where}.validate`);
    if (!validate.prologue) throw new Error(`${where}: virtual target requires validate.prologue`);
    return { kind: 'virtual', module, class: className, index: t.index as number, resolve: 'direct', derivation: 'virtual-slot', candidateValidate: {}, targetValidate: validate };
  }
  if (t.kind !== undefined && t.kind !== 'signature') throw new Error(`${where}: unsupported target kind ${JSON.stringify(t.kind)}`);
  keys(t, ['kind', 'module', 'pattern', 'validate', 'targetValidate'], where);
  const module = nonempty(t.module, `${where}.module`);
  const pattern = nonempty(t.pattern, `${where}.pattern`);
  const validate = validator(t.validate, `${where}.validate`);
  if (resolve === 'validated-call' && !validate['string-xref']) throw new Error(`${where}: validated-call requires candidate validate.string-xref`);
  if (resolve !== 'validated-call' && t.targetValidate !== undefined) throw new Error(`${where}: targetValidate is only supported with validated-call`);
  const targetValidate = t.targetValidate === undefined ? {} : validator(t.targetValidate, `${where}.targetValidate`);
  return {
    kind: 'signature',
    module, pattern, resolve: resolve as NormalizedTarget['resolve'],
    derivation: ({ direct: 'identity', 'ctor-body-xref': 'ctor-body-xref', 'lea-disp': 'lea-disp', 'validated-call': 'e8-rel32' } as Record<string, 'identity' | 'ctor-body-xref' | 'lea-disp' | 'e8-rel32'>)[resolve]!,
    candidateValidate: resolve === 'validated-call' ? validate : {},
    targetValidate: resolve === 'validated-call' ? targetValidate : validate,
  };
}

function projection(type: AuthorType | 'void', where: string): { native: NativeAtom | 'void'; projection: ProjectionSpec } {
  if (type === 'void') return { native: 'void', projection: { id: 'void', version: 1 } };
  if (!(TYPES as readonly string[]).includes(type)) throw new Error(`${where}: unsupported author type ${JSON.stringify(type)}`);
  if (type === 'bool') return { native: 'u8', projection: { id: 'bool', version: 1 } };
  if (type === 'entity' || type === 'entity?' || type === 'string' || type === 'vector') {
    return { native: 'ptr', projection: { id: type, version: 1 } };
  }
  return { native: type, projection: { id: type, version: 1 } };
}

/** Mirror the proven bounded SysV scalar classification, including provider's 128-byte floor. */
export function stackCopyBytes(receiver: Receiver, atoms: readonly NativeAtom[]): number {
  let gp = receiver === 'entity' ? 1 : 0;
  let sse = 0, spill = 0;
  for (const atom of atoms) {
    if (!(atom in ATOM_WIDTHS)) throw new Error(`unsupported ABI atom ${atom}`);
    if (atom === 'f32' || atom === 'f64') { if (sse++ >= sseRegisters) spill += 8; }
    else if (gp++ >= gpRegisters) spill += 8;
  }
  return Math.max(stackSafetyBuffer, Math.ceil(spill / 16) * 16);
}

export function normalizeFunctions(ownerId: string, parsed: FunctionFileV2 | undefined): NormalizedBundle | undefined {
  if (parsed === undefined) return undefined;
  nonempty(ownerId, 'ownerId');
  const root = object(parsed, 'function file');
  keys(root, ['schemaVersion', 'targets', 'functions'], 'function file');
  if (root.schemaVersion !== 2) throw new Error('function file schemaVersion must be 2');
  const targets = root.targets === undefined ? {} : object(root.targets, 'targets');
  const functions = object(root.functions, 'functions');
  for (const [name, raw] of Object.entries(targets)) {
    if (!IDENTIFIER.test(name) || FORBIDDEN_NAMES.has(name)) throw new Error(`target ${JSON.stringify(name)}: invalid or reserved name`);
    const t = object(raw, `target ${JSON.stringify(name)}`);
    // Local targets have no resolver of their own; validate their fields now and
    // validate strategy compatibility again at each function that references one.
    target(t, t.kind === 'virtual' ? 'direct' : t.targetValidate !== undefined ? 'validated-call' : 'direct', `target ${JSON.stringify(name)}`);
  }
  const normalized: NormalizedFunction[] = [];
  for (const localName of Object.keys(functions).sort()) {
    const where = `function ${JSON.stringify(localName)}`;
    if (!IDENTIFIER.test(localName) || FORBIDDEN_NAMES.has(localName)) throw new Error(`${where}: invalid or reserved function name`);
    const f = object(functions[localName], where);
    keys(f, ['target', 'receiver', 'parameters', 'returns', 'surfaces', 'requirement', 'resolve'], where);
    const resolve = f.resolve === undefined ? 'direct' : f.resolve;
    if (!(RESOLVERS as readonly unknown[]).includes(resolve)) throw new Error(`${where}: unknown resolve derivation ${JSON.stringify(resolve)}`);
    const rawTarget = object(f.target, `${where}.target`);
    const resolvedTarget = 'ref' in rawTarget ? (() => {
      keys(rawTarget, ['ref'], `${where}.target`);
      const ref = nonempty(rawTarget.ref, `${where}.target.ref`);
      if (!Object.prototype.hasOwnProperty.call(targets, ref)) throw new Error(`${where}: unknown local target ref ${JSON.stringify(ref)}`);
      return targets[ref] as AuthorTarget;
    })() : rawTarget as unknown as AuthorTarget;
    const normalizedTarget = target(resolvedTarget, resolve as string, `${where}.target`);
    const rawReceiver = f.receiver === undefined ? { type: 'none' } : object(f.receiver, `${where}.receiver`);
    keys(rawReceiver, ['type'], `${where}.receiver`);
    const receiver = rawReceiver.type;
    if (!(RECEIVERS as readonly unknown[]).includes(receiver)) throw new Error(`${where}: unsupported receiver ${JSON.stringify(receiver)}`);
    const rawParams = f.parameters === undefined ? [] : f.parameters;
    if (!Array.isArray(rawParams)) throw new Error(`${where}.parameters must be an array`);
    if (rawParams.length > maxParameters || rawParams.length + (receiver === 'entity' ? 1 : 0) > maxCifArguments) throw new Error(`${where}: maximum ${maxParameters} authored parameters / ${maxCifArguments} CIF arguments exceeded`);
    const names = new Set<string>();
    const parameters = rawParams.map((raw, i) => {
      const p = object(raw, `${where}.parameters[${i}]`);
      keys(p, ['name', 'type', 'mutable'], `${where}.parameters[${i}]`);
      const name = nonempty(p.name, `${where}.parameters[${i}].name`);
      if (!IDENTIFIER.test(name) || FORBIDDEN_NAMES.has(name)) throw new Error(`${where}: parameter ${JSON.stringify(name)} is invalid or reserved`);
      if (names.has(name)) throw new Error(`${where}: duplicate parameter name ${JSON.stringify(name)}`);
      names.add(name);
      if (p.mutable !== undefined && p.mutable !== 'pre') throw new Error(`${where}: parameter ${name} mutable must be "pre"`);
      const type = projection(p.type as AuthorType, `${where}.parameters[${i}].type`);
      return { name, native: type.native as NativeAtom, projection: type.projection, mutable: p.mutable === 'pre' ? ['pre'] as ['pre'] : [] as [] };
    });
    const returns = projection((f.returns === undefined ? 'void' : f.returns) as AuthorType | 'void', `${where}.returns`);
    const rawSurfaces = f.surfaces === undefined ? ['call'] : f.surfaces;
    if (!Array.isArray(rawSurfaces) || !rawSurfaces.length || rawSurfaces.some(s => !(SURFACES as readonly unknown[]).includes(s)) || new Set(rawSurfaces).size !== rawSurfaces.length) throw new Error(`${where}: invalid surfaces`);
    const surfaces = SURFACES.filter(s => rawSurfaces.includes(s));
    if (parameters.some(p => p.mutable.length) && !surfaces.includes('pre')) throw new Error(`${where}: mutable parameter requires pre surface`);
    const requirement = f.requirement === undefined ? 'optional' : f.requirement;
    if (requirement !== 'optional' && requirement !== 'required') throw new Error(`${where}: invalid requirement`);
    const receiverType = receiver as Receiver;
    const stackBytes = stackCopyBytes(receiverType, parameters.map(p => p.native));
    if (stackBytes > maxStackBytes) throw new Error(`${where}: stack-copy requirement ${stackBytes} exceeds ${maxStackBytes} bytes`);
    const nativeReturn = returns.native;
    const abi = {
      platform: PLATFORM, receiver: receiverType,
      fingerprint: `${PLATFORM}:${receiverType}:${nativeReturn}(${parameters.map(p => p.native).join(',')})`,
      stackCopyBytes: stackBytes, parameters,
      returns: { native: nativeReturn, projection: returns.projection },
    };
    const policyBase = {
      id: 'generic.v2' as const, version: 1 as const, surfaces,
      selfCall: 'bypass-own-hooks' as const,
      suppression: (surfaces.includes('pre') ? 'generic' : 'none') as 'generic' | 'none',
    };
    const policy = { ...policyBase, contractHash: hashCanonical(policyBase) };
    normalized.push({ localName, canonicalId: `${ownerId}::${localName}`, contractHash: hashCanonical({ abi, policy }), target: normalizedTarget, abi, policy, requirement });
  }
  const hashBase = { schemaVersion: 2 as const, ownerId, functions: normalized };
  const bundleHash = hashCanonical(hashBase);
  return { ...hashBase, bundleHash };
}

export function summarizeFunctions(bundle: NormalizedBundle): EngineFunctionsManifestSummary {
  const permissions: EngineFunctionsManifestSummary['permissions'] = [];
  if (bundle.functions.some(f => f.policy.surfaces.includes('call'))) permissions.push('engine:calls');
  if (bundle.functions.some(f => f.policy.surfaces.some(s => s === 'pre' || s === 'post'))) permissions.push('engine:hooks');
  return { schemaVersion: 2, bundleHash: bundle.bundleHash, permissions, functions: bundle.functions.map(f => ({
    canonicalId: f.canonicalId, contractHash: f.contractHash, surfaces: f.policy.surfaces,
    mutates: f.abi.parameters.some(p => p.mutable.length > 0),
    suppresses: f.policy.suppression !== 'none', requirement: f.requirement,
  })) };
}
