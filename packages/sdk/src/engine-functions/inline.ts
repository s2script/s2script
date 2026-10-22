import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import ts from 'typescript';
import type { AuthorFunction, FunctionFileV2 } from './model.ts';

/**
 * Engine functions declared inline with `Engine.fromSignature(name, declaration)` or
 * `Engine.fromVirtual(name, declaration)` from `@s2script/sdk/unsafe`.
 *
 * The build copies each declaration into the plugin's function bundle, exactly as if it had been
 * written in `gamedata/functions.jsonc`, so it gets the same validation, the same manifest entry
 * and permissions, and the same update-day treadmill checks. That is why a declaration must be a
 * literal: a value computed at run time could not be lifted, so it fails the build instead. At run
 * time the call simply returns `Engine.function(name)`.
 */
export interface InlineFunctions {
  functions: Record<string, AuthorFunction>;
  /** Where each inline declaration is, as `path:line`, for error messages. */
  sites: Record<string, string>;
}

const SKIP = new Set(['node_modules', 'dist', '.s2script', '.git']);
const SOURCE = /\.(?:ts|tsx|mts|cts)$/;
const FACTORIES = new Set(['fromSignature', 'fromVirtual']);

function sources(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir).sort()) {
    if (SKIP.has(name)) continue;
    const path = join(dir, name);
    const stat = statSync(path);
    if (stat.isDirectory()) sources(path, out);
    else if (SOURCE.test(name) && !name.endsWith('.d.ts')) out.push(path);
  }
  return out;
}

function unwrap(node: ts.Expression): ts.Expression {
  while (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isSatisfiesExpression(node)
    || ts.isTypeAssertionExpression(node)) node = node.expression;
  return node;
}

/** A JSON value from a literal expression, or an error naming what was not literal. */
function literal(node: ts.Expression, at: (n: ts.Node) => string): unknown {
  node = unwrap(node);
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return node.text;
  if (ts.isNumericLiteral(node)) return Number(node.text);
  if (ts.isPrefixUnaryExpression(node) && node.operator === ts.SyntaxKind.MinusToken && ts.isNumericLiteral(node.operand))
    return -Number(node.operand.text);
  if (node.kind === ts.SyntaxKind.TrueKeyword) return true;
  if (node.kind === ts.SyntaxKind.FalseKeyword) return false;
  if (node.kind === ts.SyntaxKind.NullKeyword) return null;
  if (ts.isArrayLiteralExpression(node)) {
    return node.elements.map(e => {
      if (ts.isSpreadElement(e) || ts.isOmittedExpression(e)) throw new Error(`${at(e)}: inline engine function declarations must be literals (no spreads or holes)`);
      return literal(e, at);
    });
  }
  if (ts.isObjectLiteralExpression(node)) {
    const out: Record<string, unknown> = {};
    for (const prop of node.properties) {
      if (!ts.isPropertyAssignment(prop)) throw new Error(`${at(prop)}: inline engine function declarations must be literals (no spreads, shorthand or methods)`);
      const name = prop.name;
      const key = ts.isIdentifier(name) || ts.isStringLiteral(name) || ts.isNumericLiteral(name) ? name.text : undefined;
      if (key === undefined) throw new Error(`${at(prop)}: inline engine function declarations must use plain property names`);
      if (Object.hasOwn(out, key)) throw new Error(`${at(prop)}: duplicate key ${JSON.stringify(key)}`);
      out[key] = literal(prop.initializer, at);
    }
    return out;
  }
  throw new Error(`${at(node)}: inline engine function declarations must be literals, because the build copies them into the manifest; move a computed declaration to gamedata/functions.jsonc`);
}

/** The local names this file binds to `@s2script/sdk/unsafe`'s `Engine`, and to the module itself. */
function bindings(file: ts.SourceFile): { engine: Set<string>; namespaces: Set<string> } {
  const engine = new Set<string>(), namespaces = new Set<string>();
  for (const statement of file.statements) {
    if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier)) continue;
    if (statement.moduleSpecifier.text !== '@s2script/sdk/unsafe') continue;
    const bound = statement.importClause?.namedBindings;
    if (!bound) continue;
    if (ts.isNamespaceImport(bound)) namespaces.add(bound.name.text);
    else for (const element of bound.elements) {
      if ((element.propertyName ?? element.name).text === 'Engine') engine.add(element.name.text);
    }
  }
  return { engine, namespaces };
}

