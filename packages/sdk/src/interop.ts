/** Protocol 2 metadata is derived exclusively from the resolved, hashed declaration. */
import ts from "typescript";
import { createHash } from "node:crypto";
import { pluginApiName } from "./publish-scan.ts";
import { resolve } from "node:path";
import { sharedProgramOptions } from "./tsconfig-shared.ts";
export type WireSchema = { kind: string; [key: string]: unknown };
export interface ContractMetadata {
  version: 1;
  methods: Record<
    string,
    { args: { schema: WireSchema; optional: boolean }[]; result: WireSchema }
  >;
  forwards: Record<
    string,
    {
      kind: "notification" | "hook" | "transform";
      payload: WireSchema;
      writable?: string[];
    }
  >;
}
export interface WireContract {
  metadata: ContractMetadata;
  sha256: string;
}
/** RFC 8785 JCS: ECMAScript finite numbers/escaping and UTF-16 code-unit key order. */
export function canonical(value: unknown): string {
  if (typeof value === "string") {
    // JSON.stringify would escape lone surrogates, but JCS requires rejecting them.
    if (
      /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(
        value
      )
    )
      throw new Error(
        "canonical metadata requires well-formed Unicode strings"
      );
    return JSON.stringify(value);
  }
  if (typeof value === "number" && !Number.isFinite(value))
    throw new Error("canonical metadata requires finite numbers");
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object")
    return `{${Object.keys(value)
      .sort()
      .map(
        (k) =>
          `${canonical(k)}:${canonical((value as Record<string, unknown>)[k])}`
      )
      .join(",")}}`;
  if (value === null || typeof value === "boolean" || typeof value === "number")
    return JSON.stringify(value);
  throw new Error("canonical metadata requires JSON values");
}
export function extractContract(
  path: string,
  packagesDir: string
): WireContract {
  const program = ts.createProgram([path], {
    ...sharedProgramOptions(ts),
    skipLibCheck: false,
    baseUrl: packagesDir,
    paths: { "@s2script/sdk/*": ["sdk/*.d.ts"] },
  });
  const checker = program.getTypeChecker();
  const sf = program.getSourceFile(path)!;
  const fail = (node: ts.Node, reason: string): never => {
    const pos = sf.getLineAndCharacterOfPosition(node.getStart(sf));
    throw new Error(
      `${path}:${pos.line + 1}:${
        pos.character + 1
      }: InterfaceContractError: ${reason}`
    );
  };
  const visitImports = (node: ts.Node): void => {
    if (ts.isImportDeclaration(node)) {
      const module = ts.isStringLiteral(node.moduleSpecifier)
        ? node.moduleSpecifier.text
        : "";
      const bindings = node.importClause?.namedBindings;
      const allowed =
        module === "@s2script/sdk/interfaces"
          ? ["Notification", "Hook", "Transform"]
          : module === "@s2script/sdk/entity"
          ? ["EntityRef"]
          : [];
      if (
        !bindings ||
        !ts.isNamedImports(bindings) ||
        node.importClause?.name ||
        !bindings.elements.every((e) =>
          allowed.includes((e.propertyName ?? e.name).text)
        )
      )
        fail(
          node,
          "contracts must be self-contained; only SDK Notification, Hook, Transform and EntityRef imports are supported"
        );
    }
    if (
      (ts.isExportDeclaration(node) && node.moduleSpecifier) ||
      ts.isImportTypeNode(node) ||
      ts.isImportEqualsDeclaration(node) ||
      ts.isModuleDeclaration(node)
    )
      fail(
        node,
        "external declarations and module augmentations are not supported in a contract"
      );
    ts.forEachChild(node, visitImports);
  };
  visitImports(sf);
  if (sf.referencedFiles.length || sf.typeReferenceDirectives.length)
    fail(sf, "external declaration references are not supported");
  const diags = [
    ...program.getSyntacticDiagnostics(sf),
    ...program.getSemanticDiagnostics(sf),
  ];
  if (diags.length)
    fail(sf, ts.flattenDiagnosticMessageText(diags[0].messageText, "\n"));
  const module = checker.getSymbolAtLocation(sf);
  const symbol =
    module &&
    checker.getExportsOfModule(module).find((s) => s.name === "Contract");
  if (!symbol) fail(sf, "protocol 2 requires an exported Contract");
  const contract = checker.getDeclaredTypeOfSymbol(symbol!);
  const prop = (type: ts.Type, name: string, node: ts.Node) => {
    const p = type.getProperty(name);
    if (!p) fail(node, `missing ${name}`);
    return checker.getTypeOfSymbolAtLocation(p!, p!.valueDeclaration ?? node);
  };
  for (const name of ["methods", "forwards"]) {
    const member = prop(contract, name, sf);
    if (
      !(member.flags & ts.TypeFlags.Object) ||
      member.getCallSignatures().length ||
      checker.getIndexInfosOfType(member).length
    ) {
      fail(sf, `${name} must be a finite object record`);
    }
  }
  const stack = new Set<ts.Type>();
  const schema = (
    type: ts.Type,
    node: ts.Node,
    voidAllowed = false
  ): WireSchema => {
    const f = type.flags;
    if (
      f & ts.TypeFlags.Any ||
      f & ts.TypeFlags.Unknown ||
      f & ts.TypeFlags.Never
    )
      fail(node, `unsupported wire type ${checker.typeToString(type)}`);
    if (f & ts.TypeFlags.Void && voidAllowed) return { kind: "void" };
    if (f & ts.TypeFlags.Null) return { kind: "null" };
    if (f & ts.TypeFlags.StringLiteral)
      return { kind: "literal", value: (type as ts.StringLiteralType).value };
    if (f & ts.TypeFlags.NumberLiteral)
      return { kind: "literal", value: (type as ts.NumberLiteralType).value };
    if (f & ts.TypeFlags.BooleanLiteral)
      return { kind: "literal", value: checker.typeToString(type) === "true" };
    if (f & ts.TypeFlags.String) return { kind: "string" };
    if (f & ts.TypeFlags.Number) return { kind: "number" };
    if (f & ts.TypeFlags.Boolean) return { kind: "boolean" };
    if (stack.has(type)) fail(node, "recursive wire types are unsupported");
    stack.add(type);
    try {
      if (type.isUnion()) {
        const variants = type.types.map((t) => schema(t, node));
        const objects = variants.filter((v) => v.kind === "object");
        if (
          objects.length > 1 &&
          !Object.keys(objects[0].fields as object).some((k) =>
            objects.every(
              (o) =>
                (
                  o.fields as Record<
                    string,
                    { schema: WireSchema; optional: boolean }
                  >
                )[k]?.schema.kind === "literal" &&
                !(o.fields as Record<string, { optional: boolean }>)[k].optional
            )
          )
        )
          fail(node, "object unions require a literal discriminant");
        return { kind: "union", variants };
      }
      if (checker.isArrayType(type))
        return {
          kind: "array",
          item: schema(
            checker.getTypeArguments(type as ts.TypeReference)[0],
            node
          ),
        };
      if (
        !(f & ts.TypeFlags.Object) ||
        checker.isTupleType(type) ||
        type.getCallSignatures().length ||
        checker.getIndexInfosOfType(type).length
      )
        fail(node, `unsupported wire type ${checker.typeToString(type)}`);
      const sym = type.getSymbol();
      if (
        sym?.name === "EntityRef" &&
        sym.declarations?.some(
          (d) =>
            d.getSourceFile().fileName ===
            resolve(packagesDir, "sdk/entity.d.ts")
        )
      )
        return { kind: "entityRef" };
      if (
        sym?.declarations?.some(
          (d) => ts.isClassDeclaration(d) || d.getSourceFile() !== sf
        )
      )
        fail(
          node,
          "arbitrary class instances and imported domain types are unsupported"
        );
      const fields: Record<string, { schema: WireSchema; optional: boolean }> =
        Object.create(null);
      for (const p of type
        .getProperties()
        .sort((a, b) => a.name.localeCompare(b.name))) {
        const decl = p.valueDeclaration ?? node;
        if (p.name === "__s2ref")
          fail(decl, "__s2ref is reserved for the EntityRef wire encoding");
        const optional = !!(p.flags & ts.SymbolFlags.Optional);
        let t = checker.getTypeOfSymbolAtLocation(p, decl);
        if (optional && t.isUnion()) {
          const parts = t.types.filter(
            (t) => !(t.flags & ts.TypeFlags.Undefined)
          );
          if (parts.length === 1) t = parts[0];
          else {
            fields[p.name] = {
              schema: {
                kind: "union",
                variants: parts.map((t) => schema(t, decl)),
              },
              optional,
            };
            continue;
          }
        }
        fields[p.name] = { schema: schema(t, decl), optional };
      }
      return { kind: "object", fields };
    } finally {
      stack.delete(type);
    }
  };
  const methods: ContractMetadata["methods"] = Object.create(null);
  for (const p of prop(contract, "methods", sf).getProperties()) {
    const node = p.valueDeclaration ?? sf,
      type = checker.getTypeOfSymbolAtLocation(p, node),
      sigs = type.getCallSignatures();
    const scanner = ts.createScanner(
      ts.ScriptTarget.ESNext,
      false,
      ts.LanguageVariant.Standard,
      p.name
    );
    const identifier =
      scanner.scan() === ts.SyntaxKind.Identifier &&
      scanner.scan() === ts.SyntaxKind.EndOfFileToken;
    if (!identifier || ["on", "off", "then"].includes(p.name))
      fail(node, `unsupported or reserved method name ${p.name}`);
    const exported = checker
      .getExportsOfModule(module!)
      .find((s) => s.name === p.name);
    if (
      exported &&
      (!checker.isTypeAssignableTo(
        checker.getTypeOfSymbolAtLocation(
          exported,
          exported.valueDeclaration ?? sf
        ),
        type
      ) ||
        !checker.isTypeAssignableTo(
          type,
          checker.getTypeOfSymbolAtLocation(
            exported,
            exported.valueDeclaration ?? sf
          )
        ))
    )
      fail(node, `export ${p.name} must agree with Contract.methods`);
    if (
      sigs.length !== 1 ||
      sigs[0].typeParameters?.length ||
      p.flags & ts.SymbolFlags.Optional
    )
      fail(node, "methods require one non-generic synchronous signature");
    const sig = sigs[0];
    methods[p.name] = {
      args: sig.parameters.map((p) => {
        const d = p.valueDeclaration as ts.ParameterDeclaration;
        if (d.dotDotDotToken) fail(d, "rest parameters are unsupported");
        let t = checker.getTypeOfSymbolAtLocation(p, d);
        if (d.questionToken && t.isUnion()) {
          const parts = t.types.filter(
            (t) => !(t.flags & ts.TypeFlags.Undefined)
          );
          if (parts.length === 1) t = parts[0];
          else fail(d, "optional union arguments are unsupported");
        }
        return { schema: schema(t, d), optional: !!d.questionToken };
      }),
      result: schema(checker.getReturnTypeOfSignature(sig), node, true),
    };
  }
  const forwards: ContractMetadata["forwards"] = Object.create(null);
  for (const p of prop(contract, "forwards", sf).getProperties()) {
    const node = p.valueDeclaration ?? sf,
      type = checker.getTypeOfSymbolAtLocation(p, node);
    const descriptors = [
      ["notification", "__notificationPayload"],
      ["hook", "__hookPayload"],
      ["transform", "__transformPayload"],
    ] as const;
    const descriptor = descriptors.filter(([, key]) => type.getProperty(key));
    if (descriptor.length !== 1)
      fail(
        node,
        "only SDK Notification<P>, Hook<P> and Transform<P,W> forwards are supported"
      );
    const [kind, key] = descriptor[0];
    const brand = type.getProperty(key)!;
    if (
      !brand.declarations?.some(
        (d) =>
          d.getSourceFile().fileName ===
          resolve(packagesDir, "sdk/interfaces.d.ts")
      )
    )
      fail(node, "forward descriptors must come from the SDK");
    const payload = schema(
      checker.getTypeOfSymbolAtLocation(brand, node),
      node
    );
    if (kind === "transform") {
      if (payload.kind !== "object")
        fail(node, "Transform requires a finite object payload");
      const writableType = prop(type, "__transformWritable", node);
      const keys =
        writableType.flags & ts.TypeFlags.Never
          ? []
          : writableType.isUnion()
          ? writableType.types
          : [writableType];
      const writable = keys
        .map((t) => {
          if (!(t.flags & ts.TypeFlags.StringLiteral))
            fail(node, "Transform writable keys must be string field names");
          const key = (t as ts.StringLiteralType).value;
          if (!Object.hasOwn(payload.fields as object, key))
            fail(node, "Transform writable key is absent from payload");
          return key;
        })
        .sort();
      forwards[p.name] = { kind, payload, writable };
    } else forwards[p.name] = { kind, payload };
  }
  const metadata: ContractMetadata = { version: 1, methods, forwards };
  return {
    metadata,
    sha256: createHash("sha256").update(canonical(metadata)).digest("hex"),
  };
}

