#!/usr/bin/env bash
# Exercise the actual production decoder with the pinned SDK variant layout.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
python3 - "$tmp/named_output_adapter.inc" <<'PY'
from pathlib import Path
import sys
source = Path('shim/src/s2script_mm.cpp').read_text()
start = source.index('// Format a CVariant\'s value as a string')
end = source.index('// ---------------------------------------------------------------------------\n// Load', start)
Path(sys.argv[1]).write_text(source[start:end])
PY
compiler="${CXX:-c++}"
flags=(-std=c++17 -O1 -g -pthread -DPOSIX -DCOMPILER_GCC -DPLATFORM_64BITS -Dstricmp=strcasecmp
       -I"$tmp" -isystem third_party/hl2sdk/public
       -isystem third_party/hl2sdk/public/tier0 -isystem third_party/hl2sdk/public/tier1
       -isystem third_party/hl2sdk/common -isystem third_party/hl2sdk/public/mathlib)
if [[ "$(uname -s)" == Linux ]]; then flags+=(-DLINUX -D_LINUX); fi
if printf 'int main() { return 0; }\n' | "$compiler" -x c++ - -fsanitize=address,undefined -o "$tmp/probe" >/dev/null 2>&1 && "$tmp/probe"; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all -fno-omit-frame-pointer)
  if [[ "$(uname -s)" == Linux ]]; then flags+=(-no-pie); fi
  echo 'named_output_value: ASan + UBSan enabled'
fi
"$compiler" "${flags[@]}" shim/tests/named_output_value_test.cpp -o "$tmp/test"
"$tmp/test"
