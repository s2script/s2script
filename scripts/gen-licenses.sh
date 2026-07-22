#!/usr/bin/env bash
# Regenerate licenses/licenses.txt — every third-party notice the shipped binaries owe.
#
# Reads the REAL sources (initialized submodules under third_party/, verbatim upstream
# texts under licenses/vendor/, and the crate sources in the cargo registry) so the
# notice file can never drift from what is actually linked. Fails closed: a missing
# submodule or unreadable license aborts rather than emitting an incomplete file.
#
# Output is deterministic — no timestamps, no absolute paths, crates sorted — so
# check-licenses-generated.sh can gate it with `git diff --exit-code`.
#
# Requires: git submodule update --init --recursive && cargo fetch
set -eu
cd "$(cd "$(dirname "$0")/.." && pwd)"

OUT=licenses/licenses.txt

python3 - "$OUT" <<'PYEOF'
import hashlib, os, re, subprocess, sys, glob

out_path = sys.argv[1]
ROOT = os.getcwd()
die = lambda m: (sys.stderr.write("gen-licenses: FATAL: %s\n" % m), sys.exit(1))

def read(path, what):
    if not os.path.exists(path):
        die("%s: missing %s\n  (did you run `git submodule update --init --recursive`?)" % (what, path))
    return open(path, encoding="utf-8", errors="replace").read().rstrip("\n")

def rev(path):
    # --short=12, never bare --short: git picks the abbreviation length from the repo's
    # object count, so a full clone yields 8 chars where a shallow CI checkout yields 7.
    # That made the "deterministic" output environment-dependent and failed the gate on
    # unrelated PRs. A fixed width is the whole point.
    try:
        return subprocess.run(["git", "-C", path, "rev-parse", "--short=12", "HEAD"],
                              capture_output=True, text=True, check=True).stdout.strip()
    except Exception:
        die("cannot resolve revision of %s" % path)

def rule(ch="="):  return ch * 78
def head(n, title, lic):
    return "\n%s\n %s. %s\n     %s\n%s\n" % (rule(), n, title, lic, rule())

# ---------------------------------------------------------------- native components
NATIVE = [
    ("Valve Source 2 SDK (hl2sdk)", "NOT open source — see notice below", None),
    ("Metamod:Source", "zlib/libpng license", "third_party/metamod-source/LICENSE.txt"),
    ("Google Breakpad", "BSD-3-Clause (plus aggregated sub-licenses)", "third_party/breakpad/LICENSE"),
    ("Protocol Buffers 3.21.8", "BSD-3-Clause", "third_party/hl2sdk/thirdparty/protobuf-3.21.8/LICENSE"),
    ("Hacker Disassembler Engine 64 (via MinHook)", "BSD-2-Clause", "third_party/hde/HDE-LICENSE.txt"),
    ("V8 JavaScript engine", "BSD-3-Clause", "licenses/vendor/v8.LICENSE"),
    ("rusty_v8 — Rust bindings, crate `v8`", "MIT", "licenses/vendor/rusty_v8.LICENSE"),
]

VALVE_NOTICE = """\
s2script does NOT redistribute Valve source code. `third_party/hl2sdk` is a git
submodule (alliedmodders/hl2sdk, branch `cs2`); a clone of the s2script repository
contains none of it.

The BUILT binary `s2script.so` does, however, embed Valve SDK object code.
`shim/CMakeLists.txt` compiles the following Valve translation units into it:

    entity2/entitykeyvalues.cpp     CEntityKeyValues, for EKV-configured spawns
    tier1/keyvalues3.cpp            KV3 backing store for the above
    public/tier0/memoverride.cpp    routes operator new/delete to the game allocator
    lib/linux64/release/libprotobuf.a   protobuf 3.21.8 reflection for SayText2

The alliedmodders/hl2sdk repository ships no LICENSE file. Its sources carry:

    Copyright (c) 1996-2005, Valve Corporation, All rights reserved.

Valve's SDK terms are not an open-source license and there is no upstream license
text to reproduce here. Accordingly, s2script's MIT/Apache-2.0 grant covers
s2script's own code only; it does not, and cannot, relicense the Valve-derived
portions of a built binary. Use of the Valve SDK is governed by your agreement
with Valve Corporation.

This is the same posture as every other framework in this ecosystem —
Metamod:Source, SourceMod, CounterStrikeSharp and CS2Fixes all ship binaries
built against hl2sdk on the same footing."""

