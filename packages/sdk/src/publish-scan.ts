/**
 * B1 (north-star §5.2): derive the manifest `publishes` name-set — and the dependency-usage
 * advisories — from CODE, off the tsc gate's own program. Receiver-typed matching (the object
 * before `.publish` / `.use` / `.tryUse` must be the SDK's `PluginContext`) keeps this exact
 * under renaming (`plugin((c) => c.publish(...))`) and immune to unrelated `.publish` methods.
 */

import ts from "typescript";

export interface PublishScan {
  /** String-literal names from `ctx.publish("name", …)`, deduped, source order. */
  publishNames: string[];
  /** `file:line` of every ctx.publish whose first arg is NOT a string literal (kills derivation). */
  dynamicPublishSites: string[];
  /** String-literal names from `ctx.use("name")` / `ctx.tryUse("name")`, deduped. */
  useNames: string[];
}

/** True when `type`'s symbol (or alias) is the SDK PluginContext. */
function isPluginContext(type: ts.Type): boolean {
  const sym = type.getSymbol() ?? type.aliasSymbol;
  return sym?.getName() === "PluginContext";
}

export function scanPluginProgram(program: ts.Program, pluginDir: string): PublishScan {
  const checker = program.getTypeChecker();
  const out: PublishScan = { publishNames: [], dynamicPublishSites: [], useNames: [] };
  const dirPrefix = pluginDir.replace(/\\/g, "/").replace(/\/+$/, "") + "/";

  for (const sf of program.getSourceFiles()) {
    if (sf.isDeclarationFile) continue;
    if (!sf.fileName.replace(/\\/g, "/").startsWith(dirPrefix)) continue;

    const visit = (node: ts.Node): void => {
      if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)) {
        const method = node.expression.name.text;
        if (method === "publish" || method === "use" || method === "tryUse") {
          const recv = checker.getTypeAtLocation(node.expression.expression);
          if (isPluginContext(recv)) {
            const arg0 = node.arguments[0];
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
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }

  out.publishNames = [...new Set(out.publishNames)];
  out.useNames = [...new Set(out.useNames)];
  return out;
}
