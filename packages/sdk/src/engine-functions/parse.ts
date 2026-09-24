import ts from 'typescript';
import { stripJsonComments } from '../gamedata/jsonc.ts';
import type { FunctionFileV2 } from './model.ts';

/** Parse before materializing values: JSON.parse alone silently discards duplicate keys. */
export function parseFunctionFile(path: string, text: string | undefined): FunctionFileV2 | undefined {
  if (text === undefined) return undefined;
  const stripped = stripJsonComments(text);
  const source = ts.parseJsonText(path, stripped) as ts.JsonSourceFile & { parseDiagnostics?: readonly ts.Diagnostic[] };
  if (source.parseDiagnostics?.length) {
    const first = source.parseDiagnostics[0]!;
    throw new Error(`${path}: invalid JSONC at ${first.start}: ${ts.flattenDiagnosticMessageText(first.messageText, '\n')}`);
  }
  const visit = (node: ts.Node): void => {
    if (ts.isObjectLiteralExpression(node)) {
      const seen = new Set<string>();
      for (const prop of node.properties) {
        if (!ts.isPropertyAssignment(prop) || !ts.isStringLiteral(prop.name)) {
          throw new Error(`${path}: invalid JSON property at ${prop.pos}`);
        }
        const key = prop.name.text;
        if (seen.has(key)) throw new Error(`${path}: duplicate JSON key ${JSON.stringify(key)} at ${prop.name.pos}`);
        seen.add(key);
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  // JSON.parse remains the strict syntax gate, including the v1 trailing-comma rejection.
  let parsed: unknown;
  try { parsed = JSON.parse(stripped); }
  catch (e) { throw new Error(`${path}: invalid JSONC: ${(e as Error).message}`); }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error(`${path}: root must be an object`);
  return parsed as FunctionFileV2;
}
