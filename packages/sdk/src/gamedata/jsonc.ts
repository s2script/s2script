/**
 * Minimal JSONC comment stripper.
 *
 * WHY THIS EXISTS. The first implementation stripped only FULL-LINE `//` comments via a line-anchored
 * regex. That is narrower than every other JSONC reader in this repo — the shim parses
 * `core.gamedata.jsonc` with nlohmann's `ignore_comments=true` (both comment forms, anywhere), and
 * `core/src/config.rs::strip_line_comments` is string-aware and handles trailing `//`. The gap mattered
 * because gamedata authors are explicitly told to document byte patterns the way
 * `gamedata/core.gamedata.jsonc` does, and that file uses trailing line comments and block comments
 * freely — so following the house style produced a raw `JSON.parse` syntax error.
 *
 * String-aware by construction: a `//` or `/*` inside a JSON string literal is content, not a comment
 * (byte patterns and Windows-ish paths both hit this). Escapes are honoured so `"a\\"` does not look
 * like an unterminated string.
 *
 * Comments are replaced with spaces rather than removed so byte offsets — and therefore
 * `JSON.parse`'s "at position N" — still point at the right place in the author's file.
 *
 * Trailing commas are deliberately NOT accepted: nlohmann rejects them too, so accepting them here
 * would let a gamedata build that the shim cannot load.
 */
export function stripJsonComments(src: string): string {
  let out = "";
  let i = 0;
  const n = src.length;

  while (i < n) {
    const c = src[i]!;

    // --- inside a string literal: copy verbatim until the closing quote
    if (c === '"') {
      out += c;
      i++;
      while (i < n) {
        const s = src[i]!;
        if (s === "\\") {
          out += src.slice(i, i + 2); // copy the escape pair whole
          i += 2;
          continue;
        }
        out += s;
        i++;
        if (s === '"') break;
      }
      continue;
    }

    // --- line comment: blank to end of line, KEEP the newline (line numbers must not shift)
    if (c === "/" && src[i + 1] === "/") {
      while (i < n && src[i] !== "\n") {
        out += " ";
        i++;
      }
      continue;
    }

    // --- block comment: blank it, preserving newlines inside
    if (c === "/" && src[i + 1] === "*") {
      out += "  ";
      i += 2;
      while (i < n && !(src[i] === "*" && src[i + 1] === "/")) {
        out += src[i] === "\n" ? "\n" : " ";
        i++;
      }
      if (i < n) {
        out += "  "; // the closing */
        i += 2;
      }
      continue;
    }

    out += c;
    i++;
  }

  return out;
}
