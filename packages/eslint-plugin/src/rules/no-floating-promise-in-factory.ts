/**
 * no-floating-promise-in-factory (north-star §5.3): the load window closes when the factory's
 * promise settles — an unawaited promise started in the factory races arm-at-Active (init not
 * done when handlers arm; a failure can't fail the load). Scoped to statements whose innermost
 * function IS the factory: handlers/helpers are outside this rule's contract.
 */
import ts from "typescript";
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";
import { findFactory, isFunctionNode, type FactoryNode } from "../plugin-factory.ts";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/GabeHirakawa/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

function isThenable(checker: ts.TypeChecker, type: ts.Type): boolean {
  const parts = type.isUnion() ? type.types : [type];
  for (const part of parts) {
    const then = part.getProperty("then");
    if (then === undefined) continue;
    const decl = then.valueDeclaration ?? then.declarations?.[0];
    if (decl === undefined) continue;
    if (checker.getTypeOfSymbolAtLocation(then, decl).getCallSignatures().length > 0) return true;
  }
  return false;
}

export const noFloatingPromiseInFactory = createRule({
  name: "no-floating-promise-in-factory",
  meta: {
    type: "problem",
    docs: {
      description:
        "a promise discarded inside the plugin factory races arm-at-Active; await it or void it explicitly",
    },
    messages: {
      floating:
        "floating promise in the plugin factory: the load window closes when the factory settles, so this async work is not covered by it — `await` it (or `void` it only if it genuinely must not gate the load).",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const factory: FactoryNode | null = findFactory(context.sourceCode.ast);
    if (factory === null) return {};
    const services = ESLintUtils.getParserServices(context);
    const checker = services.program.getTypeChecker();

    return {
      ExpressionStatement(node: TSESTree.ExpressionStatement) {
        // Innermost enclosing function must be the factory itself. (Program.parent is null —
        // a top-level statement walks off the top and is correctly not the factory.)
        let fn: TSESTree.Node | undefined | null = node.parent;
        while (fn !== undefined && fn !== null && fn !== factory && !isFunctionNode(fn)) fn = fn.parent;
        if (fn !== factory) return;

        const expr = node.expression;
        if (expr.type === "AwaitExpression" || expr.type === "AssignmentExpression") return;
        if (expr.type === "UnaryExpression" && expr.operator === "void") return;

        const tsNode = services.esTreeNodeToTSNodeMap.get(expr);
        if (isThenable(checker, checker.getTypeAtLocation(tsNode))) {
          context.report({ node, messageId: "floating" });
        }
      },
    };
  },
});
