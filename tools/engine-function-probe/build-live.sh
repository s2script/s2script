#!/usr/bin/env bash
# Run inside Bullseye. Builds consumers only; never builds/replaces Metamod.
set -euo pipefail
cd "$(dirname "$0")/../.."
# Host CI may compile with a newer libc, but that output is never a deployable
# bundle. The opt-in is valid only on the probe-only path.
compile_only=0
if [[ $# == 2 && $1 == --probe-only && $2 == --compile-only ]]; then
  compile_only=1
elif [[ $# != 0 && !( $# == 1 && $1 == --probe-only ) ]]; then
  echo 'usage: build-live.sh [--probe-only [--compile-only]]' >&2
  exit 2
fi
for name in S2_BUILD_JOBS CARGO_BUILD_JOBS; do
  value=${!name:-}
  [[ -z "$value" || "$value" =~ ^[1-9][0-9]*$ ]] || { echo "error: $name must be a positive integer" >&2; exit 2; }
done
[[ ${S2FN_BUILD_TOKEN:-} =~ ^[0-9a-f]{64}$ && ${S2FN_SOURCE_REVISION:-} =~ ^[0-9a-f]{40}$ ]] || { echo 'source-bound build identity required' >&2; exit 2; }
[[ "$(git rev-parse HEAD)" == "$S2FN_SOURCE_REVISION" ]] || { echo 'source revision mismatch' >&2; exit 2; }
[[ "$(uname -s)/$(uname -m)" == Linux/x86_64 ]] || { echo 'Linux x86_64 required' >&2; exit 2; }
if [[ ${1:-} != --probe-only ]]; then
  [[ $# == 0 ]] || exit 2
  # Same signed package snapshot as the existing acceptance Bullseye gate.
  cat > /etc/apt/s2script-snapshot.list <<'APT'
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20260831T235959Z/ bullseye main
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20260831T235959Z/ bullseye-updates main
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/20260831T235959Z/ bullseye-security main
APT
  cat > /etc/apt/apt.conf.d/99s2script-snapshot <<'APT'
Dir::Etc::sourcelist "s2script-snapshot.list";
Dir::Etc::sourceparts "-";
Acquire::Retries "3";
APT::Update::Error-Mode "any";
APT
  bash scripts/build-sniper.sh
fi
export PATH="/opt/cmake-3.28.6-linux-x86_64/bin:$PATH"
cmake -S tools/engine-function-probe -B build/engine-function-live/native \
  -DS2FN_LIVE_PLUGIN=ON -DCMAKE_BUILD_TYPE=Release \
  -DS2FN_BUILD_TOKEN="$S2FN_BUILD_TOKEN" -DS2FN_SOURCE_REVISION="$S2FN_SOURCE_REVISION"
cmake --build build/engine-function-live/native --parallel "${S2_BUILD_JOBS:-2}"
probe=build/engine-function-live/native/s2_engine_function_probe.so
mkdir -p build/engine-function-live/link-evidence
ldd -r "$probe" > build/engine-function-live/link-evidence/ldd.txt 2>&1
nm -D -C "$probe" > build/engine-function-live/link-evidence/exports.txt
objdump -T "$probe" > build/engine-function-live/link-evidence/symbols.txt
if grep -E 'lib(ffi|ltdl)\.so|not found' build/engine-function-live/link-evidence/ldd.txt; then exit 1; fi
if grep 'undefined symbol:' build/engine-function-live/link-evidence/ldd.txt | grep -v 'g_pMemAlloc'; then exit 1; fi
if grep -E ' [TWBD] (ffi_|KHook::(CreateHook|RemoveHook)|safetyhook::)' build/engine-function-live/link-evidence/exports.txt; then exit 1; fi
python3 - "$compile_only" <<'PY'
import re
import sys
from pathlib import Path
text=Path('build/engine-function-live/link-evidence/symbols.txt').read_text()
versions=[tuple(map(int,v.split('.'))) for v in re.findall(r'GLIBC_([0-9.]+)',text)]
if not versions: raise SystemExit('probe GLIBC version evidence missing')
print('live probe GLIBC maximum:', '.'.join(map(str,max(versions))))
if sys.argv[1]=='1':
    print('host compile-only output: not a deployable bundle or live acceptance receipt')
elif max(versions)>(2,31):
    raise SystemExit('probe exceeds GLIBC_2.31')
PY
