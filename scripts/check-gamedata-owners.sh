#!/usr/bin/env bash
# Enforces the gamedata OWNERSHIP rule (spec 2026-08-01-gamedata-tiering-design.md §4):
#
#   A key belongs to whoever NAMES it in source.
#
# gamedata/core/**  : every key MUST appear as a string literal in shim/src or core/src.
#                     A key nobody names is not core's — it belongs to a game package or a plugin.
# gamedata/<other>/ : no key may appear in shim/src or core/src. A hit means the game package's
#                     namespace has leaked back into the engine-generic layer, which is exactly
#                     the boundary this tier exists to hold.
#
# The OWNER SET is not hardcoded: it is read from the shim's kGamedataOwners[] table, so a
# gamedata/<owner>/ directory nothing loads fails here rather than sitting inert on disk.
#
# Also checks that the master index and the files on disk agree in both directions: a file present
# but unlisted would silently never load, and a file listed but absent is a boot-time hard error.
# And that no `keys` section (per-game behavioural strings, spec §7) appears under gamedata/core/.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import json, pathlib, re, sys

sys.path.insert(0, 'scripts/lib')
from jsonc import strip_jsonc_comments  # shared with check-gamedata-sigs.sh — one stripper, one file format

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

# Every section whose members are OWNED keys. `calls` (spec §5) is here from day one even though
# nothing emits one yet: A5b's whole job is moving 8 keys into gamedata/cs2/ AS `calls` descriptors,
# and a gate that exists to prove "the shim no longer names these" would silently skip every one of
# them if the section were absent. `keys` is deliberately NOT a fact section — it is checked
# separately (forbidden under core, unowned by definition elsewhere).
#
# `hooks` (declarative-inbound-hooks slice, 2026-08-02) is the INBOUND sibling of `calls`: gamedata/
# cs2/game.cs2.jsonc's `onTerminateRound`/`onRespawn` name the shim's `shape` vocabulary and a
# `signatures` target, never a game function — the same boundary `calls` holds, so it needs the same
# gate. Folded in from Task 5's review (task-6-brief.md item i): omitting it here left the ownership
# boundary UNENFORCED for every hook — no violation existed, but nothing would have caught one.
FACT_SECTIONS = ('interfaces', 'offsets', 'signatures', 'calls', 'hooks')
bad = []

# The owners the LOADER knows about, read out of the shim rather than hardcoded: the owner list is
# exactly the set of LoadGameConfig(...) call sites. An owner directory the loader never opens is
# data that can never load, and an owner the loader opens with no directory is a boot-time error —
# both must be loud here rather than discovered later.
table = re.search(r'kGamedataOwners\[\]\s*=\s*\{(.*?)\};', blob, re.S)
loader_owners = set(re.findall(r'"([^"]+)"', table.group(1))) if table else set()
if not loader_owners:
    bad.append('no kGamedataOwners[] table found in shim/src — this gate cannot tell which owners '
               'the loader reads (see s2script_mm.cpp)')
on_disk_owners = {p.name for p in pathlib.Path('gamedata').iterdir() if p.is_dir()}
for owner in sorted(on_disk_owners - loader_owners):
    bad.append(f'{owner}: gamedata/{owner}/ exists but "{owner}" is not in the shim\'s '
               f'kGamedataOwners[] table — the loader would never read the tree')

for owner in sorted(loader_owners | on_disk_owners):
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
                # The rule generalises past ('core','cs2'): core must name every key it owns, and
                # NO other owner's key may be named in the engine-generic layer.
                if owner == 'core' and not named:
                    bad.append(f'core/{f}: {section}.{key} is named nowhere in shim/src or '
                               f'core/src — it does not belong to the core owner')
                if owner != 'core' and named:
                    bad.append(f'{owner}/{f}: {section}.{key} IS named in shim/src or core/src — '
                               f'the {owner} namespace has leaked into core')

if bad:
    print('GAMEDATA OWNERSHIP VIOLATIONS:', file=sys.stderr)
    for b in bad:
        print(f'  {b}', file=sys.stderr)
    sys.exit(1)
print('check-gamedata-owners: ownership rule holds for every entry')
PY
