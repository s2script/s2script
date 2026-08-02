# Shared JSONC (JSON-with-comments) support for the gamedata gate scripts
# (check-gamedata-owners.sh, check-gamedata-sigs.sh). One stripper for one file format — two
# copies that could drift out of sync is its own bug (A5a Task 6 fix round 2).
#
# Ports packages/sdk/src/gamedata/jsonc.ts stripJsonComments() verbatim (same semantics, same file
# format). String-aware: a `//` or `/*` inside a JSON string literal is content, not a comment, and
# a backslash-escaped quote does not end the string. Comments are blanked (not deleted) so a
# json.JSONDecodeError's reported line/column still lands on the author's actual line.


def strip_jsonc_comments(src: str) -> str:
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]

        if c == '"':
            out.append(c)
            i += 1
            while i < n:
                s = src[i]
                if s == '\\':
                    out.append(src[i:i + 2])  # copy the escape pair whole
                    i += 2
                    continue
                out.append(s)
                i += 1
                if s == '"':
                    break
            continue

        if c == '/' and i + 1 < n and src[i + 1] == '/':
            while i < n and src[i] != '\n':
                out.append(' ')
                i += 1
            continue

        if c == '/' and i + 1 < n and src[i + 1] == '*':
            out.append('  ')
            i += 2
            while i < n and not (src[i] == '*' and i + 1 < n and src[i + 1] == '/'):
                out.append('\n' if src[i] == '\n' else ' ')
                i += 1
            if i < n:
                out.append('  ')  # the closing */
                i += 2
            continue

        out.append(c)
        i += 1
    return ''.join(out)