export function extractInlineFunctions(pluginDir: string): InlineFunctions {
  const result: InlineFunctions = { functions: {}, sites: {} };
  if (!existsSync(pluginDir)) return result;
  for (const path of sources(pluginDir)) {
    const text = readFileSync(path, 'utf8');
    if (!text.includes('fromSignature') && !text.includes('fromVirtual')) continue;
    const file = ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true);
    const { engine, namespaces } = bindings(file);
    if (engine.size === 0 && namespaces.size === 0) continue;
    const at = (node: ts.Node) => {
      const { line } = file.getLineAndCharacterOfPosition(node.getStart(file));
      return `${relative(pluginDir, path)}:${line + 1}`;
    };
    const isEngine = (node: ts.Expression): boolean =>
      (ts.isIdentifier(node) && engine.has(node.text)) ||
      (ts.isPropertyAccessExpression(node) && node.name.text === 'Engine' && ts.isIdentifier(node.expression) && namespaces.has(node.expression.text));
    const visit = (node: ts.Node): void => {
      if (ts.isPropertyAccessExpression(node) && FACTORIES.has(node.name.text) && isEngine(node.expression)) {
        const factory = node.name.text;
        const call = node.parent;
        if (!ts.isCallExpression(call) || call.expression !== node)
          throw new Error(`${at(node)}: Engine.${factory} must be called directly, so the build can read its declaration`);
        const [nameArg, declArg] = call.arguments;
        if (call.arguments.length !== 2 || !nameArg || !declArg)
          throw new Error(`${at(call)}: Engine.${factory}(name, declaration) takes exactly two arguments`);
        const name = literal(nameArg, at);
        if (typeof name !== 'string') throw new Error(`${at(nameArg)}: Engine.${factory} needs a string-literal name`);
        const decl = literal(declArg, at);
        if (decl === null || typeof decl !== 'object' || Array.isArray(decl))
          throw new Error(`${at(declArg)}: Engine.${factory} needs an object-literal declaration`);
        const target = (decl as { target?: unknown }).target;
        if (target === null || typeof target !== 'object' || Array.isArray(target))
          throw new Error(`${at(declArg)}: Engine.${factory}: declaration.target must be an object`);
        const kind = (target as { kind?: unknown }).kind;
        if ('ref' in target) {
          // A shared target from gamedata/functions.jsonc; its kind is checked where it is defined.
        } else if (factory === 'fromVirtual') {
          if (kind !== undefined && kind !== 'virtual') throw new Error(`${at(declArg)}: Engine.fromVirtual needs a virtual target`);
          (target as { kind?: string }).kind = 'virtual';
        } else if (kind !== undefined && kind !== 'signature') {
          throw new Error(`${at(declArg)}: Engine.fromSignature needs a signature target; use Engine.fromVirtual for a vtable slot`);
        }
        if (Object.hasOwn(result.functions, name))
          throw new Error(`${at(call)}: engine function ${JSON.stringify(name)} is already declared inline at ${result.sites[name]}`);
        result.functions[name] = decl as AuthorFunction;
        result.sites[name] = at(call);
      }
      ts.forEachChild(node, visit);
    };
    visit(file);
  }
  return result;
}

/**
 * The plugin's function file with its inline declarations added. A name declared both inline and
 * in the file fails, as does a duplicate inline name.
 */
export function mergeInlineFunctions(file: FunctionFileV2 | undefined, inline: InlineFunctions): FunctionFileV2 | undefined {
  const names = Object.keys(inline.functions);
  if (names.length === 0) return file;
  const merged: FunctionFileV2 = file
    ? { ...file, functions: { ...(file.functions ?? {}) } }
    : { schemaVersion: 2, functions: {} };
  for (const name of names) {
    if (Object.hasOwn(merged.functions, name))
      throw new Error(`${inline.sites[name]}: engine function ${JSON.stringify(name)} is declared both inline and in gamedata/functions.jsonc`);
    merged.functions[name] = inline.functions[name]!;
  }
  return merged;
}

/** Point a normalization error at the inline declaration it is about, when it names one. */
export function locateInlineError(error: unknown, inline: InlineFunctions): Error {
  const err = error instanceof Error ? error : new Error(String(error));
  for (const [name, site] of Object.entries(inline.sites)) {
    if (err.message.startsWith(`function ${JSON.stringify(name)}`)) return new Error(`${site}: ${err.message} (declared inline)`);
  }
  return err;
}
