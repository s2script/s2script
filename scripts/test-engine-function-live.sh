#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ${1:-} == --fixture-only && ${2:-} == --orders && ${3:-} == peer-first,s2-first && $# == 3 ]]; then
  bash scripts/test-engine-function-abi.sh --stock-provider
  build/engine-function-abi/engine_function_abi_test --peers
else
  echo 'PENDING: deployable CS2 engine-function probe and compatibility fixtures have not passed the bounded stock-provider/V8 gate.' >&2
  echo 'The live gate cannot report success from portable or standalone evidence.' >&2
  exit 2
fi
