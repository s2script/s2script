#!/usr/bin/env bash
# Enforces the gamedata OWNERSHIP rule (spec 2026-08-01-gamedata-tiering-design.md §4):
#
#   A key belongs to whoever NAMES it in source.
#
# gamedata/core/**  : every key MUST appear as a string literal in shim/src or core/src.
#                     A key nobody names is not core's — it belongs to a game package or a plugin.
# gamedata/cs2/**   : no key may appear in shim/src or core/src. A hit means the game package's
#                     namespace has leaked back into the engine-generic layer, which is exactly
#                     the boundary this tier exists to hold.
#
# Also checks that the master index and the files on disk agree in both directions: a file present
# but unlisted would silently never load, and a file listed but absent is a boot-time hard error.
# And that no `keys` section (per-game behavioural strings, spec §7) appears under gamedata/core/.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import json, pathlib, sys

def strip_jsonc_comments(src):
    # Ports packages/sdk/src/gamedata/jsonc.ts stripJsonComments() verbatim (same semantics,
    # same file format — two strippers that disagree on this format is its own bug). String-aware:
    # a `//` or `/*` inside a JSON string literal is content, not a comment, and a backslash-escaped
    # quote does not end the string. Comments are blanked (not deleted) so a JSONDecodeError's
    # reported line/column still lands on the author's actual line.
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

class GamedataLoadError(Exception):
    pass

def load(p):
    s = pathlib.Path(p).read_text()
    try:
        return json.loads(strip_jsonc_comments(s))
    except json.JSONDecodeError as e:
        raise GamedataLoadError(f'{p} is not valid JSON — {e}') from None

# Every C++/Rust source byte, concatenated once.
blob = []
for root in ('shim/src', 'core/src'):
    for p in pathlib.Path(root).rglob('*'):
        if p.is_file() and p.suffix in ('.cpp', '.h', '.rs'):
            blob.append(p.read_text(errors='ignore'))
blob = ''.join(blob)

FACT_SECTIONS = ('interfaces', 'offsets', 'signatures')
bad = []

for owner in ('core', 'cs2'):
    owner_dir = pathlib.Path('gamedata') / owner
    if not owner_dir.is_dir():
        bad.append(f'{owner}: owner directory missing')
        continue

    master_path = owner_dir / 'master.gamedata.jsonc'
    if not master_path.exists():
        bad.append(f'{owner}: master.gamedata.jsonc missing')
        continue
    try:
        master = load(master_path)
    except GamedataLoadError as e:
        bad.append(f'{owner}: {e}')
        continue

    listed = []
    for idx, entry in enumerate(master.get('files', [])):
        if 'file' not in entry:
            bad.append(f'{owner}: master.gamedata.jsonc files[{idx}] is missing a "file" key')
            continue
        listed.append(entry['file'])

    on_disk = sorted(p.name for p in owner_dir.glob('*.jsonc')
                     if p.name != 'master.gamedata.jsonc')
    for f in listed:
        if not (owner_dir / f).exists():
            bad.append(f'{owner}: master lists {f}, which does not exist')
    for f in on_disk:
        if f not in listed:
            bad.append(f'{owner}: {f} exists but is not listed in master — it will never load')

    for f in listed:
        p = owner_dir / f
        if not p.exists():
            continue
        try:
            j = load(p)
        except GamedataLoadError as e:
            bad.append(f'{owner}: {e}')
            continue
        if owner == 'core' and 'keys' in j:
            bad.append(f'core/{f}: `keys` (behavioural strings) is not permitted in core gamedata')
        for section in FACT_SECTIONS:
            for key in j.get(section, {}):
                named = f'"{key}"' in blob
                if owner == 'core' and not named:
                    bad.append(f'core/{f}: {section}.{key} is named nowhere in shim/src or '
                               f'core/src — it does not belong to the core owner')
                if owner == 'cs2' and named:
                    bad.append(f'cs2/{f}: {section}.{key} IS named in shim/src or core/src — '
                               f'the game-package namespace has leaked into core')

if bad:
    print('GAMEDATA OWNERSHIP VIOLATIONS:', file=sys.stderr)
    for b in bad:
        print(f'  {b}', file=sys.stderr)
    sys.exit(1)
print('check-gamedata-owners: ownership rule holds for every entry')
PY
