/**
 * no-bigint-in-interface-payloads (north-star §5.3; Slice 5B.4 lock): inter-plugin values cross
 * as JSON — a BigInt anywhere throws inside the marshaller and the WHOLE payload silently drops.
 * Flags the three statically-visible wire crossings:
 *   (a) PublishHandle.emit(event, payload)          — forward payload
 *   (b) <InterfaceHandle>.method(args...)           — consumer -> producer args
 *   (c) ctx.publish(name, impl) method return types — producer -> consumer returns
 * Fix: carry 64-bit as a decimal string (String(v)).
 */
import ts from "typescript";
import { ESLintUtils, type TSESTree } from "@typescript-eslint/utils";

const createRule = ESLintUtils.RuleCreator(
  (name) =>
    `https://github.com/s2script/s2script/blob/main/packages/eslint-plugin/docs/${name}.md`,
);

function symbolName(type: ts.Type): string | undefined {
  return (type.aliasSymbol ?? type.getSymbol())?.getName();
}

function containsBigInt(checker: ts.TypeChecker, type: ts.Type, depth = 0): boolean {
  if (depth > 3) return false;
  if (type.flags & ts.TypeFlags.BigIntLike) return true;
  if (type.isUnionOrIntersection()) {
    return type.types.some((t) => containsBigInt(checker, t, depth + 1));
  }
  const numIndex = checker.getIndexTypeOfType(type, ts.IndexKind.Number); // arrays/tuples
  if (numIndex !== undefined && containsBigInt(checker, numIndex, depth + 1)) return true;
  if (type.getFlags() & ts.TypeFlags.Object) {
    for (const prop of type.getProperties()) {
      const decl = prop.valueDeclaration ?? prop.declarations?.[0];
      if (decl === undefined) continue;
      if (containsBigInt(checker, checker.getTypeOfSymbolAtLocation(prop, decl), depth + 1)) {
        return true;
      }
    }
  }
  return false;
}

export const noBigintInInterfacePayloads = createRule({
  name: "no-bigint-in-interface-payloads",
  meta: {
    type: "problem",
    docs: {
      description:
        "BigInt cannot cross the inter-plugin JSON wire — the whole payload is silently dropped; carry 64-bit as a decimal string",
    },
    messages: {
      bigintPayload:
        "BigInt cannot cross the inter-plugin wire: JSON marshalling throws and the WHOLE payload/call is silently dropped (Slice 5B.4). Carry 64-bit values as decimal strings — String(v) here, BigInt(s) on the far side.",
    },
    schema: [],
  },
  defaultOptions: [],
  create(context) {
    const services = ESLintUtils.getParserServices(context);
    const checker = services.program.getTypeChecker();

    const typeOf = (node: TSESTree.Node): ts.Type =>
      checker.getTypeAtLocation(services.esTreeNodeToTSNodeMap.get(node));

    return {
      CallExpression(node: TSESTree.CallExpression) {
        if (node.callee.type !== "MemberExpression" || node.callee.property.type !== "Identifier") {
          return;
        }
        const method = node.callee.property.name;
        const recvType = typeOf(node.callee.object);
        const recvName = symbolName(recvType);

        // (a) PublishHandle.emit(event, payload) — check the payload arg.
        if (recvName === "PublishHandle" && method === "emit") {
          const payload = node.arguments[1];
          if (payload !== undefined && containsBigInt(checker, typeOf(payload))) {
            context.report({ node: payload, messageId: "bigintPayload" });
          }
          return;
        }

        // (b) any method on an InterfaceHandle — check every argument.
        if (recvName === "InterfaceHandle" && method !== "on") {
          for (const arg of node.arguments) {
            if (containsBigInt(checker, typeOf(arg))) {
              context.report({ node: arg, messageId: "bigintPayload" });
            }
          }
          return;
        }

        // (c) ctx.publish(name, impl) — check each impl method's RETURN type.
        if (recvName === "PluginContext" && method === "publish") {
          const impl = node.arguments[1];
          if (impl === undefined) return;
          const implType = typeOf(impl);
          for (const prop of implType.getProperties()) {
            const decl = prop.valueDeclaration ?? prop.declarations?.[0];
            if (decl === undefined) continue;
            const propType = checker.getTypeOfSymbolAtLocation(prop, decl);
            for (const sig of propType.getCallSignatures()) {
              if (containsBigInt(checker, sig.getReturnType())) {
                context.report({ node: impl, messageId: "bigintPayload" });
                return;
              }
            }
          }
        }
      },
    };
  },
});
