#!/usr/bin/env bash
# Build the GitHub Release body for a v* tag, INCLUDING the package changelogs.
#
# WHY THIS EXISTS. release.yml used to pass a hardcoded `body:` to action-gh-release. That does two
# bad things: it ships a release whose notes are pure boilerplate, and — because action-gh-release
# REPLACES the body when the release already exists — a re-run silently destroys whatever was there.
#
# Meanwhile the actual prose lives in packages/*/CHANGELOG.md, written by changesets from each
# slice's changeset. Between v0.2.2 and v0.3.0 that was 138 lines of it, none of which reached the
# release. This script pulls those entries in.
#
# The zip version and the npm package versions are INDEPENDENT (a v* tag is the runtime; @s2script/*
# publish on their own cadence), so this cannot select "the 0.3.0 section". It instead reports every
# package changelog SECTION that appeared between the previous v* tag and this one — which is
# exactly "what shipped in this runtime release".
#
# Usage: scripts/release-notes.sh <version> [sha256]     (version without the leading v)
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:?usage: release-notes.sh <version> [sha256]}"
SHA256="${2:-}"
TAG="v${VERSION}"

# The previous v* tag = the highest tag strictly BELOW this one in version order. Not merely "the
# newest other tag": re-running an older tag must look backwards, not forwards. Empty on the first
# ever release, which the generator reports as "in this first release".
PREV=$(git tag --list 'v*' --sort=v:refname \
       | awk -v cur="$TAG" '$0 == cur { exit } { last = $0 } END { if (last) print last }')

python3 - "$VERSION" "$SHA256" "$PREV" "$TAG" <<'PY'
import re, subprocess, sys, pathlib

version, sha256, prev, tag = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]

def at(ref, path):
    """File contents at a git ref, or '' if absent there."""
    r = subprocess.run(["git", "show", f"{ref}:{path}"], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else ""

def sections(md):
    """Split a changesets CHANGELOG into {version: body}."""
    out, cur, buf = {}, None, []
    for line in md.splitlines():
        m = re.match(r'^## +(\S+)', line)
        if m:
            if cur: out[cur] = "\n".join(buf).strip()
            cur, buf = m.group(1), []
        elif cur is not None:
            buf.append(line)
    if cur: out[cur] = "\n".join(buf).strip()
    return out

def demote(md):
    """Changesets writes '### Minor Changes' inside each version. This nests them under our
    '#### <version>' heading so the release page outline stays sane."""
    return re.sub(r'^(#{1,4}) ', lambda m: '#' * min(len(m.group(1)) + 2, 6) + ' ', md, flags=re.M)

def dependency_only(md):
    """True when a section says nothing but 'Updated dependencies'. Those entries are real but
    they are pure churn in a runtime release note, and they crowd out the prose that matters."""
    meaningful = [l.strip() for l in md.splitlines()
                  if l.strip() and not l.strip().startswith("#")]
    if not meaningful:
        return True
    return all(re.match(r'^-\s*(Updated dependencies|@s2script/)', l) for l in meaningful)

parts = []
for pkg in sorted(pathlib.Path("packages").glob("*/CHANGELOG.md")):
    name = pkg.parent.name
    now = sections(pkg.read_text())
    before = sections(at(prev, str(pkg))) if prev else {}
    new = [v for v in now if v not in before]
    if not new:
        continue
    # Newest first — changesets already writes them that way, so preserve file order.
    ordered = [v for v in now if v in new]
    chunks = []
    for v in ordered:
        body = now[v]
        if not body or dependency_only(body):
            continue          # "Updated dependencies" churn buries the prose; skip it
        chunks.append(f"#### {v}\n\n{demote(body)}")
    if chunks:
        parts.append(f"### `@s2script/{name}`\n\n" + "\n\n".join(chunks))

print("SourceMod-style runtime for Counter-Strike 2 (Linux x86-64), including the first-party base plugins.\n")
print(f"**Install:** extract `s2script-cs2-linux-{version}.zip` into your server's `game/csgo/` "
      "directory (overlays `addons/`). Metamod:Source 2.0 must already be installed. "
      "See [docs/INSTALL.md](../blob/main/docs/INSTALL.md).\n")
if sha256:
    print(f"**SHA256:** `{sha256}`\n")
print("Base plugins load from `addons/s2script/plugins/` on first boot. Drop additional `.s2sp` "
      "files there to add more.\n")

if parts:
    rng = f"since `{prev}`" if prev else "in this first release"
    print(f"---\n\n## Package changelogs {rng}\n")
    print("\n\n".join(parts))
elif prev:
    print(f"---\n\n_No `@s2script/*` package changes since `{prev}` — runtime/plugin-only release._")
PY
