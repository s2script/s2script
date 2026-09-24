#!/usr/bin/env bash
# Live KHook suite runner. NOT a production-release step.
#
# One judge: scripts/khook_acceptance.py. --from-file and live mode share that
# parser/judge. Do not treat compilation or an rg match as a pass. Pending is
# not pass. Native shim/probe stay loaded; script reload is the final staged case.
# Native updates require server restart and a fresh run. Human observations are ingested; this runner never invents them.
#
#   bash scripts/test-khook-live.sh A
#   bash scripts/test-khook-live.sh --self-test
#   bash scripts/test-khook-live.sh A --from-file records.jsonl --identity runtime-identity.json
#   bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
#   bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
#   bash scripts/test-khook-live.sh A --judge --run-dir DIR --observations human.json
set -euo pipefail
cd "$(dirname "$0")/.."

CTRL=(python3 scripts/khook_acceptance.py)

usage() {
  echo "usage: $0 A|B|C [--from-file FILE] [--identity FILE] [--port N]" >&2
  echo "       $0 A --prepare|--collect|--judge --run-dir DIR [--identity FILE] [--observations FILE]" >&2
  echo "       $0 --self-test" >&2
  exit 2
}

self_test() {
  # The same executable fixtures exercise offline, persisted, and live collection
  # with explicit test receipts. These are parser regressions, not live evidence.
  python3 scripts/test-khook-acceptance.py
  echo "PASS: test-khook-live.sh --self-test"
}

FROM_FILE=""
SUITE=""
RUN_DIR=""
OBSERVATIONS=""
IDENTITY=""
PORT="${S2_RCON_PORT:-27015}"
DO_PREPARE=0
DO_COLLECT=0
DO_JUDGE=0

while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) self_test; exit 0 ;;
    --from-file)
      [ $# -ge 2 ] || usage
      FROM_FILE="$2"
      shift 2
      ;;
    --run-dir)
      [ $# -ge 2 ] || usage
      RUN_DIR="$2"
      shift 2
      ;;
    --identity)
      [ $# -ge 2 ] || usage
      IDENTITY="$2"
      shift 2
      ;;
    --observations)
      [ $# -ge 2 ] || usage
      OBSERVATIONS="$2"
      shift 2
      ;;
    --prepare) DO_PREPARE=1; shift ;;
    --collect) DO_COLLECT=1; shift ;;
    --judge) DO_JUDGE=1; shift ;;
    --port)
      [ $# -ge 2 ] || usage
      PORT="$2"
      shift 2
      ;;
    A|B|C|a|b|c)
      SUITE="$(echo "$1" | tr '[:lower:]' '[:upper:]')"
      shift
      ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

[ -n "$SUITE" ] || usage

if [ -n "$FROM_FILE" ]; then
  args=("$SUITE" --from-file "$FROM_FILE")
  [ -z "$IDENTITY" ] || args+=(--identity "$IDENTITY")
  [ -z "$OBSERVATIONS" ] || args+=(--observations "$OBSERVATIONS")
  "${CTRL[@]}" "${args[@]}"
  exit $?
fi

args=("$SUITE" --port "$PORT")
if [ -n "$IDENTITY" ]; then
  args+=(--identity "$IDENTITY")
fi
if [ -n "$RUN_DIR" ]; then
  args+=(--run-dir "$RUN_DIR")
fi
if [ -n "$OBSERVATIONS" ]; then
  args+=(--observations "$OBSERVATIONS")
fi
if [ "$DO_PREPARE" -eq 1 ]; then
  args+=(--prepare)
fi
if [ "$DO_COLLECT" -eq 1 ]; then
  args+=(--collect)
fi
if [ "$DO_JUDGE" -eq 1 ]; then
  args+=(--judge)
fi
"${CTRL[@]}" "${args[@]}"
exit $?
