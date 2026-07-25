#!/usr/bin/env bash
# Fails (exit 1) if the built shim needs an engine symbol no shipped CS2 binary exports.
#
# WHY THIS GATE EXISTS. A shared library links happily with undefined symbols — they are only
# resolved at dlopen. So a header that DECLARES a method the engine never EXPORTS compiles, links,
# passes `make shim`, and then dies on the live server with:
#
#   [META] Failed to load plugin ...: undefined symbol: _ZN8CCommand8TokenizeE10CUtlStringP14characterset_t
#
# That is exactly how `CCommand::Tokenize` was discovered: it is declared in the vendored
# tier1/convar.h but no shipped binary defines it (inlined or stripped). The only symptom was the
# whole addon failing to load — every plugin down, on a real server. This check moves that discovery
# to the build.
#
# Only C++-mangled (_Z…) symbols are considered: libc/libstdc++/pthread symbols legitimately resolve
# from the system, not from the game, and would be false positives.
#
# SKIPs rather than fails when the game install is unavailable (CI without the 74GB depot), so it can
# be wired in unconditionally.
set -euo pipefail
cd "$(dirname "$0")/.."

# Check every shim artifact present. The host build is what CI produces; dist/ holds the SNIPER
# build, which is the ONLY deployable — and therefore the one that actually has to dlopen. They are
# built from the same source, so the host build normally catches this first, but checking the
# artifact that ships costs nothing and is the one whose failure took a server down.
SHIMS=()
for cand in build/shim/s2script.so dist/addons/s2script/bin/linuxsteamrt64/s2script.so; do
  [ -f "$cand" ] && SHIMS+=("$cand")
done
if [ ${#SHIMS[@]} -eq 0 ]; then
  echo "check-shim-symbols: SKIP — no shim built (build/shim or dist/)."
  exit 0
fi

GAME="${S2SCRIPT_GAME_DIR:-docker/cs2-data/game}"
if [ ! -d "$GAME" ]; then
  echo "check-shim-symbols: SKIP — no game install at $GAME (set S2SCRIPT_GAME_DIR to enable)."
  exit 0
fi

need=$(mktemp); have=$(mktemp); trap 'rm -f "$need" "$have"' EXIT

# Everything every shipped game binary defines — computed once, reused per artifact.
find "$GAME" -name '*.so' 2>/dev/null | while read -r f; do
  nm -D --defined-only "$f" 2>/dev/null | awk '{print $NF}'
done | sort -u > "$have"

rc=0
for SO in "${SHIMS[@]}"; do
  # Mangled symbols this artifact needs from somewhere other than itself.
  nm -D --undefined-only "$SO" | awk '{print $NF}' | grep -E '^_Z' | sort -u > "$need"

  # A mangled symbol that is neither in the game nor obviously from the C++ runtime.
  missing=$(comm -23 "$need" "$have" | grep -vE '^_ZN?S|^_ZSt|^_ZTI|^_ZTS|^_ZTV|^_Znw|^_Zdl|^_ZGV|^_ZNK?St|@GLIBCXX|@CXXABI' || true)

  if [ -n "$missing" ]; then
    echo "check-shim-symbols: FAIL — $SO references engine symbols nothing exports." >&2
    echo "  These resolve at dlopen, so the addon would fail to load on a real server:" >&2
    echo "$missing" | c++filt | sed 's/^/    /' >&2
    echo >&2
    echo "  A declaration in the vendored SDK header is NOT proof the engine exports it." >&2
    rc=1
  else
    echo "check-shim-symbols: ok — $SO"
  fi
done
exit $rc
