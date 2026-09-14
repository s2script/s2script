#!/usr/bin/env bash
# Live KHook suite runner. NOT a production-release step.
#
# Sends `s2_khook_probe run <SUITE>` through scripts/rcon.py, optionally merges
# JSON from `s2_khook_accept report`, rejects missing/duplicate case records,
# and exits nonzero on a required failure.
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

parse_and_judge() {
  local suite="$1"
  local raw="$2"
  python3 -c '
import json, sys
suite = sys.argv[1]
raw = sys.stdin.read()
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

recs = parse_records(raw)
by_case = {}
dups = []
for obj in recs:
    name = obj["case"]
    if name in by_case:
        dups.append(name)
        continue
    by_case[name] = obj

errors = []
if dups:
    errors.append("duplicate case records: " + ", ".join(sorted(set(dups))))

missing = [c for c in required if c not in by_case]
if missing:
    errors.append("missing case records: " + ", ".join(missing))

failed = []
pending = []
passed = []
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
' "$suite" <<<"$raw"
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

  # Duplicate.
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

  rm -f "$tmp"
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
set -e

probe_out="$(rcon "s2_khook_probe run $SUITE" || true)"
js_out=""
set +e
js_out="$(rcon "s2_khook_accept report" 2>/dev/null)"
set -e

# Probe records win; JS report fills only still-missing names, and may upgrade
# pending → pass/fail for the same case (exact engine outcomes from public APIs).
merged="$(printf '%s\n' "$probe_out" | python3 -c '
import json, sys
probe = sys.stdin.read()
js = sys.argv[1]

def recs(text):
    out = []
    for line in text.splitlines():
        s = line.strip()
        start, end = s.find("{"), s.rfind("}")
        if start < 0 or end <= start:
            continue
        try:
            obj = json.loads(s[start:end+1])
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict) and "case" in obj:
            out.append(obj)
    return out

by = {}
for obj in recs(probe):
    by[obj["case"]] = obj
for obj in recs(js):
    name = obj["case"]
    prev = by.get(name)
    if prev is None:
        by[name] = obj
        continue
    prev_res = str(prev.get("result") or "").lower()
    new_res = str(obj.get("result") or "").lower()
    if prev_res == "pending" and new_res in ("pass", "fail", "pending"):
        by[name] = obj
for obj in by.values():
    print(json.dumps(obj, separators=(",", ":")))
' "$js_out")"

echo "$probe_out"
if [ -n "$js_out" ]; then
  echo "-- s2_khook_accept report --"
  echo "$js_out"
fi
parse_and_judge "$SUITE" "$merged"
