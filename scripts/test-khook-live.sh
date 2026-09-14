#!/usr/bin/env bash
# Live KHook suite runner. NOT a production-release step.
#
# Sends `s2_khook_probe run <SUITE>` through scripts/rcon.py, optionally merges
# JSON from `s2_khook_accept report`, rejects missing/duplicate case records
# on the probe stream *before* JS merge, and exits nonzero on a required failure.
#
#   bash scripts/test-khook-live.sh A
#   bash scripts/test-khook-live.sh --self-test
#   bash scripts/test-khook-live.sh A --from-file records.jsonl
#
# Do not treat compilation or an rg match as a pass. Pending is not pass.
set -euo pipefail
cd "$(dirname "$0")/.."

RCON=(python3 scripts/rcon.py)
HOST="${S2_RCON_HOST:-127.0.0.1}"
PORT="${S2_RCON_PORT:-27015}"

SUITE_A_CASES=(
  new_capsule_registration
  shared_capsule_registration
  peer_actions_both_orders
  one_normal_invocation
  frame_client_command_hooks
  fire_event_no_suppression
  fire_event_handled_recipient_mask
  voice_recall
  sdkhooks_one_of_two_entities
  sdkhooks_phase_removal
  entity_slot_reuse_map_teardown
  check_transmit
)

usage() {
  echo "usage: $0 A|B|C [--from-file FILE] [--port N]" >&2
  echo "       $0 --self-test" >&2
  exit 2
}

# Python helper: parse JSONL records, optional probe-integrity (dups/missing),
# optional JS merge (pending names JS is allowed to prove), then judge.
run_python() {
  local mode="$1"
  local suite="$2"
  local probe="$3"
  local js="${4-}"
  python3 -c '
import json, sys
mode, suite = sys.argv[1], sys.argv[2]
probe_raw = sys.stdin.read()
js_raw = sys.argv[3] if len(sys.argv) > 3 else ""
required = {
    "A": [
        "new_capsule_registration",
        "shared_capsule_registration",
        "peer_actions_both_orders",
        "one_normal_invocation",
        "frame_client_command_hooks",
        "fire_event_no_suppression",
        "fire_event_handled_recipient_mask",
        "voice_recall",
        "sdkhooks_one_of_two_entities",
        "sdkhooks_phase_removal",
        "entity_slot_reuse_map_teardown",
        "check_transmit",
    ],
    "B": ["not_authored"],
    "C": ["not_authored"],
}[suite.upper()]
# JS may fill still-pending names it can prove. Never GameFrame/FireEvent original counts
# and never invent a pass when native Add failed.
JS_MAY_PROVE = {
    "sdkhooks_one_of_two_entities",
    "sdkhooks_phase_removal",
    "entity_slot_reuse_map_teardown",
    "voice_recall",
    "check_transmit",
    "fire_event_handled_recipient_mask",
}

def parse_records(text):
    recs = []
    for line in text.splitlines():
        s = line.strip()
        if not s:
            continue
        start = s.find("{")
        end = s.rfind("}")
        if start < 0 or end <= start:
            continue
        blob = s[start:end + 1]
        try:
            obj = json.loads(blob)
        except json.JSONDecodeError:
            continue
        if not isinstance(obj, dict) or "case" not in obj:
            continue
        recs.append(obj)
    return recs

def fold(recs):
    by = {}
    dups = []
    for obj in recs:
        name = obj["case"]
        if name in by:
            dups.append(name)
            continue
        by[name] = obj
    return by, dups

def native_add_failed(obj):
    act = str(obj.get("actual") or "")
    return "nativeAdd=fail" in act

def merge(probe_recs, js_recs):
    by, dups = fold(probe_recs)
    for obj in js_recs:
        name = obj["case"]
        prev = by.get(name)
        if prev is None:
            by[name] = obj
            continue
        prev_res = str(prev.get("result") or "").lower()
        new_res = str(obj.get("result") or "").lower()
        if prev_res != "pending":
            continue
        if name not in JS_MAY_PROVE:
            continue
        if native_add_failed(prev):
            continue
        if new_res in ("pass", "fail", "pending"):
            by[name] = obj
    return by, dups

def emit_judge(by_case, dups, integrity_only):
    errors = []
    if dups:
        errors.append("duplicate case records: " + ", ".join(sorted(set(dups))))
    missing = [c for c in required if c not in by_case]
    if missing:
        errors.append("missing case records: " + ", ".join(missing))
    if integrity_only:
        for msg in errors:
            print("FAIL:", msg)
        if errors:
            print("FAIL: suite %s probe integrity (dups/missing)" % suite)
            sys.exit(1)
        sys.exit(0)
    failed, pending, passed = [], [], []
    for c in required:
        obj = by_case.get(c)
        if not obj:
            continue
        result = str(obj.get("result") or "").lower()
        if result == "pass" or obj.get("pass") is True:
            passed.append(c)
        elif result == "pending":
            pending.append(c)
        else:
            failed.append(c)
    for msg in errors:
        print("FAIL:", msg)
    for c in failed:
        obj = by_case[c]
        print("FAIL: %s: expected=%r actual=%r" % (c, obj.get("expected"), obj.get("actual")))
    for c in pending:
        obj = by_case[c]
        print("PENDING: %s: %s" % (c, obj.get("actual")))
    for c in passed:
        print("PASS:", c)
    if errors or failed:
        print("FAIL: suite %s required=%d pass=%d pending=%d fail=%d" % (
            suite, len(required), len(passed), len(pending), len(failed)))
        sys.exit(1)
    if pending:
        print("PENDING: suite %s required=%d pass=%d pending=%d — not a green gate" % (
            suite, len(required), len(passed), len(pending)))
        sys.exit(2)
    print("PASS: suite %s (%d/%d)" % (suite, len(passed), len(required)))
    sys.exit(0)

probe_recs = parse_records(probe_raw)
js_recs = parse_records(js_raw)
if mode == "probe-integrity":
    by, dups = fold(probe_recs)
    emit_judge(by, dups, True)
elif mode == "merge-judge":
    by, dups = merge(probe_recs, js_recs)
    # Dups/missing already judged on the probe stream; still reject if the
    # merged set is missing required names (JS cannot invent a missing probe case
    # for names the probe omitted).
    emit_judge(by, [], False)
else:
    by, dups = fold(probe_recs)
    emit_judge(by, dups, False)
' "$mode" "$suite" "$js" <<<"$probe"
}

