import { closeSync, existsSync, linkSync, lstatSync, mkdirSync, openSync, readFileSync, readdirSync, renameSync, unlinkSync, writeFileSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';
import ts from 'typescript';
import { parseFunctionFile } from './parse.ts';
import { normalizeFunctions, summarizeFunctions } from './normalize.ts';
import { canonicalJson } from './canonical-json.ts';
import { validatePluginGamedata } from '../gamedata/validate.ts';
import type { AuthorFunction, AuthorTarget, FunctionFileV2, Surface, ValidatorSpec } from './model.ts';

const PLATFORM = 'linuxsteamrt64';
const SCOPE = 'structurally lossless conversion of declared v1 contract; native ABI completeness is not verified';
const IDENT = /^[A-Za-z_$][\w$]*$/;
const AUTHOR_TYPES = { bool: 'bool', int: 'i32', float: 'f32', entity: 'entity?' } as const;
type Raw = Record<string, unknown>;
type Entry = { oldCall?: string; oldHook?: string; oldExposeCtx?: string; oldReceiverAs?: string; canonicalId: string; validatorStages: { candidate?: ValidatorSpec; target: ValidatorSpec }; surfaces: Surface[]; mutates: boolean; suppression: string; policyId: string; compatibilityAdapter: string; bypassWith?: string };
export interface MigrationReport {
  scope: string;
  package: string;
  input?: string;
  destination: string;
  entries: Entry[];
  permissions: string[];
  sourceReferences: Array<{ file: string; line: number; kind: string; name: string; manualEdit: string }>;
  generatedDeclarations: string[];
  generatedDeclarationHashes: Record<string, string>;
  recommendedEdits: string[];
  ambiguities: string[];
  filesChanged: string[];
  replacedFile?: string;
  output?: FunctionFileV2;
}

const obj = (v: unknown): Raw | undefined => v !== null && typeof v === 'object' && !Array.isArray(v) ? v as Raw : undefined;
const valid = (v: unknown): v is string => typeof v === 'string' && v.length > 0;
const same = (a: unknown, b: unknown): boolean => canonicalJson(a) === canonicalJson(b);
function pathExists(path: string): boolean {
  try { lstatSync(path); return true; }
  catch (e) { if ((e as { code?: string }).code === 'ENOENT') return false; throw e; }
}
function fields(value: Raw, allowed: string[], where: string, errors: string[]): void {
  for (const key of Object.keys(value)) if (!allowed.includes(key)) errors.push(`${where}: unsupported field ${key}`);
}
function contained(dir: string, path: string): boolean {
  const r = relative(dir, path);
  return r === '' || (r !== '..' && !r.startsWith(`..${sep}`) && !isAbsolute(r));
}
function targetOf(ref: unknown, signatures: Raw, where: string, errors: string[]): { target: AuthorTarget; resolve: AuthorFunction['resolve'] } | undefined {
  const t = obj(ref);
  if (!t) { errors.push(`${where}: target must be an object`); return; }
  if (t.kind === 'signature') {
    fields(t, ['kind', 'name', 'validate'], `${where}.target`, errors);
    const sig = obj(obj(signatures[t.name as string])?.[PLATFORM]);
    if (!valid(t.name) || !sig) { errors.push(`${where}: missing named signature ${JSON.stringify(t.name)}`); return; }
    fields(sig, ['module', 'pattern', 'resolve', 'validate', 'targetValidate'], `signature ${t.name}`, errors);
    const validator = t.validate === undefined ? sig.validate : t.validate;
    if (!obj(validator) || !Object.keys(obj(validator)!).length) { errors.push(`${where}: absent or empty validator`); return; }
    if (!valid(sig.module) || !valid(sig.pattern)) { errors.push(`${where}: incomplete signature module/pattern`); return; }
    return { target: { module: sig.module, pattern: sig.pattern, validate: validator as ValidatorSpec, ...(sig.targetValidate !== undefined && { targetValidate: sig.targetValidate as ValidatorSpec }) }, resolve: sig.resolve as AuthorFunction['resolve'] };
  }
  if (t.kind === 'vtable') {
    fields(t, ['kind', 'class', 'module', PLATFORM], `${where}.target`, errors);
    const p = obj(t[PLATFORM]);
    if (!valid(t.class) || !p || !Number.isInteger(p.index) || !obj(p.validate) || !Object.keys(obj(p.validate)!).length) { errors.push(`${where}: incomplete virtual target or empty validator`); return; }
    fields(p, ['index', 'validate'], `${where}.target.${PLATFORM}`, errors);
    if (t.module !== undefined && !valid(t.module)) { errors.push(`${where}: virtual module must be a non-empty string`); return; }
    // The v1 resolver supplies this exact default for an omitted module (engine_consumer.cpp).
    return { target: { kind: 'virtual', module: (t.module as string | undefined) ?? 'libserver.so', class: t.class, index: p.index as number, validate: p.validate as ValidatorSpec }, resolve: 'direct' };
  }
  errors.push(`${where}: unsupported target kind ${JSON.stringify(t.kind)}`);
}
function callOf(name: string, value: unknown, signatures: Raw, errors: string[]): AuthorFunction | undefined {
  const where = `call ${name}`;
  const c = obj(value);
  if (!c) { errors.push(`${where}: declaration must be an object`); return; }
  fields(c, ['receiver', 'target', 'args', 'argNames', 'returns'], where, errors);
  const r = obj(c.receiver);
  if (!r || (r.kind !== 'entity' && r.kind !== 'none')) errors.push(`${where}: unsupported receiver`);
  else { fields(r, ['kind', 'via'], `${where}.receiver`, errors); if (r.via !== undefined) errors.push(`${where}: receiver.via hop cannot be represented without changing meaning`); }
  const args = c.args;
  const names = c.argNames;
  if (!Array.isArray(args) || !Array.isArray(names) || args.length !== names.length || names.some(n => !valid(n) || !IDENT.test(n))) errors.push(`${where}: args/argNames require exact lengths and valid names`);
  const target = targetOf(c.target, signatures, where, errors);
  if (c.returns === 'string' || c.returns === 'vector') errors.push(`${where}: copied return ownership is unknown`);
  else if (!['void', 'bool', 'int', 'float', 'entity'].includes(c.returns as string)) errors.push(`${where}: unsupported return ABI ${JSON.stringify(c.returns)}`);
  if (Array.isArray(args)) for (const a of args) {
    if (a === 'string' || a === 'vector') errors.push(`${where}: copied ${a} ownership is unknown`);
    else if (typeof a !== 'string' || !(a in AUTHOR_TYPES)) errors.push(`${where}: unsupported ABI feature ${JSON.stringify(a)}`);
  }
  if (!target || !r || !Array.isArray(args) || !Array.isArray(names) || args.length !== names.length || args.some(a => typeof a !== 'string' || !(a in AUTHOR_TYPES))) return;
  const returns = c.returns === 'void' ? 'void' : AUTHOR_TYPES[c.returns as keyof typeof AUTHOR_TYPES];
  if (!returns) return;
  return { target: target.target, resolve: target.resolve, receiver: { type: r.kind as 'entity' | 'none' }, parameters: args.map((a, i) => ({ name: names[i] as string, type: AUTHOR_TYPES[a as keyof typeof AUTHOR_TYPES] })), returns, surfaces: ['call'] };
}
function hookOf(name: string, value: unknown, signatures: Raw, errors: string[]): { function: AuthorFunction; bypassWith?: string; exposeCtx: string } | undefined {
  const where = `hook ${name}`;
  const h = obj(value);
  if (!h) { errors.push(`${where}: declaration must be an object`); return; }
  fields(h, ['target', 'shape', 'params', 'mutable', 'receiver', 'bypassWith', 'expose'], where, errors);
  const r = obj(h.receiver);
  if (!r || r.kind !== 'entity' || !valid(r.as)) errors.push(`${where}: receiver is not a representable entity receiver`);
  else fields(r, ['kind', 'as'], `${where}.receiver`, errors);
  const expose = obj(h.expose);
  if (!expose || !valid(expose.ctx)) errors.push(`${where}: missing expose.ctx`);
  else fields(expose, ['ctx'], `${where}.expose`, errors);
  const shape = h.shape;
  const types = shape === 'this_void' ? [] : shape === 'this_f32_i32_i32_i32' ? ['f32', 'i32', 'i32', 'i32'] : undefined;
  if (!types) errors.push(`${where}: unsupported or unrepresentable shape projection ${JSON.stringify(shape)}`);
  const names = h.params ?? [];
  if (!Array.isArray(names) || !types || names.length !== types.length || names.some(n => !valid(n) || !IDENT.test(n))) errors.push(`${where}: shape and params require exact lengths and valid names`);
  const mutable = h.mutable ?? [];
  if (!Array.isArray(mutable) || mutable.some(n => !Array.isArray(names) || !names.includes(n))) errors.push(`${where}: mutable names must match params`);
  const target = targetOf(h.target, signatures, where, errors);
  if (h.bypassWith !== undefined && !valid(h.bypassWith)) errors.push(`${where}: invalid bypassWith`);
  if (!target || !r || !expose || !types || !Array.isArray(names) || names.length !== types.length || !Array.isArray(mutable)) return;
  return { function: { target: target.target, resolve: target.resolve, receiver: { type: 'entity' }, parameters: types.map((type, i) => ({ name: names[i] as string, type: type as 'f32' | 'i32', ...(mutable.includes(names[i]) && { mutable: 'pre' as const }) })), returns: 'void', surfaces: ['pre'] }, bypassWith: h.bypassWith as string | undefined, exposeCtx: expose.ctx as string };
}

function sourceFiles(dir: string, pkg: Raw): string[] {
  const result = new Set<string>();
  const src = join(dir, 'src');
  function walk(at: string): void {
    if (!existsSync(at)) return;
    for (const item of readdirSync(at, { withFileTypes: true })) {
      const p = join(at, item.name);
      if (item.isDirectory()) walk(p);
      else if (item.isFile() && /\.[cm]?[jt]sx?$/.test(item.name)) result.add(p);
    }
  }
  walk(src);
  if (valid(pkg.main)) {
    const p = resolve(dir, pkg.main);
    if (contained(dir, p) && existsSync(p) && lstatSync(p).isFile()) result.add(p);
  }
  const queue = [...result];
  for (let i = 0; i < queue.length; i++) {
    const file = queue[i]!;
    const ast = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true);
    for (const statement of ast.statements) {
      if (!ts.isImportDeclaration(statement) && !ts.isExportDeclaration(statement)) continue;
      const spec = statement.moduleSpecifier;
      if (!spec || !ts.isStringLiteral(spec) || !spec.text.startsWith('.')) continue;
      const base = resolve(dirname(file), spec.text);
      for (const candidate of [base, ...['.ts', '.tsx', '.js', '.jsx', '.mts', '.mjs', '.cts', '.cjs'].map(x => base + x), ...['index.ts', 'index.tsx', 'index.js', 'index.jsx'].map(x => join(base, x))]) {
        if (!contained(dir, candidate) || !existsSync(candidate) || !lstatSync(candidate).isFile() || !/\.[cm]?[jt]sx?$/.test(candidate) || result.has(candidate)) continue;
        result.add(candidate);
        queue.push(candidate);
        break;
      }
    }
  }
  return [...result].sort();
}
function scanSource(files: string[], calls: Set<string>, hooks: Map<string, string>, report: MigrationReport): void {
  for (const file of files) {
    const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true) as ts.SourceFile & { parseDiagnostics?: readonly ts.Diagnostic[] };
    if (source.parseDiagnostics?.length) report.ambiguities.push(`${relative(report.package, file)}: source syntax errors prevent reliable reference analysis`);
    const visit = (node: ts.Node): void => {
      if (ts.isElementAccessExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'Engine') {
        const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
        report.ambiguities.push(`${relative(report.package, file)}:${line}: computed Engine lookup prevents proving timing/null behavior`);
      }
      if (ts.isElementAccessExpression(node) && hooks.size && (
        (ts.isIdentifier(node.expression) && node.expression.text === 'ctx') ||
        (ts.isPropertyAccessExpression(node.expression) && ts.isIdentifier(node.expression.expression) && node.expression.expression.text === 'ctx')
      )) {
        const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
        report.ambiguities.push(`${relative(report.package, file)}:${line}: computed ctx hook lookup prevents proving callback timing`);
      }
      if (ts.isPropertyAccessExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'Engine' && ['call', 'hook'].includes(node.name.text) && !(ts.isCallExpression(node.parent) && node.parent.expression === node)) {
        const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
        report.ambiguities.push(`${relative(report.package, file)}:${line}: indirect Engine.${node.name.text} lookup prevents proving timing/null behavior`);
      }
      if (ts.isCallExpression(node)) {
        const expr = node.expression;
        let kind: string | undefined;
        if (ts.isPropertyAccessExpression(expr) && ts.isIdentifier(expr.expression) && expr.expression.text === 'Engine' && ['call', 'hook'].includes(expr.name.text)) kind = expr.name.text;
        if (kind) {
          const arg = node.arguments[0];
          const line = source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1;
          if (!arg || (!ts.isStringLiteral(arg) && !ts.isNoSubstitutionTemplateLiteral(arg))) report.ambiguities.push(`${relative(report.package, file)}:${line}: dynamic Engine.${kind} reference prevents proving timing/null behavior`);
          else {
            const name = arg.text;
            if (!(kind === 'call' ? calls.has(name) : hooks.has(name))) report.ambiguities.push(`${relative(report.package, file)}:${line}: Engine.${kind} references undeclared ${name}`);
            else report.sourceReferences.push({ file, line, kind: `Engine.${kind}`, name, manualEdit: `review ${kind} lookup, null behavior and callback timing for ${name}` });
          }
        }
        if (ts.isPropertyAccessExpression(expr) && ts.isPropertyAccessExpression(expr.expression) && ts.isIdentifier(expr.expression.expression) && expr.expression.expression.text === 'ctx') {
          const ns = expr.expression.name.text;
          const name = expr.name.text;
          if (hooks.get(name) === ns) report.sourceReferences.push({ file, line: source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1, kind: 'ctx hook', name, manualEdit: `review callback timing and replace ctx.${ns}.${name} manually` });
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
}

export function analyzeV1Migration(pluginDir: string, options: { force?: boolean } = {}): MigrationReport {
  const dir = resolve(pluginDir);
  const destination = join(dir, 'gamedata', 'functions.jsonc');
  const report: MigrationReport = { scope: SCOPE, package: dir, destination, entries: [], permissions: [], sourceReferences: [], generatedDeclarations: [], generatedDeclarationHashes: {}, recommendedEdits: [], ambiguities: [], filesChanged: [] };
  if (pathExists(destination) && !options.force) report.ambiguities.push(`${destination}: destination already exists; use --force to replace it`);
  let pkg: Raw;
  try { pkg = parseFunctionFile(join(dir, 'package.json'), readFileSync(join(dir, 'package.json'), 'utf8')) as unknown as Raw; }
  catch (e) { report.ambiguities.push(`package.json: ${(e as Error).message}`); return report; }
  if (!valid(pkg.name)) { report.ambiguities.push('package.json: missing package name'); return report; }
  const s2 = obj(pkg.s2script);
  if (!s2 || !valid(s2.gamedata)) { report.ambiguities.push('package.json: missing s2script.gamedata'); return report; }
  const input = resolve(dir, s2.gamedata);
  report.input = input;
  if (!contained(dir, input)) { report.ambiguities.push('s2script.gamedata escapes plugin directory'); return report; }
  let gd: Raw;
  try { gd = parseFunctionFile(input, readFileSync(input, 'utf8')) as unknown as Raw; }
  catch (e) { report.ambiguities.push(`gamedata: ${(e as Error).message}`); return report; }
  fields(gd, ['signatures', 'calls', 'hooks'], 'gamedata', report.ambiguities);
  const signatures = obj(gd.signatures) ?? {};
  const calls = obj(gd.calls) ?? {};
  const hooks = obj(gd.hooks) ?? {};
  if (gd.signatures !== undefined && !obj(gd.signatures)) report.ambiguities.push('signatures must be an object');
  if (gd.calls !== undefined && !obj(gd.calls)) report.ambiguities.push('calls must be an object');
  if (gd.hooks !== undefined && !obj(gd.hooks)) report.ambiguities.push('hooks must be an object');
  const permissions = Array.isArray(s2.permissions) ? s2.permissions as string[] : [];
  if (obj(gd.signatures ?? {}) && obj(gd.calls ?? {}) && obj(gd.hooks ?? {})) {
    try { report.ambiguities.push(...validatePluginGamedata(gd, { permissions })); }
    catch (e) { report.ambiguities.push(`v1 validation: ${(e as Error).message}`); }
  }
  const author: FunctionFileV2 = { schemaVersion: 2, functions: {} };
  const hookCtx = new Map<string, string>();
  const pairOwners = new Map<string, string>();
  const usedSignatures = new Set<string>();
  for (const raw of [...Object.values(calls), ...Object.values(hooks)]) {
    const name = obj(obj(raw)?.target)?.name;
    if (valid(name)) usedSignatures.add(name);
  }
  for (const [name, raw] of Object.entries(signatures)) {
    const platforms = obj(raw);
    if (!platforms) { report.ambiguities.push(`signature ${name}: platforms must be an object`); continue; }
    fields(platforms, [PLATFORM], `signature ${name}`, report.ambiguities);
    if (!usedSignatures.has(name)) report.ambiguities.push(`signature ${name}: unreferenced declaration cannot be silently discarded`);
  }
  for (const [name, raw] of Object.entries(calls)) {
    const converted = callOf(name, raw, signatures, report.ambiguities);
    if (converted) author.functions[name] = converted;
  }
  for (const [name, raw] of Object.entries(hooks)) {
    const converted = hookOf(name, raw, signatures, report.ambiguities);
    if (!converted) continue;
    hookCtx.set(name, converted.exposeCtx);
    if (converted.bypassWith) {
      const oldCall = author.functions[converted.bypassWith];
      if (!oldCall) { report.ambiguities.push(`hook ${name}: bypassWith ${converted.bypassWith} names no convertible call`); continue; }
      const earlier = pairOwners.get(converted.bypassWith);
      if (earlier) report.ambiguities.push(`hook ${name}: call ${converted.bypassWith} is already paired with hook ${earlier}`);
      pairOwners.set(converted.bypassWith, name);
      if (!same(oldCall.target, converted.function.target)) report.ambiguities.push(`hook ${name}: paired target disagrees with call ${converted.bypassWith}`);
      const a = oldCall.parameters ?? [], b = converted.function.parameters ?? [];
      if (oldCall.receiver?.type !== converted.function.receiver?.type || oldCall.returns !== converted.function.returns || a.length !== b.length || a.some((p, i) => p.type !== b[i]?.type)) report.ambiguities.push(`hook ${name}: paired ABI disagrees with call ${converted.bypassWith}`);
      if (a.some((p, i) => p.name !== b[i]?.name)) report.ambiguities.push(`hook ${name}: paired parameter names disagree with call ${converted.bypassWith}`);
      if (name !== converted.bypassWith && author.functions[name]) report.ambiguities.push(`hook ${name}: name collides with call`);
      const merged: AuthorFunction = { ...oldCall, parameters: a.map((p, i) => ({ ...p, ...(b[i]?.mutable && { mutable: 'pre' as const }) })), surfaces: ['call', 'pre'] };
      author.functions[converted.bypassWith] = merged;
    } else {
      if (author.functions[name]) report.ambiguities.push(`hook ${name}: name collides with call`);
      else author.functions[name] = converted.function;
      for (const [callName, oldCall] of Object.entries(author.functions)) {
        if (callName !== name && calls[callName] && same(oldCall.target, converted.function.target)) report.ambiguities.push(`hook ${name}: call ${callName} shares its target without explicit bypassWith pairing`);
      }
    }
  }
  let bundle;
  try { bundle = normalizeFunctions(pkg.name, author); }
  catch (e) { report.ambiguities.push(`v2 normalizer: ${(e as Error).message}`); }
  if (bundle) {
    report.permissions = summarizeFunctions(bundle).permissions;
    for (const f of bundle.functions) {
      const hook = Object.keys(hooks).find(n => (obj(hooks[n])?.bypassWith ?? n) === f.localName);
      const hookDecl = hook ? obj(hooks[hook]) : undefined;
      const oldExposeCtx = obj(hookDecl?.expose)?.ctx;
      const oldReceiverAs = obj(hookDecl?.receiver)?.as;
      report.entries.push({
        ...(calls[f.localName] ? { oldCall: f.localName } : {}),
        ...(hook ? { oldHook: hook } : {}),
        ...(valid(oldExposeCtx) ? { oldExposeCtx } : {}),
        ...(valid(oldReceiverAs) ? { oldReceiverAs } : {}),
        canonicalId: f.canonicalId,
        validatorStages: { ...(Object.keys(f.target.candidateValidate).length ? { candidate: f.target.candidateValidate } : {}), target: f.target.targetValidate },
        surfaces: f.policy.surfaces,
        mutates: f.abi.parameters.some(p => p.mutable.length > 0),
        suppression: f.policy.suppression,
        policyId: f.policy.id,
        compatibilityAdapter: 'none',
        ...(hookDecl?.bypassWith ? { bypassWith: f.localName } : {}),
      });
    }
  }
  report.output = author;
  report.generatedDeclarations = ['gamedata.d.ts', 'hooks.d.ts'].map(n => join(dir, '.s2script', n)).filter(existsSync);
  for (const file of report.generatedDeclarations) {
    try { report.generatedDeclarationHashes[file] = createHash('sha256').update(readFileSync(file)).digest('hex'); }
    catch (e) { report.ambiguities.push(`${file}: generated declaration could not be read: ${(e as Error).message}`); }
  }
  report.recommendedEdits.push(`package.json: remove s2script.gamedata and authored engine:calls/engine:hooks permissions after adopting ${destination}`);
  if (report.generatedDeclarations.length) report.recommendedEdits.push('generated declarations: regenerate after changing package metadata and source imports');
  try { scanSource(sourceFiles(dir, pkg), new Set(Object.keys(calls)), hookCtx, report); }
  catch (e) { report.ambiguities.push(`source analysis: ${(e as Error).message}`); }
  if (report.sourceReferences.length && !report.recommendedEdits.some(x => x.startsWith('plugin source'))) report.recommendedEdits.push('plugin source: manually update reported Engine.call/Engine.hook/ctx references; check null behavior and callback timing');
  return report;
}

export function migrateV1Package(pluginDir: string, options: { force?: boolean } = {}): MigrationReport {
  const report = analyzeV1Migration(pluginDir, options);
  if (report.ambiguities.length || !report.output) return report;
  const dest = report.destination;
  const temp = join(dirname(dest), `.functions.jsonc.${process.pid}.${randomUUID()}.tmp`);
  try {
    mkdirSync(dirname(dest), { recursive: true });
    const fd = openSync(temp, 'wx', 0o600);
    try { writeFileSync(fd, JSON.stringify(report.output, null, 2) + '\n'); }
    finally { closeSync(fd); }
    if (options.force) {
      if (pathExists(dest)) report.replacedFile = dest;
      renameSync(temp, dest);
    } else {
      linkSync(temp, dest); // EEXIST is atomic; no check-then-clobber race.
      unlinkSync(temp);
    }
    report.filesChanged.push(dest);
  } catch (e) {
    report.replacedFile = undefined;
    report.ambiguities.push(`${dest}: publication failed: ${(e as Error).message}`);
  } finally {
    if (existsSync(temp)) unlinkSync(temp);
  }
  return report;
}