/** Inspect locally available initializers as well as their annotations: a void annotation
 * can otherwise erase an async/value result under TS's callback assignment rules. */
function implementationExpression(
  checker: ts.TypeChecker,
  expression: ts.Node,
  seen = new Set<ts.Node>()
): ts.Node {
  if (seen.has(expression)) return expression;
  seen.add(expression);
  const propertyValue = (
    receiver: ts.Node,
    name: string
  ): ts.Node | undefined => {
    const object = implementationExpression(checker, receiver, seen);
    const declaration = checker
      .getTypeAtLocation(object)
      .getProperty(name)?.valueDeclaration;
    return declaration && (declarationValue(declaration) ?? declaration);
  };
  const declarationValue = (declaration: ts.Node): ts.Node | undefined => {
    if (seen.has(declaration)) return undefined;
    seen.add(declaration);
    if (
      (ts.isVariableDeclaration(declaration) ||
        ts.isPropertyAssignment(declaration)) &&
      declaration.initializer
    )
      return implementationExpression(checker, declaration.initializer, seen);
    if (ts.isShorthandPropertyAssignment(declaration)) {
      const value =
        checker.getShorthandAssignmentValueSymbol(
          declaration
        )?.valueDeclaration;
      return value && declarationValue(value);
    }
    if (
      ts.isBindingElement(declaration) &&
      ts.isObjectBindingPattern(declaration.parent)
    ) {
      // Recover from the source object's initializer, not the binding's annotated void type.
      // The parent can itself be a BindingElement for nested object destructuring.
      const object = declarationValue(declaration.parent.parent);
      const name = declaration.propertyName ?? declaration.name;
      if (object && (ts.isIdentifier(name) || ts.isStringLiteral(name)))
        return propertyValue(object, name.text);
    }
    return undefined;
  };
  if (ts.isParenthesizedExpression(expression))
    return implementationExpression(checker, expression.expression, seen);
  if (ts.isPropertyAccessExpression(expression))
    return (
      propertyValue(expression.expression, expression.name.text) ?? expression
    );
  if (
    ts.isElementAccessExpression(expression) &&
    ts.isStringLiteralLike(expression.argumentExpression)
  )
    return (
      propertyValue(
        expression.expression,
        expression.argumentExpression.text
      ) ?? expression
    );
  let symbol = checker.getSymbolAtLocation(expression);
  if (symbol && symbol.flags & ts.SymbolFlags.Alias)
    symbol = checker.getAliasedSymbol(symbol);
  return (
    (symbol?.valueDeclaration && declarationValue(symbol.valueDeclaration)) ??
    expression
  );
}