parse_and_judge() {
  run_python judge "$1" "$2" ""
}

probe_integrity() {
  run_python probe-integrity "$1" "$2" ""
}

merge_and_judge() {
  run_python merge-judge "$1" "$2" "$3"
}

self_test() {
  local tmp
  tmp="$(mktemp)"
  # Happy path: all pass.
  {
    echo 'RCON connected.'
    echo '>>> s2_khook_probe run A'
    for c in "${SUITE_A_CASES[@]}"; do
      echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    done
  } >"$tmp"
  if ! parse_and_judge A "$(cat "$tmp")" >/dev/null; then
    echo "FAIL: self-test all-pass should exit 0"
    exit 1
  fi

  # Duplicate (judge on raw records, no merge).
  {
    echo "{\"suite\":\"A\",\"case\":\"new_capsule_registration\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    echo "{\"suite\":\"A\",\"case\":\"new_capsule_registration\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    for c in "${SUITE_A_CASES[@]:1}"; do
      echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    done
  } >"$tmp"
  if parse_and_judge A "$(cat "$tmp")" >/dev/null; then
    echo "FAIL: self-test duplicate should be nonzero"
    exit 1
  fi

  # Missing.
  {
    echo "{\"suite\":\"A\",\"case\":\"new_capsule_registration\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
  } >"$tmp"
  if parse_and_judge A "$(cat "$tmp")" >/dev/null; then
    echo "FAIL: self-test missing should be nonzero"
    exit 1
  fi

  # Required failure.
  {
    for c in "${SUITE_A_CASES[@]}"; do
      if [ "$c" = "one_normal_invocation" ]; then
        echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"bad\",\"result\":\"fail\"}"
      else
        echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
      fi
    done
  } >"$tmp"
  if parse_and_judge A "$(cat "$tmp")" >/dev/null; then
    echo "FAIL: self-test required fail should be nonzero"
    exit 1
  fi

  # Pending-only remaining is not a green pass (exit 2).
  {
    for c in "${SUITE_A_CASES[@]}"; do
      echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"waiting\",\"result\":\"pending\"}"
    done
  } >"$tmp"
  set +e
  parse_and_judge A "$(cat "$tmp")" >/dev/null
  local st=$?
  set -e
  if [ "$st" -eq 0 ]; then
    echo "FAIL: self-test pending must not exit 0"
    exit 1
  fi
  if [ "$st" -ne 2 ]; then
    echo "FAIL: self-test pending expected exit 2, got $st"
    exit 1
  fi

  # Live-path merge: duplicate probe records fail integrity BEFORE JS fold.
  local probe_dup js_all
  probe_dup="$(mktemp)"
  js_all="$(mktemp)"
  {
    echo "{\"suite\":\"A\",\"case\":\"new_capsule_registration\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    echo "{\"suite\":\"A\",\"case\":\"new_capsule_registration\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    for c in "${SUITE_A_CASES[@]:1}"; do
      echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
    done
  } >"$probe_dup"
  {
    for c in "${SUITE_A_CASES[@]}"; do
      echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"js\",\"result\":\"pass\"}"
    done
  } >"$js_all"
  set +e
  probe_integrity A "$(cat "$probe_dup")" >/dev/null
  st=$?
  set -e
  if [ "$st" -eq 0 ]; then
    echo "FAIL: self-test merge-path duplicate probe must fail integrity"
    exit 1
  fi

  # JS must not upgrade fire_event_no_suppression or frame_client_command_hooks,
  # and must not upgrade when nativeAdd=fail.
  local probe_pend js_pass
  probe_pend="$(mktemp)"
  js_pass="$(mktemp)"
  {
    for c in "${SUITE_A_CASES[@]}"; do
      if [ "$c" = "frame_client_command_hooks" ]; then
        echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"nativeAdd=fail\",\"result\":\"pending\"}"
      elif [ "$c" = "fire_event_no_suppression" ]; then
        echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"nativeAdd=ok pre=1 orig=1\",\"result\":\"pending\"}"
      elif [ "$c" = "sdkhooks_one_of_two_entities" ]; then
        echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"waiting\",\"result\":\"pending\"}"
      else
        echo "{\"suite\":\"A\",\"case\":\"$c\",\"expected\":\"e\",\"actual\":\"a\",\"result\":\"pass\"}"
      fi
    done
  } >"$probe_pend"
  {
    echo "{\"suite\":\"A\",\"case\":\"frame_client_command_hooks\",\"expected\":\"e\",\"actual\":\"js publics\",\"result\":\"pass\"}"
    echo "{\"suite\":\"A\",\"case\":\"fire_event_no_suppression\",\"expected\":\"e\",\"actual\":\"js fire\",\"result\":\"pass\"}"
    echo "{\"suite\":\"A\",\"case\":\"sdkhooks_one_of_two_entities\",\"expected\":\"e\",\"actual\":\"spawnA=1 spawnB=0\",\"result\":\"pass\"}"
  } >"$js_pass"
  if ! probe_integrity A "$(cat "$probe_pend")" >/dev/null; then
    echo "FAIL: self-test probe-integrity should pass on unique pending set"
    exit 1
  fi
  local merged_out
  set +e
  merged_out="$(merge_and_judge A "$(cat "$probe_pend")" "$(cat "$js_pass")" 2>&1)"
  st=$?
  set -e
  if [ "$st" -eq 0 ]; then
    echo "FAIL: self-test JS must not green the suite by upgrading blocked names"
    echo "$merged_out"
    exit 1
  fi
  if echo "$merged_out" | grep -q "PASS: frame_client_command_hooks"; then
    echo "FAIL: self-test JS upgraded frame_client_command_hooks after nativeAdd=fail"
    echo "$merged_out"
    exit 1
  fi
  if echo "$merged_out" | grep -q "PASS: fire_event_no_suppression"; then
    echo "FAIL: self-test JS upgraded fire_event_no_suppression"
    echo "$merged_out"
    exit 1
  fi
  if ! echo "$merged_out" | grep -q "PASS: sdkhooks_one_of_two_entities"; then
    echo "FAIL: self-test JS should upgrade sdkhooks_one_of_two_entities"
    echo "$merged_out"
    exit 1
  fi

  rm -f "$tmp" "$probe_dup" "$js_all" "$probe_pend" "$js_pass"
  echo "PASS: test-khook-live.sh --self-test"
}

FROM_FILE=""
SUITE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test) self_test; exit 0 ;;
    --from-file)
      [ $# -ge 2 ] || usage
      FROM_FILE="$2"
      shift 2
      ;;
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
  parse_and_judge "$SUITE" "$(cat "$FROM_FILE")"
  exit $?
fi

# Live path — existing RCON helper (127.0.0.1:27015, pw s2script).
rcon() {
  "${RCON[@]}" --port "$PORT" "$@"
}

echo "==> suite $SUITE via RCON $HOST:$PORT"
# Best-effort JS prepare; ignore if the acceptance plugin is not loaded.
set +e
rcon "s2_khook_accept prepare" >/dev/null 2>&1
# Drive ping twice: first Ignore (continuation), second Supercede (suppression).
# RCON may only hit DispatchConCommand; ClientCommand still needs a real client.
rcon "khook_probe_ping" >/dev/null 2>&1
rcon "khook_probe_ping" >/dev/null 2>&1
set -e

probe_out="$(rcon "s2_khook_probe run $SUITE" || true)"
js_out=""
set +e
js_out="$(rcon "s2_khook_accept report" 2>/dev/null)"
set -e

echo "$probe_out"
if [ -n "$js_out" ]; then
  echo "-- s2_khook_accept report --"
  echo "$js_out"
fi

# Reject duplicate/missing probe records BEFORE merging JS.
set +e
probe_integrity "$SUITE" "$probe_out"
integrity_st=$?
set -e
if [ "$integrity_st" -ne 0 ]; then
  exit "$integrity_st"
fi

merge_and_judge "$SUITE" "$probe_out" "$js_out"
