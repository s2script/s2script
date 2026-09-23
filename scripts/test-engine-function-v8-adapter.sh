#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ ${1:-} == --spike && ${2:-} == --stock-provider && $# == 2 ]] || { echo 'usage: test-engine-function-v8-adapter.sh --spike --stock-provider' >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: V8 stock provider proof requires linux-x86_64-sysv' >&2; exit 2; }
# Diagnostic execution is mandatory until the exact-head V8 crash is located.
# Missing diagnostics must not silently fall back to an unobserved or skipped test.
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
# No previous gate core may be mistaken for this single execution.
shopt -s nullglob
previous_cores=("$cores"/core.*)
[[ ${#previous_cores[@]} == 0 ]] || { echo 'FAIL stale core files before V8 test' >&2; exit 2; }
bash scripts/test-engine-function-abi.sh --stock-provider
export S2FN_V8_BRIDGE="$PWD/build/engine-function-abi/libengine_function_v8_bridge.so"
[[ -f "$S2FN_V8_BRIDGE" ]] || { echo 'FAIL missing real provider bridge' >&2; exit 1; }
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
[[ -f "$member_fixture" ]] || { echo 'FAIL missing member fixture' >&2; exit 2; }
{
  printf '%s\n' 'command=cargo test --locked -p s2script-core --lib v8host::engine_function_adapter_v8::busy_caller_stock_provider_spike -- --ignored --exact --nocapture'
  git rev-parse HEAD
  cargo --version
  gdb --version
  printf 'page_size=%s core_limit_KiB=%s text_cap_bytes=1048576\n' "$(getconf PAGESIZE)" "$(ulimit -c)"
  printf 'core_pattern=%s\nexecutable=%s\n' "$(cat /proc/sys/kernel/core_pattern)" "$executable"
  sha256sum "$executable" "$S2FN_V8_BRIDGE" "$member_fixture"
} > "$artifact/manifest.txt"
for binary in "$executable" "$S2FN_V8_BRIDGE" "$member_fixture"; do
  capture "$artifact/$(basename "$binary").elf.txt" readelf -h -l -n -W "$binary"
done
capture "$artifact/bridge-symbols.txt" nm -anC "$S2FN_V8_BRIDGE"
# Locate the real int target and disassemble its page plus the following page.
# This records layout without changing or relinking the fixture.
page="$(python3 - "$artifact/bridge-symbols.txt" "$(getconf PAGESIZE)" <<'PY'
import pathlib
import sys
symbols = pathlib.Path(sys.argv[1]).read_text().splitlines()
addresses = {int(line.split()[0], 16) for line in symbols if "identity<int>(int)" in line}
if len(addresses) != 1:
    raise SystemExit("FAIL expected one identity<int> ELF target")
size = int(sys.argv[2])
print(addresses.pop() // size * size)
PY
)"
capture "$artifact/target-page.txt" objdump -d -C --start-address="$page" \
  --stop-address="$((page + 2 * $(getconf PAGESIZE)))" "$S2FN_V8_BRIDGE"
cat "$artifact/manifest.txt"
printf '%s\n' 'V8 normal Cargo execution begin (one test; no live debugger)' | tee "$artifact/outcome.txt"
# The preceding --no-run only discovers the matching executable. Cargo launches
# the actual test with its original runtime environment, exactly once.
set +e
timeout --signal=TERM --kill-after=10s 120s \
  cargo test --locked -p s2script-core --lib \
  v8host::engine_function_adapter_v8::busy_caller_stock_provider_spike -- --ignored --exact --nocapture \
  2>&1 | python3 -c '
import os
import sys

# Keep draining after the saved prefix fills or a destination fails, so capture
# cannot give the Cargo child SIGPIPE. Console output still receives the stream.
streams = {sys.stdout.fileno(): None}
saved = None
failed = False
try:
    saved = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o666)
    streams[saved] = 1048576
except OSError as error:
    print(f"FAIL Cargo output capture: {error}", file=sys.stderr)
    failed = True
total = 0
while True:
    chunk = os.read(sys.stdin.fileno(), 65536)
    if not chunk:
        break
    total += len(chunk)
    for descriptor, remaining in list(streams.items()):
        data = chunk if remaining is None else chunk[:remaining]
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
if saved is not None:
    try:
        os.close(saved)
    except OSError as error:
        print(f"FAIL Cargo output capture: {error}", file=sys.stderr)
        failed = True
if total > 1048576:
    print("DIAGNOSTIC LIMITATION: Cargo output exceeds 1 MiB; saved prefix only", file=sys.stderr)
sys.exit(1 if failed else 0)
' "$artifact/test-output.txt"
pipeline_status=("${PIPESTATUS[@]}")
status=${pipeline_status[0]}
capture_status=${pipeline_status[1]}
printf 'V8 normal Cargo execution exit %s\n' "$status" | tee -a "$artifact/outcome.txt"
printf 'V8 Cargo output capture exit %s\n' "$capture_status" | tee -a "$artifact/outcome.txt"
core_files=("$cores"/core.*)
if [[ ${#core_files[@]} != 0 ]]; then
  mkdir -p "$artifact/binaries"
  cp "$executable" "$S2FN_V8_BRIDGE" "$member_fixture" "$artifact/binaries/"
  printf 'matching binary copy exit %s\n' "$?" >> "$artifact/outcome.txt"
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
  printf 'postmortem exit %s (original Cargo status retained)\n' "$?" >> "$artifact/outcome.txt"
  cat "$artifact/postmortem.txt"
elif [[ ${#core_files[@]} != 0 || $status != 0 ]]; then
  printf 'DIAGNOSTIC LIMITATION: expected one core after failure, found %s; Cargo status retained\n' "${#core_files[@]}" | tee -a "$artifact/outcome.txt"
else
  printf '%s\n' 'No core: normal test passed; original SIGSEGV remains unexplained.' | tee -a "$artifact/outcome.txt"
fi
cat "$artifact/outcome.txt"
# A lost mandatory capture fails a successful test; Cargo failure/timeout wins.
if [[ $status == 0 && $capture_status != 0 ]]; then
  exit "$capture_status"
fi
exit "$status"