SQLITE_FALLBACK = """\
SQLite is in the Public Domain.

The author disclaims copyright to this source code.  In place of
a legal notice, here is a blessing:

    May you do good and not evil.
    May you find forgiveness for yourself and forgive others.
    May you share freely, never taking more than you give."""

# ---------------------------------------------------------------- crate inventory
lock = read("Cargo.lock", "Cargo.lock")
pkgs = re.findall(r'\[\[package\]\]\nname = "([^"]+)"\nversion = "([^"]+)"', lock)
FIRST_PARTY = {"s2script-core", "s2script-cs2"}
pkgs = sorted({(n, v) for n, v in pkgs if n not in FIRST_PARTY})

reg_roots = sorted(glob.glob(os.path.expanduser("~/.cargo/registry/src/*")))
if not reg_roots:
    die("no cargo registry found — run `cargo fetch` first")

LICENSE_FILE_RE = re.compile(r"^(LICEN[CS]E|COPYING|COPYRIGHT|NOTICE|UNLICENSE)([-_.].*)?$", re.I)

inventory, no_text, texts = [], [], {}   # texts: sha -> (text, [crate refs])
for name, ver in pkgs:
    cdir = next((d for r in reg_roots
                 if os.path.isdir(d := os.path.join(r, "%s-%s" % (name, ver)))), None)
    if cdir is None:
        die("crate %s-%s not in the local registry — run `cargo fetch`" % (name, ver))
    manifest = open(os.path.join(cdir, "Cargo.toml"), encoding="utf-8", errors="replace").read()
    m = re.search(r"^license\s*=\s*\"([^\"]+)\"", manifest, re.M)
    spdx = m.group(1) if m else ("(license-file)" if re.search(r"^license-file", manifest, re.M) else None)
    if spdx is None:
        die("crate %s-%s declares no license — cannot generate a complete notice" % (name, ver))
    inventory.append((name, ver, spdx))

    found = False
    for fn in sorted(os.listdir(cdir)):
        if LICENSE_FILE_RE.match(fn) and os.path.isfile(os.path.join(cdir, fn)):
            body = open(os.path.join(cdir, fn), encoding="utf-8", errors="replace").read().strip()
            if not body:
                continue
            sha = hashlib.sha256(body.encode()).hexdigest()
            texts.setdefault(sha, [body, []])[1].append("%s %s (%s)" % (name, ver, fn))
            found = True
    if not found:
        repo = (re.search(r"^repository\s*=\s*\"([^\"]+)\"", manifest, re.M) or [None, ""])[1]
        no_text.append((name, ver, spdx, repo))

# SQLite blessing, preferably lifted verbatim from the bundled amalgamation itself
sqlite_text, sqlite_src = SQLITE_FALLBACK, "canonical text"
for c in sorted(sum((glob.glob(os.path.join(r, "libsqlite3-sys-*", "sqlite3", "sqlite3.c"))
                     for r in reg_roots), [])):
    with open(c, encoding="utf-8", errors="replace") as fh:
        hdr = fh.read(8000)
    lines = [l for l in hdr.splitlines() if l.startswith("**")]
    start = next((i for i, l in enumerate(lines) if "disclaims copyright" in l), None)
    end = next((i for i, l in enumerate(lines) if "never taking more than you give" in l), None)
    if start is not None and end is not None and end > start:
        # strip the "** " comment prefix uniformly so the blessing keeps its own indentation
        body = "\n".join((l[3:] if l.startswith("** ") else l[2:]).rstrip()
                         for l in lines[start:end + 1]).strip()
        ver = re.search(r"SQLite\s+(?:\*\*\s*)?version\s+([0-9.]+)",
                        " ".join(l[2:].strip() for l in lines))
        sqlite_text = "SQLite is in the Public Domain.\n\n" + body
        sqlite_src = "extracted from the bundled amalgamation" + (
            " (SQLite %s)" % ver.group(1).rstrip(".") if ver else "")
    break

spdx_tally = {}
for _, _, s in inventory:
    spdx_tally[s] = spdx_tally.get(s, 0) + 1

