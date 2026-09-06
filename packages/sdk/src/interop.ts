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
  forwards: Record<string, { kind: "notification"; payload: WireSchema }>;
}
export interface WireContract {
  metadata: ContractMetadata;
  sha256: string;
}
export function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object")
    return `{${Object.keys(value)
      .sort()
      .map(
        (k) =>
          `${JSON.stringify(k)}:${canonical(
            (value as Record<string, unknown>)[k]
          )}`
      )
      .join(",")}}`;
  return JSON.stringify(value);
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
          ? ["Notification"]
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
          "contracts must be self-contained; only SDK Notification and EntityRef imports are supported"
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
        {};
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
  const methods: ContractMetadata["methods"] = {};
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
  const forwards: ContractMetadata["forwards"] = {};
  for (const p of prop(contract, "forwards", sf).getProperties()) {
    const node = p.valueDeclaration ?? sf,
      type = checker.getTypeOfSymbolAtLocation(p, node);
    const brand = type.getProperty("__notificationPayload");
    if (
      !brand ||
      !brand.declarations?.some(
        (d) =>
          d.getSourceFile().fileName ===
          resolve(packagesDir, "sdk/interfaces.d.ts")
      )
    )
      fail(
        node,
        "only SDK Notification<P> forwards are supported by this protocol slice"
      );
    forwards[p.name] = {
      kind: "notification",
      payload: schema(checker.getTypeOfSymbolAtLocation(brand!, node), node),
    };
  }
  const metadata = JSON.parse(
    canonical({ version: 1, methods, forwards })
  ) as ContractMetadata;
  return {
    metadata,
    sha256: createHash("sha256").update(canonical(metadata)).digest("hex"),
  };
}

/** The checker validates call sites against authoritative contracts even if overload fallback,
 * caller-supplied generics, any, or a local augmentation would otherwise hide a mismatch. */
export function checkInteropCalls(
  program: ts.Program,
  paths: Record<string, string>,
  publishes: Set<string>,
  generatedPath: string,
  pluginDir: string,
  dependencies: Set<string>
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
        const expr = ts.isPropertyAccessExpression(node.expression)
          ? node.expression.name
          : node.expression;
        const method = pluginApiName(checker, expr);
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
              if (actual)
                for (const p of actual.getProperties())
                  for (const sig of checker
                    .getTypeOfSymbolAtLocation(p, impl)
                    .getCallSignatures())
                    if (
                      checker.getReturnTypeOfSignature(sig).flags &
                      ts.TypeFlags.Any
                    )
                      report(impl, "producer method result cannot be any");
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
