#!/usr/bin/env bash
# The update-day checks in one run: signatures, vtable pins, tracked index alignment, the
# every-vtable slot diff, the offline schema regen with its diff, and (with gdgen) the gamedata
# regeneration plus the s2sdk check. Writes a Markdown report.
#
#   tools/gamedata-offline/update-day.sh --new <dir> [--old <dir>] --out <dir>
#
# <dir>s are flat module directories (fetch-depot.sh makes them). Without --old only signatures,
# pins and the schema regen run. Exit 0 = nothing for s2script to fix (the schema may still have
# changed: regenerate it), 1 = something needs a look, 2 = the checker itself failed.
#
# gdgen (github.com/s2script/s2-gamedatagen) runs when $GDGEN names its binary or `gdgen` is on
# PATH. It writes <out>/gamedata.patch (git apply it) and <out>/regen.md, which lists only what it
# could not regenerate. The s2sdk check inside it needs clang++ and the third_party/hl2sdk submodule.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
new= old= out=
while [ $# -gt 0 ]; do
  case $1 in
    --new) new=$2; shift 2 ;;
    --old) old=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 2 ;;
  esac
done
[ -n "$new" ] && [ -n "$out" ] || { echo "usage: $0 --new <dir> [--old <dir>] --out <dir>" >&2; exit 2; }
mkdir -p "$out"

args=(--bin "$new" --json "$out/check.json" --markdown "$out/check.md" --strict)
[ -n "$old" ] && args+=(--old "$old" --all-vtables)
python3 -I "$here/check.py" "${args[@]}" > "$out/check.log" 2>&1
check=$?
if [ "$check" -gt 1 ]; then
  echo "check.py crashed (exit $check):" >&2
  tail -40 "$out/check.log" >&2
  exit 2
fi

catalog=$root/games/cs2/gamedata/schema-catalog.json
python3 -I "$here/schema.py" --bin "$new" --prev "$catalog" \
  --out "$out/schema-catalog.json" --enums "$out/schema-enums.json" > "$out/schema.log" 2>&1
schema=$?
if [ "$schema" -eq 0 ]; then
  python3 -I "$here/schema_diff.py" "$catalog" "$out/schema-catalog.json" > "$out/schema.md"
else
  printf '### Schema catalog\n\nschema.py failed (exit %s):\n\n```\n%s\n```\n' \
    "$schema" "$(tail -20 "$out/schema.log")" > "$out/schema.md"
fi

gdgen=${GDGEN:-$(command -v gdgen || true)}
regen=-1
if [ -n "$gdgen" ] && [ -n "$old" ]; then
  if [ ! -d "$root/third_party/hl2sdk/public" ]; then
    git -C "$root" submodule update --init --depth 1 third_party/hl2sdk > "$out/submodule.log" 2>&1 || true
  fi
  "$gdgen" regen --repo "$root" --old "$old" --new "$new" --out "$out" --strict > /dev/null 2> "$out/regen.log"
  regen=$?
  if [ "$regen" -gt 1 ]; then
    printf '### Gamedata regeneration (gdgen)\n\ngdgen failed (exit %s):\n\n```\n%s\n```\n' \
      "$regen" "$(tail -20 "$out/regen.log")" > "$out/regen.md"
  fi
elif [ -z "$gdgen" ]; then
  printf '### Gamedata regeneration (gdgen)\n\nNot run: gdgen is not installed (set GDGEN).\n' > "$out/regen.md"
else
  printf '### Gamedata regeneration (gdgen)\n\nNot run: no old build to regenerate from.\n' > "$out/regen.md"
fi

python3 -I - "$new" > "$out/modules.md" <<'PY'
import hashlib, pathlib, sys
from elftools.elf.elffile import ELFFile
print('### Modules\n\n| File | Size | sha256 | GNU BuildID |\n|---|---|---|---|')
for p in sorted(pathlib.Path(sys.argv[1]).glob('*.so')):
    bid = ''
    with open(p, 'rb') as f:
        for sec in ELFFile(f).iter_sections():
            if sec.name == '.note.gnu.build-id':
                for n in sec.iter_notes():
                    bid = n['n_desc']
    print(f'| {p.name} | {p.stat().st_size} | `{hashlib.sha256(p.read_bytes()).hexdigest()}` | `{bid}` |')
PY

{
  cat "$out/regen.md"
  echo
  cat "$out/check.md"
  echo
  cat "$out/schema.md"
  echo
  cat "$out/modules.md"
} > "$out/report.md"

[ "$schema" -eq 0 ] || exit 1
# gdgen leaving something unresolved (or failing) needs a look even when check.py is clean.
[ "$regen" -ge 1 ] && exit 1
exit "$check"
