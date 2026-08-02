#!/usr/bin/env bash
# THE native (Rust + C++) gate suite. Single source of truth: .github/workflows/ci-native.yml
# runs exactly this script and nothing else, and so does `make ci-native`. If a gate is not
# in here, it does not run — do not add a gate step to the workflow YAML.
#
# Cheap gates first: a boundary violation should fail in seconds, not after a build.
set -euo pipefail
cd "$(dirname "$0")/.."

# Populates the cargo registry that check-licenses-generated.sh reads every locked crate's
# license text out of, and warms it for the build below.
echo "== cargo fetch --locked =="
cargo fetch --locked

echo "== check-core-boundary.sh (dependency closure + name gates) =="
bash scripts/check-core-boundary.sh

echo "== test-boundary-nameleak.sh =="
bash scripts/test-boundary-nameleak.sh

echo "== test-sigscan.sh =="
bash scripts/test-sigscan.sh

echo "== test-gamedata.sh =="
bash scripts/test-gamedata.sh

echo "== check-deferred-sentinel.sh (S2_DISPATCH_DEFERRED: core == shim header) =="
bash scripts/check-deferred-sentinel.sh

echo "== test-defer-queue.sh (the deferred-dispatch drain, flush-inside-replay included) =="
bash scripts/test-defer-queue.sh

echo "== check-defer-selftest-gate.sh (S2_DEFER_SELFTEST: core == shim, registration-gated) =="
bash scripts/check-defer-selftest-gate.sh

echo "== test-call-validate.sh (the descriptor validators: both gates must REJECT) =="
bash scripts/test-call-validate.sh

echo "== check-gamedata-owners.sh (gamedata ownership boundary) =="
bash scripts/check-gamedata-owners.sh

echo "== check-licenses-generated.sh =="
bash scripts/check-licenses-generated.sh

echo "== cargo build =="
cargo build

echo "== cargo test -p s2script-core =="
cargo test -p s2script-core

# ccache is present in CI via hendrikmuhs/ccache-action; on a dev box it may not be.
# Only pass the launcher when it actually exists, so cmake does not fail on a missing binary.
LAUNCHER=()
if command -v ccache >/dev/null 2>&1; then
  LAUNCHER=(-DCMAKE_CXX_COMPILER_LAUNCHER=ccache)
fi

echo "== check-gamedata-sigs.sh (no build-specific operands in a signature) =="
bash scripts/check-gamedata-sigs.sh

echo "== check-invoke-abi.sh (declared-call float ABI) =="
bash scripts/check-invoke-abi.sh

# The sniper build (scripts/build-sniper.sh) runs in a container where the repo is /repo, so it
# leaves a CMakeCache.txt pointing at /repo. A later host build then dies with "current
# CMakeCache.txt directory ... is different". Detect a cache from a different source tree and
# discard it, rather than making every developer learn this by hitting it.
if [ -f build/shim/CMakeCache.txt ] && ! grep -q "CMAKE_HOME_DIRECTORY:INTERNAL=$PWD/shim\$" build/shim/CMakeCache.txt; then
  echo "== discarding a CMake cache from another source tree (sniper build) =="
  rm -rf build/shim
fi

echo "== shim build =="
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release \
  -DS2_CORE_LIB_DIR=debug \
  ${LAUNCHER[@]+"${LAUNCHER[@]}"}
cmake --build build/shim -j

echo "== ccommand_selftest (our CCommand tokenizer) =="
cmake --build build/shim --target ccommand_selftest -j >/dev/null
./build/shim/ccommand_selftest

echo "== check-shim-symbols.sh (no unresolvable engine symbols) =="
bash scripts/check-shim-symbols.sh

echo "ci-native: all native gates passed"