/** The checker validates call sites against authoritative contracts even if overload fallback,
 * caller-supplied generics, any, or a local augmentation would otherwise hide a mismatch. */
export function checkInteropCalls(
  program: ts.Program,
  paths: Record<string, string>,
  publishes: Set<string>,
  generatedPath: string,
  pluginDir: string,
  dependencies: Set<string>,
  optionalDependencies: Set<string>
): ts.Diagnostic[] {
  const checker = program.getTypeChecker(),
    out: ts.Diagnostic[] = [];
  const report = (node: ts.Node, message: string) =>
    out.push({
      category: ts.DiagnosticCategory.Error,
      code: 92002,
      file: node.getSourceFile(),
      start: node.getStart(),
      length: node.getWidth(),
      messageText: `InterfaceContractError: ${message}`,
    });
  const contracts = new Map<string, ts.Type>();
  for (const [name, path] of Object.entries(paths)) {
    const sf = program.getSourceFile(path);
    const sym = sf && checker.getSymbolAtLocation(sf);
    const c =
      sym && checker.getExportsOfModule(sym).find((s) => s.name === "Contract");
    if (c) contracts.set(name, checker.getDeclaredTypeOfSymbol(c));
  }
  const sdkInterfaces = resolve(
    program.getCompilerOptions().baseUrl!,
    "sdk/interfaces.d.ts"
  );
  for (const sf of program.getSourceFiles()) {
    const visit = (node: ts.Node): void => {
      if (
        ts.isInterfaceDeclaration(node) &&
        node.name.text === "InterfaceContracts" &&
        sf.fileName !== generatedPath &&
        sf.fileName !== sdkInterfaces
      )
        report(
          node,
          "InterfaceContracts associations are generated; local augmentation is not authority"
        );
      if (
        ts.isCallExpression(node) &&
        sf.fileName.startsWith(pluginDir + "/")
      ) {
        const method = pluginApiName(checker, node);
        if (
          [
            "publish",
            "use",
            "tryUse",
            "watchOptional",
            "bindForwards",
          ].includes(method ?? "")
        ) {
          const arg = node.arguments[0];
          const name = arg && ts.isStringLiteralLike(arg) ? arg.text : null;
          if (!name || !contracts.has(name))
            report(
              node,
              `interface ${
                name ?? "(dynamic name)"
              } has no verified protocol 2 contract; declare the dependency and run s2s add`
            );
          else {
            if (node.typeArguments?.length)
              report(
                node,
                "protocol 2 infers Contract from the interface name; explicit generic arguments are forbidden"
              );
            if (method === "watchOptional" && !optionalDependencies.has(name))
              report(node, "watchOptional requires an optionalPluginDependencies entry");
            if (method === "watchOptional" && node.arguments[1]) {
              const callback = node.arguments[1];
              const type = checker.getTypeAtLocation(implementationExpression(checker, callback));
              const asynchronous = (type: ts.Type): boolean =>
                type.isUnion() ? type.types.some(asynchronous) :
                !!(type.flags & ts.TypeFlags.Any) || !!type.getProperty("then");
              if (type.flags & ts.TypeFlags.Any || type.getCallSignatures().some(signature =>
                asynchronous(checker.getReturnTypeOfSignature(signature))))
                report(callback, "watchOptional requires a synchronous attachment callback; thenables are forbidden");
            }
            if (method === "publish") {
              if (!publishes.has(name))
                report(node, `provider is not authorized to publish ${name}`);
              const c = contracts.get(name)!,
                m = c.getProperty("methods")!;
              const expected = checker.getTypeOfSymbolAtLocation(
                m,
                m.valueDeclaration!
              );
              const impl = node.arguments[1];
              const actual = impl && checker.getTypeAtLocation(impl);
              const implementationType =
                impl &&
                checker.getTypeAtLocation(
                  implementationExpression(checker, impl)
                );
              if (
                !actual ||
                actual.flags & ts.TypeFlags.Any ||
                !checker.isTypeAssignableTo(actual, expected)
              )
                report(
                  impl ?? node,
                  `producer implementation must agree with ${name} Contract.methods (${checker.typeToString(
                    expected
                  )})`
                );
              if (actual) {
                for (const property of actual.getProperties()) {
                  if (
                    !expected.getProperty(property.name) &&
                    checker
                      .getTypeOfSymbolAtLocation(property, impl)
                      .getCallSignatures().length
                  ) {
                    report(
                      impl,
                      `undeclared method ${property.name} is not in Contract.methods`
                    );
                  }
                }
              }
              if (actual) {
                for (const expectedMethod of expected.getProperties()) {
                  const implementation = implementationType?.getProperty(
                    expectedMethod.name
                  );
                  if (!implementation) continue; // Missing methods are diagnosed above.
                  const required = checker
                    .getTypeOfSymbolAtLocation(expectedMethod, impl)
                    .getCallSignatures()[0];
                  const declaration = implementation.valueDeclaration;
                  const value =
                    declaration && ts.isPropertyAssignment(declaration)
                      ? declaration.initializer
                      : declaration &&
                        ts.isShorthandPropertyAssignment(declaration)
                      ? declaration.name
                      : undefined;
                  const methodType = value
                    ? checker.getTypeAtLocation(
                        implementationExpression(checker, value)
                      )
                    : checker.getTypeOfSymbolAtLocation(implementation, impl);
                  const signatures = methodType.getCallSignatures();
                  if (
                    !required ||
                    signatures.length !== 1 ||
                    signatures[0].typeParameters?.length
                  ) {
                    report(
                      impl,
                      `producer method ${expectedMethod.name} signature must be a single non-generic function`
                    );
                    continue;
                  }
                  const provided = signatures[0];
                  const result = checker.getReturnTypeOfSignature(provided);
                  const expectedResult =
                    checker.getReturnTypeOfSignature(required);
                  // Ordinary TS callback assignability discards results for void and makes
                  // method parameters bivariant. Neither concession describes this wire boundary.
                  const resultMatches =
                    expectedResult.flags & ts.TypeFlags.Void
                      ? !!(
                          result.flags &
                          (ts.TypeFlags.Void |
                            ts.TypeFlags.Undefined |
                            ts.TypeFlags.Never)
                        )
                      : checker.isTypeAssignableTo(result, expectedResult);
                  if (result.flags & ts.TypeFlags.Any || !resultMatches)
                    report(
                      impl,
                      `producer method ${expectedMethod.name} result must agree with its synchronous Contract signature`
                    );
                  for (const [i, parameter] of provided.parameters.entries()) {
                    const declaration = parameter.valueDeclaration;
                    if (
                      declaration &&
                      ts.isParameter(declaration) &&
                      declaration.dotDotDotToken
                    ) {
                      report(
                        impl,
                        `producer method ${expectedMethod.name} signature cannot use rest parameters`
                      );
                      continue;
                    }
                    const input = checker.getTypeOfSymbolAtLocation(
                      parameter,
                      impl
                    );
                    const contractParameter = required.parameters[i];
                    if (!contractParameter) {
                      if (
                        !(parameter.flags & ts.SymbolFlags.Optional) &&
                        !(
                          declaration &&
                          ts.isParameter(declaration) &&
                          (declaration.questionToken || declaration.initializer)
                        )
                      )
                        report(
                          impl,
                          `producer method ${expectedMethod.name} requires an undeclared input`
                        );
                    } else {
                      const requiredInput = checker.getTypeOfSymbolAtLocation(
                        contractParameter,
                        impl
                      );
                      const variants = requiredInput.isUnion()
                        ? requiredInput.types
                        : [requiredInput];
                      const hasDefault =
                        declaration &&
                        ts.isParameter(declaration) &&
                        declaration.initializer;
                      if (
                        !variants.every(
                          (type) =>
                            (hasDefault &&
                              !!(type.flags & ts.TypeFlags.Undefined)) ||
                            checker.isTypeAssignableTo(type, input)
                        )
                      )
                        report(
                          impl,
                          `producer method ${expectedMethod.name} input ${parameter.name} is narrower than its Contract signature`
                        );
                    }
                  }
                }
              }
            } else if (!dependencies.has(name))
              report(node, `undeclared dependency ${name}`);
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(sf);
  }
  return out;
}
