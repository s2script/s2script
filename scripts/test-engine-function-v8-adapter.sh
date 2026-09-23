#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ ${1:-} == --spike && ${2:-} == --stock-provider && $# == 2 ]] || { echo 'usage: test-engine-function-v8-adapter.sh --spike --stock-provider' >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: V8 stock provider proof requires linux-x86_64-sysv' >&2; exit 2; }
bash scripts/test-engine-function-abi.sh --stock-provider
export S2FN_V8_BRIDGE="$PWD/build/engine-function-abi/libengine_function_v8_bridge.so"
[[ -f "$S2FN_V8_BRIDGE" ]] || { echo 'FAIL missing real provider bridge' >&2; exit 1; }
cargo test --locked -p s2script-core --lib v8host::engine_function_adapter_v8::busy_caller_stock_provider_spike -- --ignored --exact --nocapture
