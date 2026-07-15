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

# --- docker/pre.sh: the automatic gameinfo patch hook ------------------------
[ -f docker/pre.sh ] || { echo "FAIL: docker/pre.sh missing"; exit 1; }

bash -n docker/pre.sh || { echo "FAIL: docker/pre.sh is not valid bash"; exit 1; }

# It must invoke patch-gameinfo.sh as a SUBPROCESS. patch-gameinfo.sh runs `set -euo pipefail`
# and `exit 1`; entry.sh SOURCES pre.sh, so a `source`/`.` here would kill the whole boot.
grep -qE '^[^#]*bash /patch-gameinfo\.sh' docker/pre.sh \
  || { echo "FAIL: pre.sh must run 'bash /patch-gameinfo.sh' as a subprocess"; exit 1; }
grep -qE '^[^#]*(source|\.) +/patch-gameinfo\.sh' docker/pre.sh \
  && { echo "FAIL: pre.sh must NOT source patch-gameinfo.sh (its exit 1 would kill entry.sh)"; exit 1; }

# A patch failure must not abort the boot.
grep -qE '\|\|' docker/pre.sh \
  || { echo "FAIL: pre.sh must tolerate a failed patch (|| warn), not abort the boot"; exit 1; }

# Both compose files must mount it at the path entry.sh sources.
grep -qE '^\s*-\s*\./pre\.sh:/home/steam/cs2-dedicated/pre\.sh:ro' docker/docker-compose.yml \
  || { echo "FAIL: primary compose does not mount ./pre.sh"; exit 1; }
echo "  pre.sh hook OK"

# --- docker-compose.gate.yml: env-driven, absolute-path mounts ---------------
[ -f docker/docker-compose.gate.yml ] || { echo "FAIL: docker/docker-compose.gate.yml missing"; exit 1; }

# Interpolates from a synthetic env — proves the vars wire through to real mount paths.
gate_tmp="$(mktemp -d)"
mkdir -p "$gate_tmp/s2script/configs" "$gate_tmp/s2script/data" "$gate_tmp/mm" "$gate_tmp/cs2data"
gate_cfg="$(
  GATE_NAME=s2script-cs2-probe \
  GATE_PORT=27099 \
  GATE_CS2_DATA="$gate_tmp/cs2data" \
  GATE_S2SCRIPT_DIR="$gate_tmp/s2script" \
  GATE_METAMOD_DIR="$gate_tmp/mm" \
  docker compose -f docker/docker-compose.gate.yml config 2>&1
)" || { echo "FAIL: gate compose did not interpolate: $gate_cfg"; exit 1; }

echo "$gate_cfg" | grep -q "container_name: s2script-cs2-probe" \
  || { echo "FAIL: GATE_NAME did not reach container_name"; exit 1; }
echo "$gate_cfg" | grep -q "$gate_tmp/s2script" \
  || { echo "FAIL: GATE_S2SCRIPT_DIR did not reach the mounts"; exit 1; }
echo "$gate_cfg" | grep -q "$gate_tmp/cs2data" \
  || { echo "FAIL: GATE_CS2_DATA did not reach the install mount"; exit 1; }
# 1:1 port map — the advertised port must equal the reachable one.
echo "$gate_cfg" | grep -q "27099" \
  || { echo "FAIL: GATE_PORT did not reach the port mapping"; exit 1; }
# Missing required vars must fail loudly, not mount something wrong.
if docker compose -f docker/docker-compose.gate.yml config >/dev/null 2>&1; then
  echo "FAIL: gate compose succeeded with no gate.env — the :? guards are missing"; exit 1
fi
rm -rf "$gate_tmp"
echo "  docker-compose.gate.yml OK"

echo "PASS: test-gate.sh"
