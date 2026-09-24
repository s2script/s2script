#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
test_filter=v8host::engine_function_adapter_v8::production::production_registry_outer_frame
if [[ ${1:-} == --spike && ${2:-} == --stock-provider && $# == 2 ]]; then
  test_filter=v8host::engine_function_adapter_v8::busy_caller_stock_provider_spike
elif [[ ${1:-} != --stock-provider || $# != 1 ]]; then
  echo 'usage: test-engine-function-v8-adapter.sh [--spike] --stock-provider' >&2; exit 2
fi
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: V8 stock provider proof requires linux-x86_64-sysv' >&2; exit 2; }
mode=${S2FN_V8_DIAGNOSTICS:-0}
[[ $mode == 0 || $mode == 1 ]] || { echo 'FAIL S2FN_V8_DIAGNOSTICS must be 0 or 1' >&2; exit 2; }
original_test=(cargo test --locked -p s2script-core --lib
  "$test_filter" -- --ignored --exact --nocapture)
prepare_bridge() {
  bash scripts/test-engine-function-abi.sh --stock-provider
  export S2FN_V8_BRIDGE="$PWD/build/engine-function-abi/libengine_function_v8_bridge.so"
  [[ -f "$S2FN_V8_BRIDGE" ]] || { echo 'FAIL missing real provider bridge' >&2; exit 1; }
}
# Ordinary local/server gates require no diagnostic tools or kernel policy.
if [[ $mode == 0 ]]; then
  prepare_bridge
  "${original_test[@]}"
  exit $?
fi
runs=${S2FN_V8_DIAGNOSTIC_RUNS:-32}
[[ $runs =~ ^([1-9]|[12][0-9]|3[0-2])$ ]] || {
  echo 'FAIL S2FN_V8_DIAGNOSTIC_RUNS must be an integer from 1 to 32' >&2; exit 2;
}
# Opt-in diagnostics require complete capture setup; never silently fall back.
for tool in gdb timeout readelf nm objdump sha256sum python3; do
  command -v "$tool" >/dev/null || { echo "FAIL required V8 diagnostic tool missing: $tool" >&2; exit 2; }
done
diagnostics="$PWD/build/engine-function-v8-diagnostics"
artifact="$diagnostics/artifact"
cores="$diagnostics/cores"
mkdir -p "$artifact" "$cores"
# The workflow configures this only on its disposable runner. Never mutate a
# developer or server kernel setting from this script.
[[ $(cat /proc/sys/kernel/core_pattern) == "$cores/core.%p.%t" ]] || {
  echo 'FAIL required file-based V8 core capture is not configured' >&2; exit 2;
}
ulimit -c 4194304 || { echo 'FAIL cannot enable bounded 4 GiB core capture' >&2; exit 2; }
# No previous gate core may be mistaken for evidence from this experiment.
shopt -s nullglob
previous_cores=("$cores"/core.*)
[[ ${#previous_cores[@]} == 0 ]] || { echo 'FAIL stale core files before V8 test' >&2; exit 2; }
# Refuse a second experiment in this directory, preserving the first evidence.
mkdir "$artifact/attempts" || { echo 'FAIL existing/unwritable V8 experiment artifacts' >&2; exit 2; }
prepare_bridge
# --no-run only compiles. Select its exact executable, never a stale glob result.
cargo test --locked -p s2script-core --lib --no-run --message-format=json > "$artifact/artifacts.jsonl"
executable="$(python3 - "$artifact/artifacts.jsonl" <<'PY'
import json
import pathlib
import sys
artifacts = []
executables = set()
for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
    item = json.loads(line)
    if item.get("reason") != "compiler-artifact":
        continue
    artifacts.append(item)
    # Cargo preserves the cdylib target kind even for its executable
    # unit-test harness; profile.test/executable distinguish that harness.
    if (item.get("target", {}).get("name") == "s2script_core"
            and item.get("target", {}).get("kind") == ["cdylib"]
            and item.get("profile", {}).get("test") is True
            and item.get("executable")):
        executables.add(item["executable"])

def fail(message):
    print("Cargo compiler-artifact discovery evidence:", file=sys.stderr)
    for item in artifacts:
        print(json.dumps({"package_id": item.get("package_id"),
                          "target": item.get("target"),
                          "profile_test": item.get("profile", {}).get("test"),
                          "executable": item.get("executable")}, sort_keys=True), file=sys.stderr)
    raise SystemExit(message)

if len(executables) != 1:
    fail(f"FAIL expected one exact Rust test executable, found {len(executables)}")
executable = pathlib.Path(executables.pop())
if not executable.is_absolute() or not executable.is_file():
    fail("FAIL compiled Rust test executable is missing/not absolute")
print(executable)
PY
)"
# Each diagnostic text capture is capped at 1 MiB. If its producer receives
# SIGPIPE, the failed capture remains visible, never a successful native proof.
capture() {
  local output=$1
  shift
  "$@" 2>&1 | head -c 1048576 > "$output"
}
member_fixture="$PWD/build/engine-function-abi/libengine_function_member_fixture.so"
[[ -f "$member_fixture" ]] || { echo 'FAIL missing native target fixture' >&2; exit 2; }
binaries=("$executable" "$S2FN_V8_BRIDGE" "$member_fixture")
sha256sum "${binaries[@]}" > "$artifact/fixed.sha256"
{
  printf 'command=cargo test --locked -p s2script-core --lib %s -- --ignored --exact --nocapture\n' "$test_filter"
  git rev-parse HEAD
  cargo --version
  gdb --version
  printf 'page_size=%s core_limit_KiB=%s text_cap_bytes=1048576\n' "$(getconf PAGESIZE)" "$(ulimit -c)"
  printf 'core_pattern=%s\nexecutable=%s\n' "$(cat /proc/sys/kernel/core_pattern)" "$executable"
  printf 'requested_processes=%s attempt_limit_seconds=120 kill_grace_seconds=10 experiment_budget_seconds=600\n' "$runs"
  cat "$artifact/fixed.sha256"
} > "$artifact/manifest.txt"
for binary in "$executable" "$S2FN_V8_BRIDGE" "$member_fixture"; do
  capture "$artifact/$(basename "$binary").elf.txt" readelf -h -l -n -W "$binary"
done
capture "$artifact/bridge-symbols.txt" nm -anC "$S2FN_V8_BRIDGE"
capture "$artifact/fixture-symbols.txt" nm -anC "$member_fixture"
capture "$artifact/fixture-elf-boundary.txt" python3 scripts/check-engine-function-fixture.py "$member_fixture"
# Locate the real int target and disassemble its page plus the following page.
# Its hidden body now belongs to the target fixture, not the adapter bridge.
# This records layout without changing or relinking any of the three binaries.
page="$(python3 - "$artifact/fixture-symbols.txt" "$(getconf PAGESIZE)" "$artifact/target-identity.txt" "$member_fixture" <<'PY'
import pathlib
import sys
symbols = pathlib.Path(sys.argv[1]).read_text().splitlines()
addresses = {int(line.split()[0], 16) for line in symbols
             if "identity<int>(int)" in line and line.split()[1] in ("t", "T")}
if len(addresses) != 1:
    raise SystemExit("FAIL expected one identity<int> ELF target")
size = int(sys.argv[2])
address = addresses.pop()
page = address // size * size
pathlib.Path(sys.argv[3]).write_text(
    f"target_owner={sys.argv[4]}\ntarget_symbol=identity<int>(int)\n"
    f"target_rva={address:#x}\ntarget_page_rva={page:#x}\n")
print(page)
PY
)"
capture "$artifact/target-page.txt" objdump -d -C --start-address="$page" \
  --stop-address="$((page + 2 * $(getconf PAGESIZE)))" "$member_fixture"
cat "$artifact/target-identity.txt"
cat "$artifact/manifest.txt"
# Keep the reviewed bounded, fully draining consumer for every attempt. Also
# retain emitted runtime addresses even when they occur beyond the saved prefix.
capture_cargo() {
  python3 -c '
import os
import re
import sys

# Keep draining after the saved prefix fills or a destination fails, so capture
# cannot give the Cargo child SIGPIPE. Console output still receives the stream.
streams = {sys.stdout.fileno(): None}
saved = None
runtime = None
failed = False
try:
    saved = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o666)
    streams[saved] = 1048576
except OSError as error:
    print(f"FAIL Cargo output capture: {error}", file=sys.stderr)
    failed = True
try:
    runtime = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o666)
    streams[runtime] = 1048576
except OSError as error:
    print(f"FAIL Cargo output capture: {error}", file=sys.stderr)
    failed = True
total = 0
pending = b""
while True:
    chunk = os.read(sys.stdin.fileno(), 65536)
    if not chunk:
        break
    total += len(chunk)
    combined = pending + chunk
    addresses = b"".join(match.group() for match in re.finditer(
        rb"(?:event=create-begin target=0x[0-9a-fA-F]+|fixture-boundary=accepted [^\r\n]+)\r?\n", combined)
        if match.end() > len(pending))
    pending = combined[-65536:] # Retain bounded module-identity lines across reads.
    for descriptor, remaining in list(streams.items()):
        data = addresses if descriptor == runtime else chunk
        if remaining is not None:
            if descriptor == runtime and len(data) > remaining:
                print("FAIL runtime address capture exceeds 1 MiB", file=sys.stderr)
                failed = True
            data = data[:remaining]
        if remaining is not None:
            streams[descriptor] -= len(data)
        try:
            while data:
                written = os.write(descriptor, data)
                if written == 0:
                    raise OSError("write returned zero bytes")
                data = data[written:]
        except OSError as error:
            print(f"FAIL Cargo output capture: {error}", file=sys.stderr)
            failed = True
            del streams[descriptor]
for descriptor in (saved, runtime):
    if descriptor is not None:
        try:
            os.close(descriptor)
        except OSError as error:
            print(f"FAIL Cargo output capture: {error}", file=sys.stderr)
            failed = True
if total > 1048576:
    print("DIAGNOSTIC LIMITATION: Cargo output exceeds 1 MiB; saved prefix only", file=sys.stderr)
sys.exit(1 if failed else 0)
' "$1" "$2"
}
now_ms() { python3 -c 'import time; print(time.monotonic_ns() // 1000000)'; }
# Compilation and static ELF capture precede this fixed-build process experiment.
# Postmortem, if needed, has its own existing 60s + 5s bounds below.
started=$(now_ms)
deadline=$((started + 600000))
successes=0
status=0
printf 'requested=%s successes=0 budget_ms=600000\n' "$runs" > "$artifact/outcome.txt"
for ((index=1; index<=runs; index++)); do
  now=$(now_ms)
  if ((deadline - now < 130000)); then
    printf 'FAIL incomplete experiment: successes=%s requested=%s; insufficient full attempt budget\n' "$successes" "$runs" | tee -a "$artifact/outcome.txt"
    exit 124
  fi
  attempt="$artifact/attempts/$(printf 'attempt-%02d' "$index")"
  mkdir "$attempt"
  printf 'attempt=%s elapsed_ms=%s\n' "$index" "$((now - started))" > "$attempt/outcome.txt"
  if ! sha256sum "${binaries[@]}" > "$attempt/hashes-before.sha256" ||
     ! cmp -s "$artifact/fixed.sha256" "$attempt/hashes-before.sha256"; then
    printf 'FAIL hash capture/mismatch before attempt=%s; experiment invalid\n' "$index" | tee -a "$artifact/outcome.txt" "$attempt/outcome.txt"
    exit 2
  fi
  # Hashing is outside the child timeout, so recheck immediately before launch.
  now=$(now_ms)
  if ((deadline - now < 130000)); then
    printf 'FAIL incomplete experiment: successes=%s requested=%s; insufficient full attempt budget\n' "$successes" "$runs" | tee -a "$artifact/outcome.txt" "$attempt/outcome.txt"
    exit 124
  fi
  attempt_started=$now
  printf 'V8 normal Cargo execution begin attempt=%s/%s (no live debugger)\n' "$index" "$runs" | tee -a "$attempt/outcome.txt"
  set +e
  timeout --signal=TERM --kill-after=10s 120s "${original_test[@]}" 2>&1 | capture_cargo "$attempt/test-output.txt" "$attempt/runtime-addresses.txt"
  pipeline_status=("${PIPESTATUS[@]}")
  cargo_status=${pipeline_status[0]}
  capture_status=${pipeline_status[1]}
  diagnostic_status=0
  # Check every attempted process even when Cargo failed. Never replace its status.
  if ! sha256sum "${binaries[@]}" > "$attempt/hashes-after.sha256" ||
     ! cmp -s "$artifact/fixed.sha256" "$attempt/hashes-after.sha256"; then
    printf 'FAIL hash capture/mismatch after attempt=%s; experiment invalid\n' "$index" | tee -a "$artifact/outcome.txt" "$attempt/outcome.txt"
    diagnostic_status=2
  fi
  now=$(now_ms) || diagnostic_status=2
  if ((now >= deadline)); then
    printf 'FAIL incomplete experiment: overall deadline reached at attempt=%s\n' "$index" | tee -a "$artifact/outcome.txt" "$attempt/outcome.txt"
    diagnostic_status=124
  fi
  core_files=("$cores"/core.*)
  if [[ ${#core_files[@]} != 0 && $cargo_status == 0 ]]; then
    printf 'FAIL unexpected core after successful Cargo attempt=%s\n' "$index" | tee -a "$artifact/outcome.txt" "$attempt/outcome.txt"
    diagnostic_status=2
  fi
  # Each output, address list and hash snapshot belongs only to this attempt.
  printf 'attempt=%s elapsed_ms=%s duration_ms=%s cargo_status=%s capture_status=%s diagnostic_status=%s\n' \
    "$index" "$((now - started))" "$((now - attempt_started))" "$cargo_status" "$capture_status" "$diagnostic_status" | tee -a "$attempt/outcome.txt" "$artifact/outcome.txt"
  record_status=$?
  status=$cargo_status
  [[ $status != 0 ]] || status=$capture_status
  [[ $status != 0 ]] || status=$diagnostic_status
  [[ $status != 0 ]] || status=$record_status
  if [[ $status != 0 ]]; then
    printf 'Stopped at first failure: attempt=%s successes=%s status=%s\n' "$index" "$successes" "$status" | tee -a "$artifact/outcome.txt"
    break
  fi
  set -e
  successes=$((successes + 1))
done
if [[ $status == 0 ]]; then
  printf 'Fixed-build experiment completed: successes=%s requested=%s; original SIGSEGV remains unexplained.\n' "$successes" "$runs" | tee -a "$artifact/outcome.txt"
  cat "$artifact/fixed.sha256"
  exit 0
fi
# Keep all first-failure evidence under that attempt, and never execute again.
# Diagnostic cleanup cannot mask a nonzero Cargo/timeout/capture result.
artifact=$attempt
core_files=("$cores"/core.*)
if [[ ${#core_files[@]} != 0 ]]; then
  mkdir -p "$artifact/binaries"
  cp "$executable" "$S2FN_V8_BRIDGE" "$member_fixture" "$artifact/binaries/"
  printf 'current binary copy exit %s (compare with fixed and attempt hashes)\n' "$?" >> "$artifact/outcome.txt"
fi
if [[ ${#core_files[@]} == 1 ]]; then
  core=${core_files[0]}
  # Raw cores stay outside the upload directory. The size cap can truncate a
  # dump: retain readelf/GDB warnings and status alongside the original outcome.
  ls -l "$core" >> "$artifact/outcome.txt"
  core_bytes=$(python3 -c 'import os,sys; print(os.stat(sys.argv[1]).st_size)' "$core")
  if [[ $core_bytes -ge 4294967296 ]]; then
    echo 'DIAGNOSTIC LIMITATION: core reached 4 GiB cap; dump may be truncated' >> "$artifact/outcome.txt"
  fi
  capture "$artifact/core-headers.txt" timeout --signal=TERM --kill-after=5s 60s readelf -l -W "$core"
  printf 'core headers exit %s (nonzero/size cap may mean incomplete core)\n' "$?" >> "$artifact/outcome.txt"
  cat > "$diagnostics/postmortem.gdb" <<'GDB'
set pagination off
set confirm off
set auto-load off
set print elements 32
set print frame-arguments scalars
python
import gdb
for command in ("p $_siginfo", "info registers", "thread apply all bt full 40",
                "info proc mappings", "x/24i $pc-32"):
    try:
        gdb.execute(command)
    except gdb.error as error:
        gdb.write("Diagnostic command unavailable: %s: %s\n" % (command, error))
end
GDB
  capture "$artifact/postmortem.txt" timeout --signal=TERM --kill-after=5s 60s \
    gdb -nx --batch -iex 'set auto-load off' -x "$diagnostics/postmortem.gdb" "$executable" -c "$core"
  printf 'postmortem exit %s (first failure status retained)\n' "$?" >> "$artifact/outcome.txt"
  cat "$artifact/postmortem.txt"
else
  printf 'DIAGNOSTIC LIMITATION: expected one core after failure, found %s; first failure status retained\n' "${#core_files[@]}" | tee -a "$artifact/outcome.txt"
fi
cat "$artifact/outcome.txt"
exit "$status"