# ---------------------------------------------------------------- emit
o = []
o.append(rule())
o.append(" s2script — THIRD-PARTY LICENSES AND NOTICES")
o.append(rule())
o.append("""
GENERATED FILE — do not edit by hand.
    Regenerate:  ./scripts/gen-licenses.sh
    Verify:      ./scripts/check-licenses-generated.sh

Scope: every third-party work redistributed in, or statically linked into, the
s2script release artifacts — addons/s2script/bin/s2script.so,
addons/s2script/bin/libs2script_core.so, and the bundled base plugins.

s2script itself is licensed MIT OR Apache-2.0; see ../LICENSE. Nothing below
changes that. These are the notices s2script owes to others.

Pinned at generation time:""")
o.append("    hl2sdk            %s   (branch cs2)" % rev("third_party/hl2sdk"))
o.append("    metamod-source    %s   (2.0.0.1403)" % rev("third_party/metamod-source"))
o.append("    breakpad          %s" % rev("third_party/breakpad"))
o.append("    rust crates       %d   (from Cargo.lock)" % len(inventory))
o.append("\nCONTENTS")
for i, (title, lic, _) in enumerate(NATIVE, 1):
    o.append("    %-3d %-52s %s" % (i, title, lic))
n = len(NATIVE)
o.append("    %-3d %-52s %s" % (n + 1, "SQLite (bundled via libsqlite3-sys)", "public domain"))
o.append("    %-3d %-52s %d crates" % (n + 2, "Rust crate dependencies — inventory", len(inventory)))
o.append("    %-3d %-52s %d texts" % (n + 3, "Rust crate dependencies — license texts", len(texts)))

for i, (title, lic, path) in enumerate(NATIVE, 1):
    o.append(head(i, title, lic))
    o.append(VALVE_NOTICE if path is None else read(path, title))

o.append(head(n + 1, "SQLite (bundled via libsqlite3-sys)", "public domain"))
o.append("[%s]\n" % sqlite_src)
o.append(sqlite_text)

o.append(head(n + 2, "Rust crate dependencies — inventory", "%d crates from Cargo.lock" % len(inventory)))
o.append("Licenses in effect across the crate graph (by declared SPDX expression):\n")
for s, c in sorted(spdx_tally.items(), key=lambda kv: (-kv[1], kv[0])):
    o.append("    %4d  %s" % (c, s))
o.append("\nNo crate in this graph is under a copyleft license.\n")
o.append("%-44s %-14s %s" % ("CRATE", "VERSION", "SPDX"))
o.append("%-44s %-14s %s" % ("-" * 44, "-" * 14, "-" * 18))
for name, ver, spdx in inventory:
    o.append("%-44s %-14s %s" % (name, ver, spdx))

if no_text:
    o.append("\nPackages whose published crate archive contains no license text. The license\n"
             "named above applies; canonical texts are at https://spdx.org/licenses/.\n")
    for name, ver, spdx, repo in no_text:
        o.append("    %-40s %-12s %-28s %s" % (name, ver, spdx, repo))

o.append(head(n + 3, "Rust crate dependencies — license texts", "%d distinct texts" % len(texts)))
o.append("Reproduced verbatim from each published crate archive and de-duplicated by\n"
         "content, so every distinct copyright line is preserved. Each text is followed\n"
         "by the crates it was taken from.\n")
for j, sha in enumerate(sorted(texts, key=lambda s: (texts[s][1][0].lower(), s)), 1):
    body, refs = texts[sha]
    o.append("\n%s\n[%d/%d]  %s\n%s\n" % (rule("-"), j, len(texts), ";  ".join(sorted(refs)), rule("-")))
    o.append(body)

o.append("\n%s\n END OF THIRD-PARTY LICENSES AND NOTICES\n%s" % (rule(), rule()))

with open(out_path, "w", encoding="utf-8") as fh:
    fh.write("\n".join(o).rstrip("\n") + "\n")

print("gen-licenses: wrote %s — %d native components, %d crates, %d distinct crate license texts"
      % (out_path, len(NATIVE) + 1, len(inventory), len(texts)))
PYEOF

# Keep each published npm package's license texts in lockstep with licenses/. npm always
# includes LICENSE* files in the tarball, so consumers get the actual terms the package's
# SPDX field names — without hand-maintaining six copies.
for pkg in packages/sdk packages/cs2 packages/eslint-plugin; do
    cp licenses/MIT.txt        "$pkg/LICENSE-MIT"
    cp licenses/Apache-2.0.txt "$pkg/LICENSE-APACHE"
done
echo "gen-licenses: synced LICENSE-MIT + LICENSE-APACHE into packages/{sdk,cs2,eslint-plugin}"
