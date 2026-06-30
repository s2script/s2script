#!/usr/bin/env bash
# Injects the Metamod:Source SearchPath into csgo/gameinfo.gi (idempotent).
# Run ONCE after the first CS2 download completes, before restarting the server:
#   docker exec s2script-cs2 /patch-gameinfo.sh
#
# Confirmed paths (joedwards32/cs2, STEAMAPPDIR=/home/steam/cs2-dedicated):
#   game dir: /home/steam/cs2-dedicated/game/csgo
#   gameinfo:  /home/steam/cs2-dedicated/game/csgo/gameinfo.gi
set -euo pipefail
GI="${1:-/home/steam/cs2-dedicated/game/csgo/gameinfo.gi}"

if [ ! -f "$GI" ]; then
    echo "ERROR: gameinfo.gi not found at $GI" >&2
    echo "Has CS2 finished downloading? (first container start triggers a ~30GB download)" >&2
    exit 1
fi

if grep -q "csgo/addons/metamod" "$GI"; then
    echo "gameinfo.gi already patched"; exit 0
fi

# Use Python3 (available in the image) for robust, whitespace-preserving insertion.
# Inserts 'Game   csgo/addons/metamod' before the first SearchPaths entry that is
# exactly 'Game   csgo' (not csgo_lv, not csgo/bin/...) so metamod is found first.
python3 - "$GI" <<'PYEOF'
import sys, re

path = sys.argv[1]
text = open(path, 'r').read()

# Match exactly: indentation + "Game" + whitespace + "csgo" + optional trailing space + EOL
# Does NOT match "csgo_lv", "csgo/bin/...", etc.
pattern = re.compile(r'^([ \t]+Game[ \t]+csgo)[ \t]*$', re.MULTILINE)
match = pattern.search(text)
if not match:
    print("ERROR: Could not find bare 'Game  csgo' line in SearchPaths block", file=sys.stderr)
    sys.exit(1)

# Reconstruct leading indentation from the matched line
indent = re.match(r'^([ \t]*)', match.group(1)).group(1)
metamod_entry = indent + 'Game\t\t\t\tcsgo/addons/metamod'

new_text = pattern.sub(lambda m: metamod_entry + '\n' + m.group(0), text, count=1)
if new_text == text:
    print("ERROR: substitution left text unchanged", file=sys.stderr)
    sys.exit(1)

open(path, 'w').write(new_text)
print("patched: " + path)
PYEOF

echo "SearchPaths after patch:"
grep -n "metamod\|Game.*csgo\|GameBin" "$GI" || true
