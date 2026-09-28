#!/usr/bin/env bash
# Game-package boundary gate over PRODUCTION core/shim source. Core and shim select and bootstrap a
# game package generically; they must never name one, load its prelude by path, or keep the
# bespoke damage path the package's trusted takeDamageOld function replaced.
#
# Production = everything before a file's top-level `#[cfg(test)]` (Rust tests live at the bottom),
# excluding comment lines and the test-only trees/files below. Identity-specific fixtures belong in
# those test files, never inline in a production module.
#
# `--self-test` plants a production literal (must fail) and a test-module literal (must pass).
set -euo pipefail
cd "$(dirname "$0")/.."

PACKAGE_RE='@s2script/cs2|pawn\.js|Cs2JsPath'
DAMAGE_RE='s2script_core_dispatch_damage|__s2_damage_|InstallDamage'

production_files() {
  find core/src shim/src shim/include -type f \( -name '*.rs' -o -name '*.cpp' -o -name '*.h' \) \
    -not -path '*/tests/*' -not -name 'tests.rs' -not -name '*_tests.rs' -not -name '*_test.cpp'
}

# Print `file:line:text` for production, non-comment lines of one file.
production_lines() {
  awk -v f="$1" '
    /^#\[cfg\(test\)\]/ { exit }
    { t = $0; sub(/^[ \t]+/, "", t) }
    t ~ /^(\/\/|\/\*|\*)/ { next }
    { print f ":" NR ":" $0 }
  ' "$1"
}

scan() {
  local status=0 file hits
  while IFS= read -r file; do
    hits=$(production_lines "$file" | grep -E "$PACKAGE_RE" || true)
    if [[ -n "$hits" ]]; then
      echo "$hits" >&2; echo "BOUNDARY VIOLATION: game-package literal in production core/shim" >&2; status=1
    fi
    hits=$(production_lines "$file" | grep -E "$DAMAGE_RE" || true)
    if [[ -n "$hits" ]]; then
      echo "$hits" >&2; echo "BOUNDARY VIOLATION: bespoke damage path remains in production core/shim" >&2; status=1
    fi
  done < <(production_files)
  return $status
}

if [[ "${1:-}" == "--self-test" ]]; then
  probe="core/src/__game_package_boundary_probe.rs"
  trap 'rm -f "$probe"' EXIT
  printf 'const _P: &str = "@s2script/cs2";\n' > "$probe"
  if scan 2>/dev/null; then echo "FAIL: gate missed a production package literal" >&2; exit 1; fi
  printf 'fn _f() {}\n#[cfg(test)]\nmod tests { const _P: &str = "@s2script/cs2"; }\n' > "$probe"
  if ! scan; then echo "FAIL: gate rejected a test-module fixture literal" >&2; exit 1; fi
  printf '// resolves the @s2script/cs2 package\nfn _f() { let _d = 1; } // __s2_damage_read\n' > "$probe"
  if scan 2>/dev/null; then echo "FAIL: gate missed a trailing-comment-line damage literal" >&2; exit 1; fi
  echo "game-package boundary self-test OK"
  exit 0
fi

scan
echo "game-package boundary OK: no package literal or bespoke damage path in production core/shim"
