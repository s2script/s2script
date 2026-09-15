#!/usr/bin/env bash
# Build the unmodified stock host and both consumers against GLIBC <= 2.31.
# This is part of ci-native, not a separately maintained workflow gate.
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" != "--container" ]]; then
  command -v docker >/dev/null || { echo "error: Docker is required for the KHook sniper build" >&2; exit 1; }
  exec docker run --rm --platform linux/amd64 -v "$PWD:/repo" -w /repo \
    rust:bullseye bash /repo/scripts/test-khook-sniper-build.sh --container
fi

export DEBIAN_FRONTEND=noninteractive
# Bullseye security packages are moving out of the live mirrors. Use the
# signed snapshot for this compatibility build so indexes and .debs agree.
cat > /etc/apt/s2script-snapshot.list <<'APT_SOURCES'
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20260831T235959Z/ bullseye main
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian/20260831T235959Z/ bullseye-updates main
deb [check-valid-until=no] https://snapshot.debian.org/archive/debian-security/20260831T235959Z/ bullseye-security main
APT_SOURCES
cat > /etc/apt/apt.conf.d/99s2script-snapshot <<'APT_CONFIG'
Dir::Etc::sourcelist "s2script-snapshot.list";
Dir::Etc::sourceparts "-";
Acquire::Retries "3";
APT::Update::Error-Mode "any";
APT_CONFIG
apt-get update -qq
apt-get install -y -qq build-essential binutils clang curl git python3 python3-pip >/dev/null
git config --global --add safe.directory '*'
# AMBuild is a build tool, pinned independently from the runtime source gitlinks.
python3 -m pip install --disable-pip-version-check --quiet \
  'git+https://github.com/alliedmodders/ambuild.git@01212cb57c96561f664b6dfb2ee10e66de6f81e4'

echo '== optional pinned stock Metamod AMBuild (sniper) =='
CC=clang CXX=clang++ bash scripts/build-metamod-pinned.sh || {
  tail -100 build/metamod-pinned/build.log
  exit 1
}
python3 scripts/verify-metamod-artifact.py --tree build/metamod-pinned/tree \
  --manifest build/metamod-pinned/metamod-build.json

echo '== shim/core sniper build =='
bash scripts/build-sniper.sh
export PATH="/opt/cmake-3.28.6-linux-x86_64/bin:$PATH"
echo '== optimized acceptance probe build =='
cmake -S tools/khook-probe -B build/khook-probe -DCMAKE_BUILD_TYPE=Release
cmake --build build/khook-probe -j2
python3 - <<'PY'
import importlib.util
from pathlib import Path
spec = importlib.util.spec_from_file_location("artifact_verifier", "scripts/verify-metamod-artifact.py")
verifier = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verifier)
for filename in ("build/shim/s2script.so", "target/release/libs2script_core.so",
                 "build/khook-probe/s2_khook_probe.so"):
    verifier.check_elf(Path(filename), filename)
    print(f"verified sniper ELF: {filename}")
PY
echo 'KHook sniper build: stock host, shim/core, and probe verified'
