// TSDoc content extractor for the author-facing .d.ts stubs.
// Sibling to doccov.ts: where doccov flags MISSING docs, this pulls the doc CONTENT
// (banner, per-symbol signature + summary + @param/@returns/@throws/@example) so the
// website can generate its module reference from the shipped types — the single source
// of truth — instead of a hand-curated list that drifts.
import ts from "typescript";
import { readFileSync } from "node:fs";

export type SymbolKind =
  | "function"
  | "const"
  | "interface"
  | "class"
  | "enum"
  | "type"
  | "method"
  | "property"
  | "accessor"
  | "enumMember";

export interface DocParam {
  name: string;
  text: string;
}

export interface SymbolDoc {
  name: string;
  kind: SymbolKind;
  /** A cleaned one-line-ish signature, e.g. `delay(ms: number): Promise<void>`. Containers show just their name. */
  signature: string;
  /** The description prose (JSDoc comment body, `{@link X}` flattened to `X`). */
  summary: string;
  params?: DocParam[];
  returns?: string;
  throws?: string[];
  examples?: string[];
  /** Members of a container symbol (interface / class / enum / const-object / type-literal). */
  members?: SymbolDoc[];
}

export interface ModuleDoc {
  /** Cleaned module description drawn from the file banner (the `@pkg — …` prefix + boilerplate stripped). */
  banner: string;
  exports: SymbolDoc[];
}

/** Flatten a JSDoc comment (string, or a mix of text + `{@link}` nodes) to plain prose. */
function commentToString(comment: string | ts.NodeArray<ts.JSDocComment> | undefined): string {
  if (comment === undefined) return "";
  if (typeof comment === "string") return comment.trim();
  return comment
    .map((part) => {
      if (ts.isJSDocLink(part) || ts.isJSDocLinkCode(part) || ts.isJSDocLinkPlain(part)) {
        const name = part.name ? part.name.getText() : "";
        const tail = part.text ?? "";
        return (name + tail).trim();
      }
      return part.text;
    })
    .join("")
    .trim();
}

/** Description prose attached to a node (concatenated JSDoc comment blocks, tags excluded). */
function summaryOf(node: ts.Node): string {
  const parts = ts
    .getJSDocCommentsAndTags(node)
    .filter((n): n is ts.JSDoc => ts.isJSDoc(n))
    .map((doc) => commentToString(doc.comment))
    .filter((s) => s.length > 0);
  return parts.join("\n\n").trim();
}

interface Tagged {
  params?: DocParam[];
  returns?: string;
  throws?: string[];
  examples?: string[];
}

/** Pull the structured `@param` / `@returns` / `@throws` / `@example` tags off a node. */
function tagsOf(node: ts.Node): Tagged {
  const out: Tagged = {};
  for (const tag of ts.getJSDocTags(node)) {
    const kind = tag.tagName.text;
    if (ts.isJSDocParameterTag(tag)) {
      // TSDoc separates `@param name - desc` with a hyphen; strip it — it's syntax, not content.
      const text = commentToString(tag.comment).replace(/^-\s*/, "");
      if (text) (out.params ??= []).push({ name: tag.name.getText(), text });
    } else if (kind === "returns" || kind === "return") {
      const text = commentToString(tag.comment);
      if (text) out.returns = text;
    } else if (kind === "throws" || kind === "exception") {
      const text = commentToString(tag.comment);
      if (text) (out.throws ??= []).push(text);
    } else if (kind === "example") {
      const text = commentToString(tag.comment);
      if (text) (out.examples ??= []).push(text);
    }
  }
  return out;
}

function paramsText(node: ts.SignatureDeclarationBase, sf: ts.SourceFile): string {
  const ps = node.parameters.map((p) => p.getText(sf));
  return "(" + ps.join(", ") + ")";
}

function readonlyPrefix(node: ts.Node): string {
  const mods = ts.canHaveModifiers(node) ? ts.getModifiers(node) : undefined;
  return mods?.some((m) => m.kind === ts.SyntaxKind.ReadonlyKeyword) ? "readonly " : "";
}

/** Build a clean, source-faithful signature string for a leaf member/declaration. */
function signatureOf(node: ts.Node, name: string, sf: ts.SourceFile): string {
  if (ts.isFunctionDeclaration(node) || ts.isMethodSignature(node) || ts.isMethodDeclaration(node)) {
    const ret = node.type ? ": " + node.type.getText(sf) : "";
    return name + paramsText(node, sf) + ret;
  }
  if (ts.isGetAccessorDeclaration(node)) {
    const ret = node.type ? ": " + node.type.getText(sf) : "";
    return "get " + name + "()" + ret;
  }
  if (ts.isPropertySignature(node) || ts.isPropertyDeclaration(node)) {
    const opt = node.questionToken ? "?" : "";
    const ty = node.type ? node.type.getText(sf) : "unknown";
    return readonlyPrefix(node) + name + opt + ": " + ty;
  }
  if (ts.isEnumMember(node)) {
    return node.initializer ? name + " = " + node.initializer.getText(sf) : name;
  }
  return name; // containers (interface/class/enum/const/type) — the kind badge carries the rest
}

function memberName(node: ts.Node): string | null {
  const name = (node as { name?: ts.Node }).name;
  if (name && (ts.isIdentifier(name) || ts.isStringLiteral(name))) return name.text;
  return null;
}

