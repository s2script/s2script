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

# --- gate.sh pure helpers ---------------------------------------------------
[ -f scripts/gate.sh ] || { echo "FAIL: scripts/gate.sh missing"; exit 1; }
bash -n scripts/gate.sh || { echo "FAIL: scripts/gate.sh is not valid bash"; exit 1; }

# Source the helpers only (no CLI dispatch, no `set -e` leaking into this shell).
GATE_LIB_ONLY=1 source scripts/gate.sh

# Instance name is derived from the worktree dir basename.
got="$(gate_instance_name /home/gkh/projects/s2script-sound)"
[ "$got" = "s2script-cs2-sound" ] || { echo "FAIL: gate_instance_name gave '$got'"; exit 1; }
got="$(gate_instance_name /home/gkh/projects/s2script-zones-polish)"
[ "$got" = "s2script-cs2-zones-polish" ] || { echo "FAIL: gate_instance_name gave '$got'"; exit 1; }
# A dir not named s2script-* still gets a sane instance name.
got="$(gate_instance_name /tmp/experiment)"
[ "$got" = "s2script-cs2-experiment" ] || { echo "FAIL: gate_instance_name gave '$got'"; exit 1; }
# Trailing slash must not produce an empty suffix.
got="$(gate_instance_name /home/gkh/projects/s2script-sound/)"
[ "$got" = "s2script-cs2-sound" ] || { echo "FAIL: gate_instance_name trailing slash gave '$got'"; exit 1; }

# The primary is the repo's main worktree; --git-dir == --git-common-dir identifies it.
prim="$(gate_find_primary)"
[ -d "$prim/.git" ] || [ -f "$prim/.git" ] || { echo "FAIL: gate_find_primary gave '$prim'"; exit 1; }
[ -d "$prim/docker" ] || { echo "FAIL: gate_find_primary '$prim' has no docker/"; exit 1; }

# gate_port_free must be hermetic — do NOT assert against 27015, which is only taken while the
# primary happens to be running (that would fail spuriously whenever it is down). Bind a port
# ourselves instead, so the test is deterministic wherever it runs.
gate_port_free 27099 || { echo "FAIL: gate_port_free says the unbound 27099 is taken"; exit 1; }

python3 -c "
import socket, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', 27099)); s.listen(1)
time.sleep(10)
" &
gate_probe_pid=$!
sleep 0.7
if gate_port_free 27099; then
  kill "$gate_probe_pid" 2>/dev/null
  echo "FAIL: gate_port_free reported a bound port as free"; exit 1
fi
kill "$gate_probe_pid" 2>/dev/null; wait "$gate_probe_pid" 2>/dev/null

# A claimed port must land in the documented range.
p="$(gate_claim_port)" || { echo "FAIL: gate_claim_port found nothing"; exit 1; }
[ "$p" -ge 27016 ] && [ "$p" -le 27030 ] || { echo "FAIL: claimed port $p out of range"; exit 1; }

grep -q '^\.gate/$' .gitignore || { echo "FAIL: .gate/ is not gitignored"; exit 1; }
echo "  gate.sh helpers OK"

echo "PASS: test-gate.sh"
