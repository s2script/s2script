#!/usr/bin/env bash
# Verifies the name-leak gate FAILS when a CS2 identifier is present in core/ and PASSES when clean.
# Also verifies the include_str!/games/ gate fires on a planted violation and passes when clean.
set -uo pipefail
cd "$(dirname "$0")/.."

# 1. Clean tree must pass.
if ! bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  echo "FAIL: gate rejected a clean core/"; exit 1
fi

# 2. Plant a CS2 name in a temp core file; gate must fail.
tmp="core/src/__nameleak_probe.rs"
echo '// CCSPlayerPawn m_iHealth' > "$tmp"
if bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  rm -f "$tmp"; echo "FAIL: gate did not catch a CS2 name in core/"; exit 1
fi
rm -f "$tmp"

# 3. Clean tree must still pass (post-cleanup).
if ! bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  echo "FAIL: gate rejected a clean core/ after CS2-name cleanup"; exit 1
fi

# 4. Plant a games/ include_str! in a temp core file; gate must fail.
tmp2="core/src/__games_include_probe.rs"
echo 'const _: &str = include_str!("../../games/cs2/js/pawn.js");' > "$tmp2"
if bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  rm -f "$tmp2"; echo "FAIL: gate did not catch an include_str! of a games/ file in core/"; exit 1
fi
rm -f "$tmp2"

# 5. Clean tree must pass again.
if ! bash scripts/check-core-boundary.sh >/dev/null 2>&1; then
  echo "FAIL: gate rejected a clean core/ after include_str! cleanup"; exit 1
fi

echo "PASS: name-leak gate catches CS2 identifiers and passes when clean"
echo "PASS: include_str!/games/ gate catches embedded game files and passes when clean"