function memberKind(node: ts.Node): SymbolKind {
  switch (node.kind) {
    case ts.SyntaxKind.MethodSignature:
    case ts.SyntaxKind.MethodDeclaration:
      return "method";
    case ts.SyntaxKind.GetAccessor:
    case ts.SyntaxKind.SetAccessor:
      return "accessor";
    case ts.SyntaxKind.EnumMember:
      return "enumMember";
    default:
      return "property";
  }
}

function buildMember(node: ts.Node, sf: ts.SourceFile): SymbolDoc | null {
  const nm = memberName(node);
  if (!nm) return null;
  const doc: SymbolDoc = {
    name: nm,
    kind: memberKind(node),
    signature: signatureOf(node, nm, sf),
    summary: summaryOf(node),
    ...tagsOf(node),
  };
  return doc;
}

function walkMembers(members: ts.NodeArray<ts.Node>, sf: ts.SourceFile): SymbolDoc[] {
  const out: SymbolDoc[] = [];
  for (const m of members) {
    switch (m.kind) {
      case ts.SyntaxKind.PropertySignature:
      case ts.SyntaxKind.MethodSignature:
      case ts.SyntaxKind.PropertyDeclaration:
      case ts.SyntaxKind.MethodDeclaration:
      case ts.SyntaxKind.GetAccessor:
      case ts.SyntaxKind.EnumMember: {
        const d = buildMember(m, sf);
        if (d) out.push(d);
        break;
      }
      default:
        break; // IndexSignature / Constructor / SetAccessor(paired) / call+construct sigs → skip
    }
  }
  return out;
}

function hasExportMod(node: ts.Node): boolean {
  const mods = ts.canHaveModifiers(node) ? ts.getModifiers(node) : undefined;
  return !!mods?.some((m) => m.kind === ts.SyntaxKind.ExportKeyword);
}

/** Strip the `@pkg — ` prefix and the shared "NO runtime code…" boilerplate from a file banner. */
export function cleanBanner(raw: string): string {
  let s = raw.replace(/\s+/g, " ").trim(); // normalize first so the ^@ prefix anchor holds
  s = s.replace(/^@\S+\s*[—–-]\s*/, ""); // strip the "@pkg — " prefix (em/en-dash or hyphen)
  s = s.replace(/\bNO runtime code\b.*$/i, ""); // cut the boilerplate tail every stub banner carries
  return s.replace(/\s+([.,;:])/g, "$1").trim();
}

function bannerOf(sf: ts.SourceFile, text: string): string {
  const first = sf.statements[0];
  const start = first ? first.getFullStart() : 0;
  const ranges = ts.getLeadingCommentRanges(text, start) ?? [];
  const banner = ranges.find(
    (r) =>
      r.kind === ts.SyntaxKind.MultiLineCommentTrivia &&
      r.pos === 0 &&
      text.slice(r.pos, r.pos + 3) === "/**",
  );
  if (!banner) return "";
  const body = text
    .slice(banner.pos, banner.end)
    .replace(/^\/\*\*/, "")
    .replace(/\*\/$/, "")
    .split("\n")
    .map((line) => line.replace(/^\s*\*?/, "").trim())
    .join(" ");
  return cleanBanner(body);
}

export function extractModule(fileName: string, text: string): ModuleDoc {
  const sf = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, /*setParentNodes*/ true);
  const exports: SymbolDoc[] = [];

  for (const st of sf.statements) {
    if (!hasExportMod(st)) continue; // imports, `export * from`, non-exported decls → skip
    if (ts.isInterfaceDeclaration(st)) {
      exports.push({
        name: st.name.text,
        kind: "interface",
        signature: st.name.text,
        summary: summaryOf(st),
        ...tagsOf(st),
        members: walkMembers(st.members, sf),
      });
    } else if (ts.isClassDeclaration(st) && st.name) {
      exports.push({
        name: st.name.text,
        kind: "class",
        signature: st.name.text,
        summary: summaryOf(st),
        ...tagsOf(st),
        members: walkMembers(st.members, sf),
      });
    } else if (ts.isEnumDeclaration(st)) {
      exports.push({
        name: st.name.text,
        kind: "enum",
        signature: st.name.text,
        summary: summaryOf(st),
        ...tagsOf(st),
        members: walkMembers(st.members, sf),
      });
    } else if (ts.isTypeAliasDeclaration(st)) {
      const members = ts.isTypeLiteralNode(st.type) ? walkMembers(st.type.members, sf) : undefined;
      exports.push({
        name: st.name.text,
        kind: "type",
        signature: st.name.text,
        summary: summaryOf(st),
        ...tagsOf(st),
        ...(members && members.length ? { members } : {}),
      });
    } else if (ts.isFunctionDeclaration(st) && st.name) {
      exports.push({
        name: st.name.text,
        kind: "function",
        signature: signatureOf(st, st.name.text, sf),
        summary: summaryOf(st),
        ...tagsOf(st),
      });
    } else if (ts.isVariableStatement(st)) {
      // Doc + tags live on the statement; a const-object's methods live in its type literal.
      const stSummary = summaryOf(st);
      const stTags = tagsOf(st);
      for (const d of st.declarationList.declarations) {
        if (!ts.isIdentifier(d.name)) continue;
        const members = d.type && ts.isTypeLiteralNode(d.type) ? walkMembers(d.type.members, sf) : undefined;
        exports.push({
          name: d.name.text,
          kind: "const",
          signature: d.name.text,
          summary: stSummary,
          ...stTags,
          ...(members && members.length ? { members } : {}),
        });
      }
    }
  }

  return { banner: bannerOf(sf, text), exports };
}

export function extractModuleFile(fileName: string): ModuleDoc {
  return extractModule(fileName, readFileSync(fileName, "utf8"));
}
