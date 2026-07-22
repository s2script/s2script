/**
 * no-await-in-raw-view (north-star §5.3; standing constraint: raw-live views are block-scoped and
 * cannot cross `await`). Precise "used after an await" dataflow is loop-hostile, so the rule
 * enforces the teachable superset: a raw-view-typed value may NEVER be referenced inside an
 * async function at all — copy the fields you need into plain values first. Symbol-name keyed
 * (RAW_VIEW_TYPES) so future views join with one line.
 */
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";
import { isFunctionNode } from "../plugin-factory.ts";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/s2script/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

const RAW_VIEW_TYPES: ReadonlySet<string> = new Set(["UserCmdView"]);

export const noAwaitInRawView = createRule({
  name: "no-await-in-raw-view",
  meta: {
    type: "problem",
    docs: {
      description:
        "raw tick-scoped views (UserCmdView) must not enter async code — they are dead across any await",
    },
    messages: {
      rawViewInAsync:
        "a {{type}} is a tick-scoped raw view: inside an async function it can outlive its tick and read/write nothing (or garbage). Copy the fields you need into plain values BEFORE going async.",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const services = ESLintUtils.getParserServices(context);
    const checker = services.program.getTypeChecker();

    return {
      Identifier(node: TSESTree.Identifier) {
        // Innermost enclosing function must be async. (Program.parent is null — a node outside
        // any function walks off the top and correctly yields no report.)
        let fn: TSESTree.Node | undefined | null = node.parent;
        while (fn !== undefined && fn !== null && !isFunctionNode(fn)) fn = fn.parent;
        if (fn === undefined || fn === null || !fn.async) return;

        // Skip pure type positions (import type / annotations) — they carry no runtime value.
        if (node.parent?.type === "TSTypeReference" || node.parent?.type === "ImportSpecifier") return;

        const tsNode = services.esTreeNodeToTSNodeMap.get(node);
        const t = checker.getTypeAtLocation(tsNode);
        const sym = t.getSymbol() ?? t.aliasSymbol;
        const name = sym?.getName();
        if (name !== undefined && RAW_VIEW_TYPES.has(name)) {
          context.report({ node, messageId: "rawViewInAsync", data: { type: name } });
        }
      },
    };
  },
});
