/**
 * Shared factory locator: finds the function passed to `plugin(...)` in
 * `export default plugin(<factory>)`, where `plugin` was imported from "@s2script/sdk/plugin".
 * Import-source matching (not scope analysis) keeps it dependency-light; shadowing `plugin`
 * between the import and the export is not a pattern worth chasing.
 */
import type { TSESTree } from "@typescript-eslint/utils";

export type FactoryNode = TSESTree.ArrowFunctionExpression | TSESTree.FunctionExpression;

export function findFactory(ast: TSESTree.Program): FactoryNode | null {
  let pluginLocal: string | null = null;
  for (const stmt of ast.body) {
    if (stmt.type === "ImportDeclaration" && stmt.source.value === "@s2script/sdk/plugin") {
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
