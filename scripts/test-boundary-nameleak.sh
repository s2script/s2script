#!/usr/bin/env bash
# Verifies the name-leak gate FAILS when a CS2 identifier is present in core/ and PASSES when clean.
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
echo "PASS: name-leak gate catches CS2 identifiers and passes when clean"
