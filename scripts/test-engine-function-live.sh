#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ${1:-} == --fixture-only && ${2:-} == --orders && ${3:-} == peer-first,s2-first && $# == 3 ]]; then
  bash scripts/test-engine-function-abi.sh --stock-provider
  build/engine-function-abi/engine_function_abi_test --peers
elif [[ ${1:-} == --docker ]]; then
  exec python3 tools/engine-function-probe/live.py "$@"
else
  echo 'usage: test-engine-function-live.sh --fixture-only --orders peer-first,s2-first' >&2
  echo '   or: test-engine-function-live.sh --docker <compose> --rcon <rcon.py> [--bundle <built bundle>]' >&2
  exit 2
fi
