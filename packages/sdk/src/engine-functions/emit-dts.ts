import type { NormalizedBundle, NormalizedFunction, ProjectionSpec } from './model.ts';

function typeOf(projection: ProjectionSpec): string {
  switch (projection.id) {
    case 'void': return 'void';
    case 'bool': return 'boolean';
    case 'i32': case 'u32': case 'f32': case 'f64': return 'number';
    case 'i64': case 'u64': return 'bigint';
    case 'entity': return 'EntityRef';
    case 'entity?': return 'EntityRef | null';
    case 'string': return 'string';
    case 'vector': return '{ readonly x: number; readonly y: number; readonly z: number }';
    default: throw new Error(`no public TypeScript projection for ${projection.id}`);
  }
}

function binding(f: NormalizedFunction, name: string): string[] {
  const returnType = typeOf(f.abi.returns.projection);
  // Author names remain exact view keys; positional prefixes make every call label legal TS.
  const params = [
    ...(f.abi.receiver === 'entity' ? ['self: EntityRef'] : []),
    ...f.abi.parameters.map((p, i) => `arg${i}_${p.name}: ${typeOf(p.projection)}`),
  ];
  const preFields = [
    ...(f.abi.receiver === 'entity' ? ['    readonly self: EntityRef;'] : []),
    ...f.abi.parameters.map(p => `    ${p.mutable.includes('pre') ? '' : 'readonly '}${p.name}: ${typeOf(p.projection)};`),
  ];
  const postFields = [
    ...(f.abi.receiver === 'entity' ? ['    readonly self: EntityRef;'] : []),
    ...f.abi.parameters.map(p => `    readonly ${p.name}: ${typeOf(p.projection)};`),
    `    readonly returnValue: ${returnType};`,
  ];
  const preResult = f.policy.suppression === 'none'
    ? '0 | 1 | void'
    : returnType === 'void'
      ? '0 | 1 | 2 | 3 | void'
      : `0 | 1 | void | { action: 2 | 3; returnValue: ${returnType} }`;
  const surfaces = new Set(f.policy.surfaces);
  return [
    `  interface ${name}PreView {`, ...preFields, '  }',
    `  interface ${name}PostView {`, ...postFields, '  }',
    `  interface ${name}AvailableBinding {`,
    '    readonly available: true;',
    '    readonly status: FunctionStatus;',
    ...(surfaces.has('call') ? [`    call(${params.join(', ')}): ${returnType};`] : []),
    ...(surfaces.has('pre') ? [
      `    onPre(handler: (view: ${name}PreView) => ${preResult}): FunctionSubscription;`,
      // A plain `=> void` callback accepts value-returning lambdas in TypeScript. The union
      // preserves ordinary void expressions but rejects HookResult action values.
      `    onPre(options: { observeOnly: true }, handler: (view: Readonly<${name}PreView>) => void | undefined): FunctionSubscription;`,
    ] : []),
    ...(surfaces.has('post') ? [`    onPost(handler: (view: Readonly<${name}PostView>) => void): FunctionSubscription;`] : []),
    '  }',
    ...(f.requirement === 'optional' ? [
      `  interface ${name}UnavailableBinding {`,
      '    readonly available: false;',
      '    readonly status: FunctionStatus;',
      '  }',
    ] : []),
  ];
}

/** Concrete module augmentation is a typecheck root, not a user tsconfig requirement. */
export function emitEngineFunctionTypes(bundle: NormalizedBundle | undefined): string {
  const functions = bundle?.functions ?? [];
  const used = new Set<string>();
  const names = new Map<string, string>();
  for (const f of functions) {
    const base = f.localName[0]!.toUpperCase() + f.localName.slice(1);
    let name = base, suffix = 2;
    while (used.has(name)) name = `${base}_${suffix++}`;
    used.add(name);
    names.set(f.localName, name);
  }
  return [
    '// GENERATED from gamedata/functions.jsonc — DO NOT EDIT.',
    'import type { EntityRef } from "@s2script/sdk/entity";',
    'import "@s2script/sdk/unsafe";',
    'declare module "@s2script/sdk/unsafe" {',
    ...functions.flatMap(f => binding(f, names.get(f.localName)!)),
    '  interface EngineFunctions {',
    ...functions.map(f => `    ${f.localName}: ${names.get(f.localName)}AvailableBinding${f.requirement === 'optional' ? ` | ${names.get(f.localName)}UnavailableBinding` : ''};`),
    '  }',
    '}',
    '',
  ].join('\n');
}
