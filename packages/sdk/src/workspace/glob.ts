/**
 * The segment glob matcher for `*` and `**` (design spec 2026-07-27 §4.4).
 *
 * Hand-rolled rather than pulled from minimatch/picomatch because the SDK ships with zero
 * runtime dependencies and the grammar we actually need is tiny: npm `workspaces` entries
 * and `s2script.workspace.plugins` are `plugins/*`, `packages/*`, `plugins/disabled/*` —
 * plus `--filter` patterns like `@me/*` and `plugins/sh*`. Everything here is pure string
 * work; the filesystem side of glob expansion lives in workspace.ts so this file stays
 * trivially unit-testable.
 *
 * Grammar:
 *   `**`  as a WHOLE segment  — matches zero or more segments
 *   `*`   inside a segment    — matches any run of characters except `/` (so `sh*` works)
 *   anything else             — a literal, matched exactly
 */

/** Normalize Windows separators; the whole module speaks posix paths. */
export function toPosix(p: string): string {
  return p.replace(/\\/g, "/");
}

/**
 * Split a posix path or pattern into its meaningful segments: leading `./`, duplicated and
 * trailing slashes are noise, not structure.
 */
export function splitSegments(p: string): string[] {
  return toPosix(p)
    .split("/")
    .filter((s) => s !== "" && s !== ".");
}

/** Regex-escape everything but `*`, which becomes "any run of non-separator characters". */
function segmentRegExp(pattern: string): RegExp {
  const body = pattern.replace(/[.+^${}()|[\]\\?]/g, "\\$&").replace(/\*/g, "[^/]*");
  return new RegExp(`^${body}$`);
}

/** True when one path segment matches one pattern segment (`*` is intra-segment only). */
export function matchSegment(pattern: string, segment: string): boolean {
  if (!pattern.includes("*")) return pattern === segment;
  return segmentRegExp(pattern).test(segment);
}

function matchFrom(pat: string[], seg: string[], pi: number, si: number): boolean {
  if (pi === pat.length) return si === seg.length;
  if (pat[pi] === "**") {
    // Zero or more segments: try every split point. Patterns are a handful of segments long,
    // so the naive backtrack is cheaper than the machinery to avoid it.
    for (let i = si; i <= seg.length; i++) {
      if (matchFrom(pat, seg, pi + 1, i)) return true;
    }
    return false;
  }
  if (si === seg.length) return false;
  return matchSegment(pat[pi], seg[si]) && matchFrom(pat, seg, pi + 1, si + 1);
}

/**
 * True when `path` matches `pattern`. Both are treated as segment lists, which is why
 * `plugins/*` does NOT match `plugins/a/b` — the distinction npm workspaces relies on.
 */
export function matchGlob(pattern: string, path: string): boolean {
  return matchFrom(splitSegments(pattern), splitSegments(path), 0, 0);
}

/** True when the pattern contains a wildcard at all (a plain path needs no expansion). */
export function hasMagic(pattern: string): boolean {
  return pattern.includes("*");
}
