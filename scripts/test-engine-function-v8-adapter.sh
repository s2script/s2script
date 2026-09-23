#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ ${1:-} == --spike && ${2:-} == --stock-provider && $# == 2 ]] || { echo 'usage: test-engine-function-v8-adapter.sh --spike --stock-provider' >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || { echo 'UNSUPPORTED platform: V8 stock provider proof requires linux-x86_64-sysv' >&2; exit 2; }
# Diagnostic execution is mandatory until the exact-head V8 crash is located.
# Missing diagnostics must not silently fall back to an unobserved or skipped test.
for tool in gdb timeout; do
  command -v "$tool" >/dev/null || { echo "FAIL required V8 diagnostic tool missing: $tool" >&2; exit 2; }
done
bash scripts/test-engine-function-abi.sh --stock-provider
export S2FN_V8_BRIDGE="$PWD/build/engine-function-abi/libengine_function_v8_bridge.so"
[[ -f "$S2FN_V8_BRIDGE" ]] || { echo 'FAIL missing real provider bridge' >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
# --no-run only compiles. Select its exact executable, never a stale glob result.
cargo test --locked -p s2script-core --lib --no-run --message-format=json > "$tmp/artifacts.jsonl"
executable="$(python3 - "$tmp/artifacts.jsonl" <<'PY'
import json
import pathlib
import sys
executables = set()
for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
    item = json.loads(line)
    if (item.get("reason") == "compiler-artifact"
            and item.get("target", {}).get("name") == "s2script_core"
            and item.get("target", {}).get("kind") == ["lib"]
            and item.get("profile", {}).get("test") is True
            and item.get("executable")):
        executables.add(item["executable"])
if len(executables) != 1:
    raise SystemExit(f"FAIL expected one exact Rust test executable, found {len(executables)}")
executable = pathlib.Path(executables.pop())
if not executable.is_absolute() or not executable.is_file():
    raise SystemExit("FAIL compiled Rust test executable is missing/not absolute")
print(executable)
PY
)"
cat > "$tmp/diagnose.gdb" <<'GDB'
set pagination off
set confirm off
set disable-randomization off
set startup-with-shell off
set print thread-events off
set print elements 32
set print frame-arguments scalars
# A handled signal is not itself a failed test. Snapshot each stop, then deliver
# its signal and continue until the actual inferior exit (or the bounded limit).
handle SIGSEGV stop print pass
handle SIGBUS stop print pass
handle SIGILL stop print pass
handle SIGFPE stop print pass
python
import gdb

def exit_value(name):
    value = gdb.parse_and_eval(name)
    return None if value.type.code == gdb.TYPE_CODE_VOID else int(value)

gdb.execute("run")
for stop in range(32):
    if not gdb.selected_inferior().pid:
        break
    gdb.write("V8 diagnostic stop %d; passing signal after snapshot\n" % (stop + 1))
    for command in ("info program", "thread apply all bt full 40", "info registers",
                    "info proc mappings", "x/24i $pc-32"):
        try:
            gdb.execute(command)
        except gdb.error as error:
            gdb.write("Diagnostic command unavailable: %s: %s\n" % (command, error))
    gdb.execute("continue")
if gdb.selected_inferior().pid:
    gdb.write("FAIL V8 diagnostic exceeded 32 signal stops\n")
    gdb.execute("kill")
    gdb.execute("quit 124")
# Preserve the actual inferior outcome, including fatal signals after delivery.
# Never let GDB's successful command processing relabel a crashed test as green.
signal = exit_value("$_exitsignal")
code = exit_value("$_exitcode")
if signal is not None:
    gdb.write("V8 diagnostic inferior terminated by signal %d\n" % signal)
    gdb.execute("quit %d" % (128 + signal))
elif code is not None:
    gdb.write("V8 diagnostic inferior exit %d\n" % code)
    gdb.execute("quit %d" % code)
else:
    gdb.write("FAIL V8 diagnostic has no inferior exit status\n")
    gdb.execute("quit 2")
end
GDB
# Mirror .cargo/config.toml for this direct executable invocation.
export RUST_TEST_THREADS=1
# One real test execution, including legitimate handled-signal continuations.
# An independent timeout bounds deadlocks/debugger stalls; timeout remains red.
timeout --signal=TERM --kill-after=10s 120s \
  gdb -nx --batch --return-child-result -x "$tmp/diagnose.gdb" --args "$executable" \
  v8host::engine_function_adapter_v8::busy_caller_stock_provider_spike --ignored --exact --nocapture
