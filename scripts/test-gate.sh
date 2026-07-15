#!/usr/bin/env bash
# Unit tests for the multi-instance live-gate tooling (scripts/gate.sh + scripts/rcon.py --port).
# Pure-function tests only — no Docker, no network, safe to run anywhere.
set -uo pipefail
cd "$(dirname "$0")/.."

# --- rcon.py --port parsing -------------------------------------------------
python3 - <<'PYEOF' || exit 1
import importlib.util
spec = importlib.util.spec_from_file_location("rcon", "scripts/rcon.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)          # module name is "rcon", so main() does not run

# Default port, command untouched (every existing call site).
assert m.parse_args(["sm_say hi"]) == (27015, ["sm_say hi"]), m.parse_args(["sm_say hi"])
# --port <n> consumes its value and is not treated as a command.
assert m.parse_args(["--port", "27017", "meta list"]) == (27017, ["meta list"])
# --port=<n> form.
assert m.parse_args(["--port=27018", "a", "b"]) == (27018, ["a", "b"])
# Multiple commands preserved in order.
assert m.parse_args(["one", "two"]) == (27015, ["one", "two"])
# --port anywhere in argv.
assert m.parse_args(["cmd", "--port", "27020"]) == (27020, ["cmd"])
print("  rcon parse_args OK")
PYEOF

echo "PASS: test-gate.sh"
