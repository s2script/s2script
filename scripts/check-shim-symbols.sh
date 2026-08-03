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

# ---------------------------------------------------------------------------
# Part 1: every WEAK-UNDEFINED s2script_core_* the shim references must be defined by the core it
# ships with. Runs whether or not a game install is present — it needs neither.
#
# WHY THIS EXISTS, and why the check above does not cover it. Core entry points the shim calls are
# `extern "C"`, so they never match the `^_Z` filter above; and some are declared
# `__attribute__((weak))` ON PURPOSE, so that a shim paired with an OLDER core degrades (null
# pointer, named WARN at Load) instead of failing dlopen and taking the whole addon down —
# s2script_core_dispatch_hook is the first. Weakness buys that at one cost: a MISSPELLING on the core
# side no longer fails the link either. It compiles, it links, it loads, and the feature is simply
# never delivered, with a WARN that reads exactly like a version mismatch. This is the gate that
# turns that back into a build failure.
#
# Pairs are explicit rather than a union of every core .so lying around: a stale `target/release`
# next to a fresh `target/debug` would otherwise vouch for a symbol the artifact under test does not
# have. build/shim is paired with the profile CMake actually linked (from its own cache); dist/ is
# paired with the core .so sitting beside it.
core_rc=0
core_pair() {  # $1 = shim artifact, $2 = its core .so
  local so="$1" core="$2"
  [ -f "$so" ] || return 0
  if [ ! -f "$core" ]; then
    echo "check-shim-symbols: SKIP core-symbol check for $so — no $core."
    return 0
  fi
  local weak defined missing
  # Weak-undefined (nm type 'w') AND plain-undefined ('U') core entry points. Both are checked: 'U'
  # would fail at dlopen rather than silently, but naming it here beats discovering it on a server.
  weak=$(nm -D --undefined-only "$so" | awk '$1 ~ /^[wU]$/ || $2 ~ /^[wU]$/ {print $NF}' \
         | grep -E '^s2script_core_' | sort -u || true)
  [ -n "$weak" ] || { echo "check-shim-symbols: ok — $so references no s2script_core_* entry points"; return 0; }
  defined=$(nm -D --defined-only "$core" | awk '{print $NF}' | sort -u)
  missing=$(comm -23 <(printf '%s\n' "$weak") <(printf '%s\n' "$defined") || true)
  if [ -n "$missing" ]; then
    echo "check-shim-symbols: FAIL — $so references core entry points $core does not define." >&2
    printf '%s\n' "$missing" | sed 's/^/    /' >&2
    echo >&2
    echo "  A WEAK reference resolves to null instead of failing the load, so this would ship as a" >&2
    echo "  silently dead feature. Check the spelling of the #[no_mangle] fn in core/src/ffi.rs" >&2
    echo "  against the declaration in shim/include/s2script_core.h." >&2
    core_rc=1
  else
    echo "check-shim-symbols: ok — $so's $(printf '%s\n' "$weak" | grep -c .) s2script_core_* entry point(s) all defined in $core"
  fi
}

CORE_PROFILE=release
if [ -f build/shim/CMakeCache.txt ]; then
  CORE_PROFILE=$(sed -n 's/^S2_CORE_LIB_DIR:STRING=//p' build/shim/CMakeCache.txt | head -1)
  [ -n "$CORE_PROFILE" ] || CORE_PROFILE=release
fi
core_pair build/shim/s2script.so "target/$CORE_PROFILE/libs2script_core.so"
core_pair dist/addons/s2script/bin/linuxsteamrt64/s2script.so \
          dist/addons/s2script/bin/linuxsteamrt64/libs2script_core.so
[ "$core_rc" -eq 0 ] || exit 1

# ---------------------------------------------------------------------------
# Part 2: the engine-symbol check (needs the game install).
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
