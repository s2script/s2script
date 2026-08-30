/**
 * Shared load-window locator: leftover `export default plugin(<factory>)` (imported from
 * "@s2script/sdk/plugin" or the root barrel) plus `export function OnPluginStart`.
 * Import-source matching (not scope analysis) keeps it dependency-light; shadowing `plugin`
 * between the import and the export is not a pattern worth chasing.
 */
import type { TSESTree } from "@typescript-eslint/utils";

export type FactoryNode =
  | TSESTree.ArrowFunctionExpression
  | TSESTree.FunctionExpression
  | TSESTree.FunctionDeclaration;

export function findFactory(ast: TSESTree.Program): FactoryNode | null {
  let pluginLocal: string | null = null;
  for (const stmt of ast.body) {
    if (
      stmt.type === "ImportDeclaration" &&
      (stmt.source.value === "@s2script/sdk/plugin" || stmt.source.value === "@s2script/sdk")
    ) {
      for (const spec of stmt.specifiers) {
        if (
          spec.type === "ImportSpecifier" &&
          spec.imported.type === "Identifier" &&
          spec.imported.name === "plugin"
        ) {
          pluginLocal = spec.local.name;
        }
      }
    }
  }
  if (pluginLocal === null) return null;

  for (const stmt of ast.body) {
    if (stmt.type !== "ExportDefaultDeclaration") continue;
    const d = stmt.declaration;
    if (d.type === "CallExpression" && d.callee.type === "Identifier" && d.callee.name === pluginLocal) {
      const a = d.arguments[0];
      if (a !== undefined && (a.type === "ArrowFunctionExpression" || a.type === "FunctionExpression")) {
        return a;
      }
    }
  }
  return null;
}

/** `export function OnPluginStart` / `export async function OnPluginStart`. */
export function findOnPluginStart(ast: TSESTree.Program): FactoryNode | null {
  for (const stmt of ast.body) {
    if (stmt.type !== "ExportNamedDeclaration") continue;
    const d = stmt.declaration;
    if (d && d.type === "FunctionDeclaration" && d.id?.name === "OnPluginStart") {
      return d;
    }
    if (d && d.type === "VariableDeclaration") {
      for (const decl of d.declarations) {
        if (
          decl.id.type === "Identifier" &&
          decl.id.name === "OnPluginStart" &&
          decl.init &&
          (decl.init.type === "ArrowFunctionExpression" || decl.init.type === "FunctionExpression")
        ) {
          return decl.init;
        }
      }
    }
  }
  return null;
}

/** Load-window body: leftover `plugin()` factory, or `export function OnPluginStart`. */
export function findLoadWindow(ast: TSESTree.Program): FactoryNode | null {
  return findFactory(ast) ?? findOnPluginStart(ast);
}

/** True for any function-ish AST node (the nesting boundary the rules care about). */
export function isFunctionNode(
  n: TSESTree.Node,
): n is TSESTree.ArrowFunctionExpression | TSESTree.FunctionExpression | TSESTree.FunctionDeclaration {
  return (
    n.type === "ArrowFunctionExpression" ||
    n.type === "FunctionExpression" ||
    n.type === "FunctionDeclaration"
  );
}
