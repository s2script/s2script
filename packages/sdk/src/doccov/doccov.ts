// Doc-coverage analyzer for the author-facing .d.ts stubs (the intellisense surface).
// Flags every exported symbol / member lacking its OWN /** */ doc comment.
// The per-file banner (a /** at file offset 0) does NOT satisfy a symbol.
import ts from "typescript";
import { readFileSync } from "node:fs";

export interface Gap {
  file: string;
  line: number; // 1-based
  symbol: string;
  kind: string;
}

function hasExportMod(node: ts.Node): boolean {
  const mods = ts.canHaveModifiers(node) ? ts.getModifiers(node) : undefined;
  return !!mods?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword);
}

function memberName(node: ts.Node): string | null {
  const name = (node as { name?: ts.Node }).name;
  if (name && (ts.isIdentifier(name) || ts.isStringLiteral(name))) return name.text;
  return null;
}

export function analyzeSource(fileName: string, text: string): Gap[] {
  const sf = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, /*setParentNodes*/ true);
  const gaps: Gap[] = [];

  // Banner = a /** leading the first statement that starts at file offset 0.
  let bannerPos = -1;
  const first = sf.statements[0];
  if (first) {
    const r = (ts.getLeadingCommentRanges(text, first.getFullStart()) ?? []).find(
      (x) =>
        x.kind === ts.SyntaxKind.MultiLineCommentTrivia && text.slice(x.pos, x.pos + 3) === "/**",
    );
    if (r && r.pos === 0) bannerPos = 0;
  }

  const hasOwnDoc = (node: ts.Node): boolean =>
    (ts.getLeadingCommentRanges(text, node.getFullStart()) ?? []).some(
      (r) =>
        r.kind === ts.SyntaxKind.MultiLineCommentTrivia &&
        text.slice(r.pos, r.pos + 3) === "/**" &&
        r.pos !== bannerPos,
    );

  const lineOf = (node: ts.Node) => sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
  const flag = (node: ts.Node, name: string, kind: string) => {
    if (!hasOwnDoc(node)) gaps.push({ file: fileName, line: lineOf(node), symbol: name, kind });
  };

  const walkMembers = (members: ts.NodeArray<ts.Node>) => {
    for (const m of members) {
      switch (m.kind) {
        case ts.SyntaxKind.PropertySignature:
        case ts.SyntaxKind.MethodSignature:
        case ts.SyntaxKind.PropertyDeclaration:
        case ts.SyntaxKind.MethodDeclaration:
        case ts.SyntaxKind.GetAccessor:
        case ts.SyntaxKind.SetAccessor:
        case ts.SyntaxKind.EnumMember: {
          const nm = memberName(m);
          if (nm) flag(m, nm, ts.SyntaxKind[m.kind]);
          break;
        }
        default:
          break; // IndexSignature / Constructor / Construct+Call signatures → skipped
      }
    }
  };

  for (const st of sf.statements) {
    if (!hasExportMod(st)) continue; // imports, `export * from`, non-exported decls → skip
    if (ts.isInterfaceDeclaration(st)) {
      flag(st, st.name.text, "interface");
      walkMembers(st.members);
    } else if (ts.isClassDeclaration(st) && st.name) {
      flag(st, st.name.text, "class");
      walkMembers(st.members);
    } else if (ts.isEnumDeclaration(st)) {
      flag(st, st.name.text, "enum");
      walkMembers(st.members);
    } else if (ts.isTypeAliasDeclaration(st)) {
      flag(st, st.name.text, "type");
      if (ts.isTypeLiteralNode(st.type)) walkMembers(st.type.members);
    } else if (ts.isFunctionDeclaration(st) && st.name) {
      flag(st, st.name.text, "function");
    } else if (ts.isVariableStatement(st)) {
      const documented = hasOwnDoc(st);
      for (const d of st.declarationList.declarations) {
        const nm = ts.isIdentifier(d.name) ? d.name.text : "(const)";
        if (!documented) gaps.push({ file: fileName, line: lineOf(d), symbol: nm, kind: "const" });
        if (d.type && ts.isTypeLiteralNode(d.type)) walkMembers(d.type.members);
      }
    }
  }
  return gaps;
}

export function findUndocumented(files: string[]): Gap[] {
  return files.flatMap((f) => analyzeSource(f, readFileSync(f, "utf8")));
}
