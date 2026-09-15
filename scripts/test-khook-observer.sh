#!/usr/bin/env bash
# Compile + run the engine-free FireEvent consuming-call observer harness.
#
# ASan is not decoration: F2 is a use-after-free in Hook_FireEventPost after
# FireEvent consumes the event. Missing sanitizer runtime is a failed gate, not
# a pass. The legacy_post_uaf subprocess must abort; the fixed observer must not.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
out="$WORKDIR/khook_acceptance_observer_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra)

SAN=0
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  SAN=1
  echo "   (with -fsanitize=address,undefined)"
else
  echo "error: sanitizer runtime missing; observer tests require ASan/UBSan" >&2
  exit 1
fi

g++ "${flags[@]}" \
    -I tools/khook-probe \
    -o "$out" shim/tests/khook_acceptance_observer_test.cpp

export ASAN_OPTIONS="abort_on_error=1:detect_leaks=0:halt_on_error=1"
export UBSAN_OPTIONS="halt_on_error=1:print_stacktrace=1"

legacy_log="$WORKDIR/legacy_post_uaf.log"
set +e
timeout --signal=KILL 12s "$out" legacy_post_uaf >"$legacy_log" 2>&1
legacy_st=$?
set -e
echo "----- legacy_post_uaf (exit $legacy_st) -----"
cat "$legacy_log"
if [[ "$legacy_st" -eq 0 ]]; then
  echo "error: legacy POST dereference after delete did not fail; ASan is not catching F2" >&2
  exit 1
fi
if ! grep -Eqi "heap-use-after-free|use-after-free|AddressSanitizer" "$legacy_log"; then
  echo "error: legacy_post_uaf died without an ASan use-after-free diagnostic" >&2
  exit 1
fi
echo "ok:   legacy POST UAF caught by ASan"

echo "==> fixed observer"
"$out"
echo "==> observer host tests: PASS (sanitizers=$SAN)"

# Schema-compatible fixtures (including negative controls) judged by the frozen R4 judge.
# source_revision must match git HEAD because --from-file uses current_source_revision().
REV="$(git rev-parse HEAD)"
FIX_DIR="$WORKDIR/fixtures"
mkdir -p "$FIX_DIR"
python3 tools/khook-probe/testdata/gen_from_file.py "$REV" "$FIX_DIR"

judge_file() {
  local file="$1"
  local want="$2"
  echo "==> judge --from-file $(basename "$file") (want exit $want)"
  set +e
  python3 scripts/khook_acceptance.py A --from-file "$file"
  st=$?
  set -e
  if [[ "$st" -ne "$want" ]]; then
    echo "error: $(basename "$file") expected exit $want, got $st" >&2
    exit 1
  fi
  echo "ok:   $(basename "$file") exit $st"
}

judge_file "$FIX_DIR/r6-pending.jsonl" 2
judge_file "$FIX_DIR/negative-missing-js.jsonl" 1
judge_file "$FIX_DIR/negative-flipped.jsonl" 1
judge_file "$FIX_DIR/negative-omitted-plugin.jsonl" 1
judge_file "$FIX_DIR/partial-continue.jsonl" 2

echo "PASS: test-khook-observer.sh"
