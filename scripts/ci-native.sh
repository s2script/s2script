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

echo "== test-original-module.sh (verified original instruction images) =="
bash scripts/test-original-module.sh

echo "== test-engine-resolver.sh (recipe-aware original-image resolution) =="
bash scripts/test-engine-resolver.sh

echo "== test-sigscan.sh =="
bash scripts/test-sigscan.sh

echo "== test-gamedata.sh =="
bash scripts/test-gamedata.sh

echo "== check-deferred-sentinel.sh (S2_DISPATCH_DEFERRED: core == shim header) =="
bash scripts/check-deferred-sentinel.sh

echo "== check-engine-ops-order.sh (S2EngineOps field order: core == shim header) =="
bash scripts/check-engine-ops-order.sh

echo "== test-defer-queue.sh (the deferred-dispatch drain, flush-inside-replay included) =="
bash scripts/test-defer-queue.sh

echo "== test-client-bootstrap.sh (unsigned userid sentinel and late-load occupancy) =="
bash scripts/test-client-bootstrap.sh

echo "== test-hook-dispatch.sh (hook shape vocabulary, bypass latch, collapse) =="
bash scripts/test-hook-dispatch.sh

echo "== test-khook-binding.sh (checked KHook receipts, Observe/BeginRemove) =="
bash scripts/test-khook-binding.sh

echo "== test-khook-shutdown.sh (Unload Busy/Pending/Complete) =="
bash scripts/test-khook-shutdown.sh

echo "== test-khook-command.py (production ClientCommand and registered dispatch) =="
python3 scripts/test-khook-command.py

echo "== native_fixture_test.py (production acceptance driver) =="
python3 tools/khook-probe/testdata/native_fixture_test.py

echo "== controlled_evidence_test.py (strict native verdict evidence) =="
python3 tools/khook-probe/testdata/controlled_evidence_test.py

echo "== test-khook-observer.sh (consumed FireEvent observer + suite A fixtures) =="
bash scripts/test-khook-observer.sh

echo "== test-install-metamod.sh (artifact verifier + install transaction) =="
bash scripts/cloud/test-install-metamod.sh

echo "== test-metamod-build.py (unmodified source preparation + stale receipt invalidation) =="
python3 scripts/test-metamod-build.py

echo "== test-khook-acceptance.py (suite A judge/registry) =="
python3 scripts/test-khook-acceptance.py

echo "== test-khook-runtime-witness.py (mapped native module identity) =="
python3 scripts/test-khook-runtime-witness.py

echo "== test-khook-runtime-build.py (fresh source-bound acceptance bundle) =="
python3 scripts/test-khook-runtime-build.py

echo "== test-khook-runtime-identity.py (installed runtime receipt) =="
python3 scripts/test-khook-runtime-identity.py

echo "== test-khook-live.sh --self-test (shared judge via --from-file) =="
bash scripts/test-khook-live.sh --self-test

echo "== test-detour-reloc.sh (prologue relocation, tier selection, named refusals) =="
bash scripts/test-detour-reloc.sh

echo "== check-hook-shapes.sh (inbound hook shape name/id table: core == shim) =="
bash scripts/check-hook-shapes.sh

echo "== check-defer-selftest-gate.sh (S2_DEFER_SELFTEST: core == shim, registration-gated) =="
bash scripts/check-defer-selftest-gate.sh

echo "== test-call-validate.sh (the descriptor validators: both gates must REJECT) =="
bash scripts/test-call-validate.sh

echo "== check-gamedata-owners.sh (gamedata ownership boundary) =="
bash scripts/check-gamedata-owners.sh

echo '== check-call-descriptors.sh (every shipped `calls`/`hooks` descriptor is well-formed) =='
bash scripts/check-call-descriptors.sh

echo "== check-licenses-generated.sh =="
bash scripts/check-licenses-generated.sh

echo "== cargo build =="
cargo build

# Includes the integrated interop diagnostic/churn and protocol/API compatibility tests.
# scripts/test-interop.sh native runs that focused subset when iterating on acceptance.
echo "== cargo test -p s2script-core =="
cargo test -p s2script-core
bash scripts/test-async-pressure.sh

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

echo "== check-shim-symbols.sh (core entry points defined; no unresolvable engine symbols) =="
bash scripts/check-shim-symbols.sh

# The acceptance bundle includes a freshly built .s2sp with its own identity.
# Like ci-js, only CI refreshes node_modules; local runs use the installed SDK.
if [ -n "${CI:-}" ]; then
  echo "== npm ci (acceptance fixture build tools) =="
  npm ci
fi
echo "== build-khook-runtime.py (stock host, native consumers and stamped fixture) =="
python3 scripts/build-khook-runtime.py

echo "ci-native: all native gates passed"
