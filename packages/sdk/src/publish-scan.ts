/**
 * B1 (north-star §5.2): derive the manifest `publishes` name-set — and the dependency-usage
 * advisories — from CODE, off the tsc gate's own program. Receiver-typed matching (the object
 * before `.publish` / `.use` / `.tryUse` must be the SDK's `PluginContext`) keeps this exact
 * under renaming (`plugin((c) => c.publish(...))`) and immune to unrelated `.publish` methods.
 *
 * Free `publish` / `use` / `tryUse` (imported from `@s2script/sdk/plugin`) are collected the same
 * way, keyed off the aliased symbol's declaration file (`plugin.d.ts`) so a local helper named
 * `publish` is not a false positive. `importNames` are static import/re-export specifiers that are
 * not `@s2script/*` and not relative — producer-as-import (`import { greet } from "@demo/greeter"`).
 */

import ts from "typescript";

export interface PublishScan {
  /** String-literal names from `ctx.publish("name", …)` / free `publish("name", …)`, deduped, source order. */
  publishNames: string[];
  /** `file:line` of every publish whose first arg is NOT a string literal (kills derivation). */
  dynamicPublishSites: string[];
  /** String-literal names from `ctx.use` / `ctx.tryUse` / free `use` / `tryUse`, deduped. */
  useNames: string[];
  /** Non-`@s2script/*`, non-relative static import/re-export specifiers, deduped. */
  importNames: string[];
}

/** True when `type`'s symbol (or alias) is the SDK PluginContext. */
function isPluginContext(type: ts.Type): boolean {
  const sym = type.getSymbol() ?? type.aliasSymbol;
  return sym?.getName() === "PluginContext";
}

/** True when `node` (an identifier) aliases a symbol declared in packages/sdk/plugin.d.ts. */
function isFromPluginDts(checker: ts.TypeChecker, node: ts.Node): boolean {
  let sym = checker.getSymbolAtLocation(node);
  if (sym === undefined) return false;
  if (sym.flags & ts.SymbolFlags.Alias) {
    const aliased = checker.getAliasedSymbol(sym);
    if (aliased !== undefined) sym = aliased;
  }
  for (const d of sym.declarations ?? []) {
    const f = d.getSourceFile().fileName.replace(/\\/g, "/");
    if (f.endsWith("/plugin.d.ts")) return true;
  }
  return false;
}

function recordPublishOrUse(
  out: PublishScan,
  method: string,
  arg0: ts.Expression | undefined,
  sf: ts.SourceFile,
  node: ts.Node,
): void {
  if (method === "publish") {
    if (arg0 !== undefined && ts.isStringLiteralLike(arg0)) {
      out.publishNames.push(arg0.text);
    } else {
      const { line } = sf.getLineAndCharacterOfPosition(node.getStart());
      out.dynamicPublishSites.push(`${sf.fileName}:${line + 1}`);
    }
  } else if (arg0 !== undefined && ts.isStringLiteralLike(arg0)) {
    out.useNames.push(arg0.text);
  }
}

function recordImportSpecifier(out: PublishScan, spec: string): void {
  if (spec.startsWith(".") || spec.startsWith("@s2script/")) return;
  out.importNames.push(spec);
}

export function scanPluginProgram(program: ts.Program, pluginDir: string): PublishScan {
  const checker = program.getTypeChecker();
  const out: PublishScan = { publishNames: [], dynamicPublishSites: [], useNames: [], importNames: [] };
  const dirPrefix = pluginDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";

  for (const sf of program.getSourceFiles()) {
    if (sf.isDeclarationFile) continue;
    if (!sf.fileName.replace(/\\/g, "/").startsWith(dirPrefix)) continue;

    const visit = (node: ts.Node): void => {
      if (
        (ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) &&
        node.moduleSpecifier !== undefined &&
        ts.isStringLiteralLike(node.moduleSpecifier)
      ) {
        recordImportSpecifier(out, node.moduleSpecifier.text);
      }

      if (ts.isCallExpression(node)) {
        if (ts.isPropertyAccessExpression(node.expression)) {
          const method = node.expression.name.text;
          if (method === "publish" || method === "use" || method === "tryUse") {
            const recv = checker.getTypeAtLocation(node.expression.expression);
            if (isPluginContext(recv)) {
              recordPublishOrUse(out, method, node.arguments[0], sf, node);
            }
          }
        } else if (ts.isIdentifier(node.expression)) {
          const name = node.expression.text;
          if ((name === "publish" || name === "use" || name === "tryUse") && isFromPluginDts(checker, node.expression)) {
            recordPublishOrUse(out, name, node.arguments[0], sf, node);
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }

  out.publishNames = [...new Set(out.publishNames)];
  out.useNames = [...new Set(out.useNames)];
  out.importNames = [...new Set(out.importNames)];
  return out;
}
