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

echo "== shim build =="
cmake -S shim -B build/shim -DCMAKE_BUILD_TYPE=Release \
  -DS2_CORE_LIB_DIR=debug \
  ${LAUNCHER[@]+"${LAUNCHER[@]}"}
cmake --build build/shim -j

echo "ci-native: all native gates passed"
