/**
 * no-ctx-escape — THE one escape the type system cannot catch (L1 design §4.2/B2 §5.3): the
 * factory's `ctx` (or a member destructured from it) captured inside a nested function. Such a
 * reference runs after the plugin reaches Active, when the ctx is sealed — at runtime it throws
 * "registration outside the load window"; this rule makes it a red squiggle instead.
 * Scope handles from ctx.createScope() are intentionally NOT flagged (late driving is their job).
 *
 * Accepted residual (locked decision #7): the rule does NOT chase `ctx` passed as an argument to
 * another function — the runtime seal backstops that path.
 */
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";
import { findFactory, isFunctionNode, type FactoryNode } from "../plugin-factory.ts";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/GabeHirakawa/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

/** Every binding name introduced by the factory's first parameter pattern. */
function param0Names(param: TSESTree.Parameter): Set<string> {
  const names = new Set<string>();
  const collect = (p: TSESTree.Node): void => {
    switch (p.type) {
      case "Identifier": names.add(p.name); break;
      case "ObjectPattern":
        for (const prop of p.properties) collect(prop.type === "Property" ? prop.value : prop.argument);
        break;
      case "ArrayPattern":
        for (const el of p.elements) if (el !== null) collect(el);
        break;
      case "AssignmentPattern": collect(p.left); break;
      case "RestElement": collect(p.argument); break;
      default: break;
    }
  };
  collect(param);
  return names;
}

export const noCtxEscape = createRule({
  name: "no-ctx-escape",
  meta: {
    type: "problem",
    docs: {
      description:
        "the plugin factory's ctx (and members destructured from it) is load-window-only; referencing it inside a nested function defers the use past the seal",
    },
    messages: {
      escaped:
        "'{{name}}' escapes the load window: it is referenced inside a nested function, which runs after the plugin is Active and the ctx is sealed (the registration will throw). Register during the factory run, or allocate a Scope with ctx.createScope() at load and drive that instead.",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const factory: FactoryNode | null = findFactory(context.sourceCode.ast);
    if (factory === null || factory.params.length === 0) return {};
    const names = param0Names(factory.params[0]);

    // The ctx bindings are declared BY the factory node itself.
    const ctxVars = context.sourceCode
      .getDeclaredVariables(factory)
      .filter((v) => names.has(v.name));

    return {
      "Program:exit"() {
        for (const v of ctxVars) {
          for (const ref of v.references) {
            // Innermost enclosing function of the reference.
            let n: TSESTree.Node | undefined = ref.identifier.parent;
            while (n !== undefined && n !== factory && !isFunctionNode(n)) n = n.parent;
            if (n !== undefined && n !== factory) {
              context.report({
                node: ref.identifier,
                messageId: "escaped",
                data: { name: v.name },
              });
            }
          }
        }
      },
    };
  },
});
