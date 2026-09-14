#!/usr/bin/env bash
# Live KHook suite runner. NOT a production-release step.
#
# One judge: scripts/khook_acceptance.py. --from-file and live mode share that
# parser/judge. Do not treat compilation or an rg match as a pass. Pending is
# not pass. Human observations are ingested; this runner never invents them.
#
#   bash scripts/test-khook-live.sh A
#   bash scripts/test-khook-live.sh --self-test
#   bash scripts/test-khook-live.sh A --from-file records.jsonl
#   bash scripts/test-khook-live.sh A --prepare --run-dir build/khook-acceptance/run-001
#   bash scripts/test-khook-live.sh A --collect --run-dir build/khook-acceptance/run-001
#   bash scripts/test-khook-live.sh A --judge --run-dir DIR --observations human.json
set -euo pipefail
cd "$(dirname "$0")/.."

CTRL=(python3 scripts/khook_acceptance.py)

usage() {
  echo "usage: $0 A|B|C [--from-file FILE] [--port N]" >&2
  echo "       $0 A --prepare|--collect|--judge --run-dir DIR [--observations FILE]" >&2
  echo "       $0 --self-test" >&2
  exit 2
}

emit_fixture() {
  local kind="$1"
  local run_id="${2:-self-test}"
  "${CTRL[@]}" A --emit-fixture "$kind" --identity-run-id "$run_id"
}

self_test() {
  local tmp st
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' RETURN

  emit_fixture all-pass self-test-all-pass >"$tmp/all.jsonl"
  if ! "$0" A --from-file "$tmp/all.jsonl" >/dev/null; then
    echo "FAIL: self-test all-pass should exit 0"
    exit 1
  fi

  emit_fixture duplicate self-test-dup >"$tmp/dup.jsonl"
  if "$0" A --from-file "$tmp/dup.jsonl" >/dev/null; then
    echo "FAIL: self-test duplicate should be nonzero"
    exit 1
  fi

  emit_fixture missing self-test-missing >"$tmp/missing.jsonl"
  if "$0" A --from-file "$tmp/missing.jsonl" >/dev/null; then
    echo "FAIL: self-test missing should be nonzero"
    exit 1
  fi

  emit_fixture fail self-test-fail >"$tmp/fail.jsonl"
  if "$0" A --from-file "$tmp/fail.jsonl" >/dev/null; then
    echo "FAIL: self-test required fail should be nonzero"
    exit 1
  fi

  emit_fixture pending self-test-pending >"$tmp/pending.jsonl"
  set +e
  "$0" A --from-file "$tmp/pending.jsonl" >/dev/null
  st=$?
  set -e
  if [ "$st" -eq 0 ]; then
    echo "FAIL: self-test pending must not exit 0"
    exit 1
  fi
  if [ "$st" -ne 2 ]; then
    echo "FAIL: self-test pending expected exit 2, got $st"
    exit 1
  fi

  emit_fixture native-fail-js-pass self-test-js-overwrite >"$tmp/js.jsonl"
  set +e
  local merged_out
  merged_out="$("$0" A --from-file "$tmp/js.jsonl" 2>&1)"
  st=$?
  set -e
  if [ "$st" -eq 0 ]; then
    echo "FAIL: self-test JS must not overwrite a native failure"
    echo "$merged_out"
    exit 1
  fi
  if echo "$merged_out" | grep -q "^PASS: frame_client_command_hooks$"; then
    echo "FAIL: self-test JS upgraded frame_client_command_hooks after native fail"
    echo "$merged_out"
    exit 1
  fi

  echo '{"suite":"B","case":"not_authored","result":"pass"}' >"$tmp/b.jsonl"
  set +e
  "$0" B --from-file "$tmp/b.jsonl" >/dev/null
  st=$?
  set -e
  if [ "$st" -eq 0 ]; then
    echo "FAIL: self-test suite B must be unavailable"
    exit 1
  fi

  echo "PASS: test-khook-live.sh --self-test"
}

FROM_FILE=""
SUITE=""
RUN_DIR=""
OBSERVATIONS=""
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
      SUITE="${1^^}"
      shift
      ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

[ -n "$SUITE" ] || usage

if [ -n "$FROM_FILE" ]; then
  "${CTRL[@]}" "$SUITE" --from-file "$FROM_FILE"
  exit $?
fi

args=("$SUITE" --port "$PORT")
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
