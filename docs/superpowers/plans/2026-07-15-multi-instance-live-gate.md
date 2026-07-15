# Multi-instance live gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Any git worktree can run `scripts/gate.sh up` and get its own CS2 dedicated server on its own port, sharing the single 74 G install, with the primary folder's server untouched.

**Architecture:** Each instance gets a btrfs **reflink clone** of `docker/cs2-data` (measured: 0.496s, zero real disk) so it has a full independent writable install — no shared state, so the stock `joedwards32/cs2` image runs unmodified. A new `docker/docker-compose.gate.yml` is today's compose with env-driven paths, fed by an absolute-path `.gate/gate.env` that `scripts/gate.sh` writes. Separately, `docker/pre.sh` hooks the image's own boot sequence to re-patch `gameinfo.gi` automatically.

**Tech Stack:** bash, Docker Compose, btrfs reflinks (`cp --reflink=always`), python3 (rcon client).

**Spec:** `docs/superpowers/specs/2026-07-15-multi-instance-live-gate-design.md`

## Global Constraints

- **Scope is `docker/` + `scripts/` only.** No `core/`, `shim/`, `S2EngineOps` op, `packages/*`, or plugin change. Therefore: **no sniper rebuild, no changeset**, and the boundary gates are unaffected.
- **Port range is 27016–27030 inclusive.** 27015 is the primary; Steam holds 27036 and 27060.
- **Every path written to `.gate/gate.env` is absolute.** Compose interpolation must never depend on cwd or the compose file's location.
- **Shell style matches `scripts/`:** `#!/usr/bin/env bash`, `set -euo pipefail` (or `set -uo pipefail` where a test script checks failures deliberately), `cd "$(dirname "$0")/.."` to reach the repo root, `echo "FAIL: ..."; exit 1` on assertion failure.
- **`docker/cs2-data` and `docker/metamod` are gitignored** — they exist *only* in the primary folder. A linked worktree has nothing to clone from and must locate the primary via `git rev-parse --git-common-dir`.
- **Never restart or recreate the primary's container as part of a task.** A live gate may be running. Task 5 is the only task that touches Docker, and it only creates *instances*.

---

### Task 1: `rcon.py --port`

`scripts/rcon.py` hardcodes `PORT = 27015`, so it can only ever talk to the primary. Add a `--port` flag while keeping every existing call site (`python3 scripts/rcon.py "sm_say hi"`) working unchanged.

**Files:**
- Modify: `scripts/rcon.py:5`, `scripts/rcon.py:26-28`
- Create: `scripts/test-gate.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: `parse_args(argv: list[str]) -> tuple[int, list[str]]` in `scripts/rcon.py`, returning `(port, cmds)`. `scripts/test-gate.sh` — the test file Task 4 extends.

- [ ] **Step 1: Write the failing test**

Create `scripts/test-gate.sh`:

```bash
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-gate.sh`
Expected: FAIL — `AttributeError: module 'rcon' has no attribute 'parse_args'`

- [ ] **Step 3: Write minimal implementation**

In `scripts/rcon.py`, replace line 5:

```python
HOST, PORT, PW = "127.0.0.1", 27015, "s2script"
```

with:

```python
HOST, PORT, PW = "127.0.0.1", 27015, "s2script"


def parse_args(argv):
    """Split argv into (port, cmds). `--port N` / `--port=N` may appear anywhere;
    every other arg is an RCON command. Defaults to PORT so existing call sites
    (`rcon.py "sm_say hi"`) are unchanged."""
    port, cmds, i = PORT, [], 0
    while i < len(argv):
        a = argv[i]
        if a == "--port":
            if i + 1 >= len(argv):
                raise SystemExit("rcon.py: --port requires a value")
            port = int(argv[i + 1])
            i += 2
        elif a.startswith("--port="):
            port = int(a.split("=", 1)[1])
            i += 1
        else:
            cmds.append(a)
            i += 1
    return port, cmds
```

Then replace lines 26-28 (`def main():` through the `create_connection` line):

```python
def main():
    cmds = sys.argv[1:]
    s = socket.create_connection((HOST, PORT), timeout=10)
```

with:

```python
def main():
    port, cmds = parse_args(sys.argv[1:])
    s = socket.create_connection((HOST, port), timeout=10)
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-gate.sh`
Expected: PASS — prints `  rcon parse_args OK` then `PASS: test-gate.sh`

- [ ] **Step 5: Verify the live primary still answers on the default port**

Run: `python3 scripts/rcon.py "meta version"`
Expected: `RCON connected.` followed by Metamod's version block — proves the default-port path is unbroken against the running server.

- [ ] **Step 6: Commit**

```bash
git add scripts/rcon.py scripts/test-gate.sh
git commit -m "feat(gate): rcon.py --port so a gate instance can be addressed"
```

---

### Task 2: Automatic `gameinfo.gi` patching via `pre.sh`

The image's `entry.sh` **sources `pre.sh` at line 153 — after its `steamcmd +app_update` (line 45) and before srcds launches (line 203)**. That is exactly where a re-patch belongs: a CS2 update rewrites `gameinfo.gi` and drops the Metamod SearchPath, which today requires a manual `docker exec s2script-cs2 /patch-gameinfo.sh`.

`entry.sh` has **no `set -e`**, and `entry.sh:64-69` only copies its own `pre.sh` if one is absent — so a mounted `pre.sh` wins and a non-zero command inside it is non-fatal.

**Files:**
- Create: `docker/pre.sh`
- Modify: `docker/docker-compose.yml` (the primary's — add one mount line)
- Test: `scripts/test-gate.sh` (append a section)

**Interfaces:**
- Consumes: nothing.
- Produces: `docker/pre.sh` — mounted at `/home/steam/cs2-dedicated/pre.sh:ro` by both compose files. Task 3's `docker-compose.gate.yml` mounts the same file.

- [ ] **Step 1: Write the failing test**

Append to `scripts/test-gate.sh`, immediately before the final `echo "PASS: test-gate.sh"` line:

```bash
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-gate.sh`
Expected: FAIL — `FAIL: docker/pre.sh missing`

- [ ] **Step 3: Create `docker/pre.sh`**

```bash
#!/bin/bash
# PRE HOOK — the image's entry.sh SOURCES this file (entry.sh:153), after its
# `steamcmd +app_update` (entry.sh:45) and before srcds launches (entry.sh:203).
#
# Why this exists: a CS2 update rewrites game/csgo/gameinfo.gi and drops the Metamod
# SearchPath, so Metamod (and therefore s2script) silently stops loading. Re-patching by
# hand — `docker exec <container> /patch-gameinfo.sh` — was the last manual step in the
# update treadmill. Running it here makes it self-healing on every boot, and the patch is
# idempotent so a no-op boot costs nothing.
#
# patch-gameinfo.sh is invoked as a SUBPROCESS, never sourced: it runs `set -euo pipefail`
# and exits 1 when gameinfo.gi is missing, which would terminate entry.sh itself if sourced.
# entry.sh has no `set -e`, so a non-zero subprocess here is non-fatal — warn and boot on.
if [ -f /patch-gameinfo.sh ]; then
    bash /patch-gameinfo.sh || echo "[s2script] WARN: gameinfo patch failed (Metamod may not load)"
else
    echo "[s2script] WARN: /patch-gameinfo.sh not mounted — skipping gameinfo patch"
fi
```

- [ ] **Step 4: Mount it in the primary's compose**

In `docker/docker-compose.yml`, find the existing `patch-gameinfo.sh` mount:

```yaml
      # gameinfo.gi patcher script (called manually after first CS2 download)
      - ./patch-gameinfo.sh:/patch-gameinfo.sh:ro
```

Replace those two lines with:

```yaml
      # gameinfo.gi patcher script — invoked automatically by pre.sh on every boot.
      - ./patch-gameinfo.sh:/patch-gameinfo.sh:ro

      # Boot hook: entry.sh sources pre.sh AFTER its steamcmd app_update and BEFORE srcds
      # starts, so this re-patches the Metamod SearchPath that a CS2 update wipes — retiring
      # the manual `docker exec s2script-cs2 /patch-gameinfo.sh` step. Nested inside the
      # cs2-data mount above; Docker resolves by longest target path (same trick as configs/).
      - ./pre.sh:/home/steam/cs2-dedicated/pre.sh:ro
```

- [ ] **Step 5: Run test to verify it passes**

Run: `bash scripts/test-gate.sh`
Expected: PASS — prints `  pre.sh hook OK`

- [ ] **Step 6: Verify the compose file is still valid**

Run: `docker compose -f docker/docker-compose.yml config >/dev/null && echo "compose OK"`
Expected: `compose OK` (this only parses the file — it does not touch the running container).

- [ ] **Step 7: Commit**

```bash
git add docker/pre.sh docker/docker-compose.yml scripts/test-gate.sh
git commit -m "feat(gate): auto-patch gameinfo.gi every boot via the image's pre.sh hook"
```

---

### Task 3: `docker/docker-compose.gate.yml`

Today's compose, with every path env-driven so one file serves any worktree. It is fed exclusively by `.gate/gate.env` (Task 4) — the `:?` guards make a hand-run fail loudly rather than silently mount the wrong tree.

**Files:**
- Create: `docker/docker-compose.gate.yml`
- Test: `scripts/test-gate.sh` (append a section)

**Interfaces:**
- Consumes: `docker/pre.sh` (Task 2).
- Produces: a compose file requiring these env vars, which Task 4's `gate.sh` must write into `.gate/gate.env`: `GATE_NAME`, `GATE_PORT`, `GATE_CS2_DATA`, `GATE_S2SCRIPT_DIR`, `GATE_METAMOD_DIR`. Optional with defaults: `GATE_MAXPLAYERS` (12), `GATE_STARTMAP` (de_inferno), `GATE_DAMAGE_SELFTEST` (0).

- [ ] **Step 1: Write the failing test**

Append to `scripts/test-gate.sh`, immediately before the final `echo "PASS: test-gate.sh"` line:

```bash
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-gate.sh`
Expected: FAIL — `FAIL: docker/docker-compose.gate.yml missing`

- [ ] **Step 3: Create `docker/docker-compose.gate.yml`**

```yaml
# Per-worktree live-gate instance — one CS2 server per git worktree, sharing the single
# ~74 G install via a btrfs reflink clone. See:
#   docs/superpowers/specs/2026-07-15-multi-instance-live-gate-design.md
#
# Do NOT run this by hand. scripts/gate.sh resolves every path to an absolute one and writes
# .gate/gate.env; the `:?` guards below turn a bare `docker compose -f … up` into a loud
# error rather than a silently wrong mount.
#
# The primary folder keeps docker-compose.yml (container s2script-cs2, port 27015) unchanged.
services:
  cs2:
    image: joedwards32/cs2
    container_name: ${GATE_NAME:?gate.sh must set GATE_NAME (run scripts/gate.sh up)}
    stdin_open: true
    tty: true
    environment:
      CS2_SERVERNAME: "${GATE_NAME}"
      CS2_LAN: "1"
      CS2_RCONPW: "s2script"
      # 1:1 with the host port — Source 2 serves game traffic, A2S queries and RCON on this
      # one number, so a 1:1 map keeps the advertised port identical to the reachable one.
      CS2_PORT: "${GATE_PORT:?gate.sh must set GATE_PORT}"
      CS2_MAXPLAYERS: "${GATE_MAXPLAYERS:-12}"
      CS2_GAMETYPE: "0"
      CS2_GAMEMODE: "1"
      CS2_STARTMAP: "${GATE_STARTMAP:-de_inferno}"
      # Off by default (production default). A damage-hook gate sets GATE_DAMAGE_SELFTEST=1.
      S2_DAMAGE_SELFTEST: "${GATE_DAMAGE_SELFTEST:-0}"
      SRCDS_TOKEN: ""
    ports:
      - "${GATE_PORT}:${GATE_PORT}/udp"
      - "${GATE_PORT}:${GATE_PORT}/tcp"
    volumes:
      # This instance's private reflink clone of the install (gate.sh makes it: 0.5s, 0 disk).
      - ${GATE_CS2_DATA:?gate.sh must set GATE_CS2_DATA}:/home/steam/cs2-dedicated

      # The addon under test. Defaults to this worktree's dist/addons/s2script; --addons or
      # --s2script repoint it. :ro blocks CONTAINER writes; the host still drops .s2sp files in.
      - ${GATE_S2SCRIPT_DIR:?gate.sh must set GATE_S2SCRIPT_DIR}:/home/steam/cs2-dedicated/game/csgo/addons/s2script:ro

      # Nested RW mounts over the :ro addon (Docker resolves by longest target path): the shim
      # auto-generates configs/<id>.json and @s2script/db creates data/<name>.sqlite at runtime.
      - ${GATE_S2SCRIPT_DIR}/configs:/home/steam/cs2-dedicated/game/csgo/addons/s2script/configs
      - ${GATE_S2SCRIPT_DIR}/data:/home/steam/cs2-dedicated/game/csgo/addons/s2script/data

      # Metamod:Source. Cloned per instance rather than shared :ro — MM writes metamod-fatal.log.
      - ${GATE_METAMOD_DIR:?gate.sh must set GATE_METAMOD_DIR}:/home/steam/cs2-dedicated/game/csgo/addons/metamod

      # gameinfo.gi patcher + the boot hook that runs it automatically (see docker/pre.sh).
      - ./patch-gameinfo.sh:/patch-gameinfo.sh:ro
      - ./pre.sh:/home/steam/cs2-dedicated/pre.sh:ro
```

- [ ] **Step 4: Run test to verify it passes**

Run: `bash scripts/test-gate.sh`
Expected: PASS — prints `  docker-compose.gate.yml OK`

- [ ] **Step 5: Commit**

```bash
git add docker/docker-compose.gate.yml scripts/test-gate.sh
git commit -m "feat(gate): env-driven per-worktree compose file"
```

---

### Task 4: `scripts/gate.sh`

The CLI. Pure helpers are sourceable (`GATE_LIB_ONLY=1`) so they can be unit-tested without Docker.

**Files:**
- Create: `scripts/gate.sh`
- Modify: `.gitignore`
- Test: `scripts/test-gate.sh` (append a section)

**Interfaces:**
- Consumes: `docker/docker-compose.gate.yml` and its required vars (Task 3); `scripts/rcon.py --port` (Task 1) for the hint it prints.
- Produces: `scripts/gate.sh {up|down|destroy|status}`; sourceable helpers `gate_find_primary`, `gate_is_primary`, `gate_instance_name <dir>`, `gate_port_free <port>`, `gate_claim_port`; writes `.gate/gate.env`.

- [ ] **Step 1: Write the failing test**

Append to `scripts/test-gate.sh`, immediately before the final `echo "PASS: test-gate.sh"` line:

```bash
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `bash scripts/test-gate.sh`
Expected: FAIL — `FAIL: scripts/gate.sh missing`

- [ ] **Step 3: Create `scripts/gate.sh`**

```bash
#!/usr/bin/env bash
# Per-worktree CS2 live gate: one server per worktree, sharing the single ~74 G install.
#
#   scripts/gate.sh up [--addons <dir> | --s2script <dir>]   boot this worktree's instance
#   scripts/gate.sh down                                     stop it, keep the clone
#   scripts/gate.sh destroy                                  stop it and delete the clone
#   scripts/gate.sh status                                   show this worktree's instance
#
# Design: docs/superpowers/specs/2026-07-15-multi-instance-live-gate-design.md
# Each instance gets a btrfs reflink clone of the primary's docker/cs2-data (0.5s, zero real
# disk), so it has a full independent writable install and the stock image runs unmodified.

GATE_PORT_LO=27016          # 27015 is the primary; Steam holds 27036 + 27060.
GATE_PORT_HI=27030
GATE_LOCK="${TMPDIR:-/tmp}/s2script-gate-ports.lock"

# ---------------------------------------------------------------------------
# Pure helpers (unit-tested via: GATE_LIB_ONLY=1 source scripts/gate.sh)
# ---------------------------------------------------------------------------

# Absolute path of the repo's MAIN worktree (the primary folder). docker/cs2-data and
# docker/metamod are gitignored, so they exist only there — a linked worktree has nothing
# of its own to clone from.
gate_find_primary() {
  dirname "$(git rev-parse --path-format=absolute --git-common-dir)"
}

# True (0) when cwd is the primary folder: in the main worktree --git-dir and
# --git-common-dir resolve to the same path; in a linked worktree they differ.
gate_is_primary() {
  local d c
  d="$(git rev-parse --path-format=absolute --git-dir)"
  c="$(git rev-parse --path-format=absolute --git-common-dir)"
  [ "$d" = "$c" ]
}

# s2script-sound -> s2script-cs2-sound ; /tmp/experiment -> s2script-cs2-experiment
gate_instance_name() {
  local base="${1%/}"          # strip any trailing slash first
  base="${base##*/}"
  case "$base" in
    s2script-*) echo "s2script-cs2-${base#s2script-}" ;;
    *)          echo "s2script-cs2-${base}" ;;
  esac
}

# True (0) when nothing on the host listens on $1 and no container publishes it.
# NOTE: written as if/then rather than `grep -q … && return 1` — under `set -e` the latter
# makes the whole compound return non-zero when grep finds nothing, exiting the script.
gate_port_free() {
  local p="$1"
  if ss -tuln 2>/dev/null | grep -qE "[:.]${p}([[:space:]]|$)"; then return 1; fi
  if docker ps --format '{{.Ports}}' 2>/dev/null | grep -qE "(^|[^0-9])${p}->"; then return 1; fi
  return 0
}

# Echo the first free port in [GATE_PORT_LO, GATE_PORT_HI]; non-zero if the range is full.
gate_claim_port() {
  local p
  for (( p=GATE_PORT_LO; p<=GATE_PORT_HI; p++ )); do
    if gate_port_free "$p"; then echo "$p"; return 0; fi
  done
  return 1
}

if [ "${GATE_LIB_ONLY:-0}" = "1" ]; then
  return 0 2>/dev/null || exit 0
fi

set -euo pipefail

# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

WORKTREE="$(git rev-parse --show-toplevel)"
GATE_DIR="$WORKTREE/.gate"
COMPOSE="$WORKTREE/docker/docker-compose.gate.yml"

die() { echo "[gate] ERROR: $*" >&2; exit 1; }
say() { echo "[gate] $*"; }

gate_require_worktree() {
  if gate_is_primary; then
    die "this is the primary folder — use 'docker compose -f docker/docker-compose.yml up -d'
       (container s2script-cs2 on port 27015). gate.sh is for linked worktrees."
  fi
}

# Reflink-clone the primary's install + metamod into .gate/ (first `up` only).
gate_clone() {
  local primary; primary="$(gate_find_primary)"
  [ -d "$primary/docker/cs2-data" ] || die "no install at $primary/docker/cs2-data — has the primary ever booted?"
  [ -d "$primary/docker/metamod" ]  || die "no Metamod at $primary/docker/metamod (see the Docker runbook in README.md)"

  # A clone taken mid-update would capture a half-updated tree.
  for d in downloading temp; do
    if [ -n "$(ls -A "$primary/docker/cs2-data/steamapps/$d" 2>/dev/null)" ]; then
      die "the primary is mid-steamcmd-update (steamapps/$d is non-empty) — wait, then retry"
    fi
  done

  mkdir -p "$GATE_DIR"
  if [ ! -d "$GATE_DIR/cs2-data" ]; then
    say "cloning install (reflink, ~0.5s, 0 disk) from $primary/docker/cs2-data"
    cp -a --reflink=always "$primary/docker/cs2-data" "$GATE_DIR/cs2-data" \
      || die "reflink clone failed — is /home still btrfs? (cp --reflink=always does not fall back)"
  fi
  if [ ! -d "$GATE_DIR/metamod" ]; then
    cp -a --reflink=always "$primary/docker/metamod" "$GATE_DIR/metamod" \
      || die "metamod clone failed"
  fi
}

gate_up() {
  local addons_dir="" s2_dir=""
  while [ $# -gt 0 ]; do
    case "$1" in
      --addons)   [ $# -ge 2 ] || die "--addons needs a directory"
                  addons_dir="$(cd "$2" && pwd)" || die "--addons: no such directory: $2"; shift 2 ;;
      --s2script) [ $# -ge 2 ] || die "--s2script needs a directory"
                  s2_dir="$(cd "$2" && pwd)" || die "--s2script: no such directory: $2"; shift 2 ;;
      *) die "unknown option: $1 (try: up [--addons <dir> | --s2script <dir>])" ;;
    esac
  done
  # if/then, not `[ … ] && [ … ] && die` — under `set -e` that compound exits the script
  # whenever the condition is FALSE (the common case).
  if [ -n "$addons_dir" ] && [ -n "$s2_dir" ]; then
    die "--addons and --s2script are mutually exclusive"
  fi

  gate_require_worktree
  [ -f "$COMPOSE" ] || die "missing $COMPOSE"
  gate_clone

  local name; name="$(gate_instance_name "$WORKTREE")"

  # Resolve the mount sources to ABSOLUTE paths (compose must not depend on cwd).
  local s2 mm
  if [ -n "$addons_dir" ]; then
    s2="$addons_dir/s2script"; mm="$addons_dir/metamod"
  elif [ -n "$s2_dir" ]; then
    s2="$s2_dir";              mm="$GATE_DIR/metamod"
  else
    s2="$WORKTREE/dist/addons/s2script"; mm="$GATE_DIR/metamod"
  fi

  [ -d "$s2" ] || die "no addon at $s2 — run 'npx s2script build' + scripts/package-addon.sh first"
  for sub in configs data; do
    [ -d "$s2/$sub" ] || die "missing $s2/$sub — package-addon.sh creates it; Docker would
       otherwise create it root-owned and the container (uid 1000) could not write it"
    [ -w "$s2/$sub" ] || die "$s2/$sub is not writable by you (uid $(id -u)); the container runs as uid 1000"
  done
  [ -d "$mm/bin" ] || say "WARN: $mm has no bin/ — Metamod will not load (is this really a metamod dir?)"

  # Claim a port under a lock so two worktrees cannot race onto the same one. A port already
  # recorded in gate.env is reused if still free, so an instance keeps its port across up/down.
  local port=""
  exec 9>"$GATE_LOCK"
  flock 9
  if [ -f "$GATE_DIR/gate.env" ]; then
    port="$(grep -E '^GATE_PORT=' "$GATE_DIR/gate.env" 2>/dev/null | cut -d= -f2 || true)"
    if [ -n "$port" ] && ! gate_port_free "$port"; then
      # Ours from a previous `up`? Then it is not really taken by someone else.
      if ! docker ps -a --format '{{.Names}}' | grep -qx "$name"; then
        say "recorded port $port is now taken by something else — re-claiming"
        port=""
      fi
    fi
  fi
  [ -n "$port" ] || port="$(gate_claim_port)" || die "no free port in ${GATE_PORT_LO}-${GATE_PORT_HI}"

  cat > "$GATE_DIR/gate.env" <<EOF
# Written by scripts/gate.sh — do not edit by hand. All paths are absolute.
GATE_NAME=$name
GATE_PORT=$port
GATE_CS2_DATA=$GATE_DIR/cs2-data
GATE_S2SCRIPT_DIR=$s2
GATE_METAMOD_DIR=$mm
EOF
  flock -u 9
  exec 9>&-

  say "worktree : $WORKTREE"
  say "instance : $name"
  say "port     : $port"
  say "addon    : $s2"
  say "metamod  : $mm"
  docker compose --env-file "$GATE_DIR/gate.env" -f "$COMPOSE" -p "$name" up -d
  say "ready -> python3 scripts/rcon.py --port $port \"meta list\""
  say "         docker logs -f $name"
}

gate_compose_do() {
  gate_require_worktree
  [ -f "$GATE_DIR/gate.env" ] || die "no instance here yet (run: scripts/gate.sh up)"
  local name; name="$(gate_instance_name "$WORKTREE")"
  docker compose --env-file "$GATE_DIR/gate.env" -f "$COMPOSE" -p "$name" "$@"
}

gate_down()    { gate_compose_do down; say "stopped (clone kept — 'destroy' removes it)"; }

gate_destroy() {
  gate_compose_do down || true
  rm -rf "$GATE_DIR"
  say "destroyed (clone removed)"
}

gate_status() {
  gate_require_worktree
  [ -f "$GATE_DIR/gate.env" ] || { say "no instance in this worktree"; return 0; }
  cat "$GATE_DIR/gate.env"
  docker ps --filter "name=$(gate_instance_name "$WORKTREE")" \
            --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}'
}

case "${1:-}" in
  up)      shift; gate_up "$@" ;;
  down)    gate_down ;;
  destroy) gate_destroy ;;
  status)  gate_status ;;
  *) echo "usage: scripts/gate.sh {up [--addons <dir> | --s2script <dir>] | down | destroy | status}" >&2
     exit 2 ;;
esac
```

- [ ] **Step 4: Add `.gate/` to `.gitignore`**

In `.gitignore`, replace this line:

```
docker/metamod/
```

with:

```
docker/metamod/
.gate/
```

- [ ] **Step 5: Run test to verify it passes**

Run: `bash scripts/test-gate.sh`
Expected: PASS — prints `  gate.sh helpers OK` then `PASS: test-gate.sh`

- [ ] **Step 6: Verify gate.sh refuses to run in the primary folder**

Run: `cd /home/gkh/projects/s2script && bash scripts/gate.sh status; cd -`

Expected: exits non-zero with `[gate] ERROR: this is the primary folder — use 'docker compose ...'`.
Note: `scripts/gate.sh` does not exist in the primary until this branch merges, so if it reports "No such file or directory" instead, run the check from the gate worktree against the primary path:
`(cd /home/gkh/projects/s2script && bash /home/gkh/projects/s2script-gate/scripts/gate.sh status)` — expect the same refusal.

- [ ] **Step 7: Document it in the README**

`README.md` has a `## Docker verification runbook` section (line ~167) covering the single-server
setup. Append this subsection to the end of that section — without it, nothing tells a human that
`gate.sh` exists or how update day now works:

```markdown
### Running a second server (one gate per worktree)

The primary folder runs the server you already know: `docker compose -f docker/docker-compose.yml
up -d` → container `s2script-cs2` on port 27015. That is unchanged.

Any *linked worktree* can run its own server at the same time, sharing the one ~74 G install:

    cd ~/projects/s2script-sound
    scripts/package-addon.sh          # build this worktree's dist/addons/s2script first
    scripts/gate.sh up                # -> its own container + a port in 27016-27030
    python3 scripts/rcon.py --port <N> "meta list"
    scripts/gate.sh down              # stop, keep the clone
    scripts/gate.sh destroy           # stop and delete the clone

`gate.sh up` reflink-clones `docker/cs2-data` into the worktree's gitignored `.gate/` — 0.5s and
zero real disk, because /home is btrfs. Each instance therefore has a *full independent* install,
which is why two servers never corrupt each other. (`du` will report ~74 G per clone; that is
reflinked extents counted per-file. `df` is the truth.)

To point a gate at an addon build other than the worktree's own:

    scripts/gate.sh up --addons   <dir>    # <dir> holds metamod/ + s2script/ (e.g. a release zip)
    scripts/gate.sh up --s2script <dir>    # <dir> IS the s2script folder

**Update day.** The primary updates `docker/cs2-data` on boot as always. Instance clones do not
follow it, and an instance's own boot would update its clone in place (costing that clone real
disk). So the rule is: **update the primary, then `gate.sh destroy` + `gate.sh up` each instance**
— re-cloning is 0.5s, in-place updating is not. `gameinfo.gi` re-patching is now automatic on every
boot (`docker/pre.sh`), so the old manual `docker exec … /patch-gameinfo.sh` step is gone.
```

- [ ] **Step 8: Commit**

```bash
git add scripts/gate.sh scripts/test-gate.sh .gitignore README.md
git commit -m "feat(gate): scripts/gate.sh — per-worktree CS2 instance via reflink clone"
```

---

### Task 5: Live gate

The spec's verification list, run for real. This is the only task that starts containers.

**Files:** none (verification only).

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: evidence for the PR body.

- [ ] **Step 1: Record the baseline**

```bash
df -h /home | tail -1                      # free space BEFORE any clone
docker ps --format '{{.Names}}\t{{.Status}}' | grep s2script-cs2
docker inspect s2script-cs2 --format 'RestartCount={{.RestartCount}}'
```

Expected: note the free figure; the primary is `Up`, `RestartCount=0`.

- [ ] **Step 2: Give this worktree a `dist/`, then bring a gate up**

`package-addon.sh` assembles `dist/` from `build/shim/s2script.so` and
`target/**/libs2script_core.so` — **this worktree has neither** (both are gitignored, and a sniper
rebuild takes many minutes). This slice changes no core, shim, or op, so the primary's already-built
artifacts are byte-for-byte valid here. Reflink-copy its `dist` rather than rebuilding:

```bash
cd /home/gkh/projects/s2script-gate
cp -a --reflink=auto /home/gkh/projects/s2script/dist ./dist
ls dist/addons/s2script/bin/linuxsteamrt64/          # expect s2script.so + libs2script_core.so
bash scripts/gate.sh up
```

Expected: `[gate] cloning install (reflink, ~0.5s, 0 disk)`, then instance `s2script-cs2-gate`, a
port in 27016-27030, and `ready -> python3 scripts/rcon.py --port <N> ...`.

Note `--reflink=auto` (not `always`) here: `dist/` may sit on a different filesystem than the
primary in some setups, and a real copy of a ~145 M dist is an acceptable fallback — whereas for the
74 G install `always` is deliberate, so a silent full copy can never happen.

- [ ] **Step 2b: Prove the `--s2script` flag repoints the addon mount**

```bash
bash scripts/gate.sh destroy
bash scripts/gate.sh up --s2script /home/gkh/projects/s2script-gate/dist/addons/s2script
grep GATE_S2SCRIPT_DIR .gate/gate.env
docker inspect s2script-cs2-gate --format '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{"\n"}}{{end}}' | grep s2script
```

Expected: `gate.env` records the absolute path passed, and the container's mount table shows it
bound at `/home/steam/cs2-dedicated/game/csgo/addons/s2script` — proving the flexible mount works.

- [ ] **Step 3: Prove the clone consumed no real disk**

```bash
df -h /home | tail -1
```

Expected: free space **unchanged** from Step 1 (the `du` of `.gate/cs2-data` will read ~74 G — that is reflinked extents counted per-file; `df` is the truth).

- [ ] **Step 4: Prove the instance is a real, working server**

```bash
PORT=$(grep GATE_PORT= .gate/gate.env | cut -d= -f2)
docker logs "s2script-cs2-gate" 2>&1 | grep -E "GAMEDATA VALIDATION|gameinfo.gi|s2script"
python3 scripts/rcon.py --port "$PORT" "meta list" "sm_say gate-instance-alive"
```

Expected: `GAMEDATA VALIDATION: 15 ok, 0 FAILED` (or the current count), Metamod lists s2script, and rcon responds on the instance's own port.

- [ ] **Step 5: Prove the primary is untouched**

```bash
docker inspect s2script-cs2 --format 'RestartCount={{.RestartCount}}'
python3 scripts/rcon.py "meta version"
```

Expected: `RestartCount=0`, and the primary still answers on 27015 — two servers live at once.

- [ ] **Step 6: Prove `pre.sh` self-heals a wiped gameinfo**

```bash
grep -c "csgo/addons/metamod" .gate/cs2-data/game/csgo/gameinfo.gi   # expect 1
# Simulate what a CS2 update does: strip the Metamod SearchPath.
sed -i '/csgo\/addons\/metamod/d' .gate/cs2-data/game/csgo/gameinfo.gi
grep -c "csgo/addons/metamod" .gate/cs2-data/game/csgo/gameinfo.gi   # expect 0
docker restart "s2script-cs2-gate" && sleep 45
grep -c "csgo/addons/metamod" .gate/cs2-data/game/csgo/gameinfo.gi   # expect 1 again
docker logs "s2script-cs2-gate" 2>&1 | tail -40 | grep -i "gameinfo\|meta"
```

Expected: the count goes 1 → 0 → **1**, and Metamod loads — proving the boot hook re-patched it with no manual `docker exec`.

- [ ] **Step 7: Prove two instances coexist**

```bash
cd /home/gkh/projects/s2script-sound && bash /home/gkh/projects/s2script-gate/scripts/gate.sh up
```

Expected: a *different* port claimed, both containers `Up`, and `rcon.py --port` reaching each independently. (Skip if that worktree has no built `dist/` — note it in the PR instead.)

- [ ] **Step 8: Tear down and confirm cleanup**

```bash
cd /home/gkh/projects/s2script-sound && bash /home/gkh/projects/s2script-gate/scripts/gate.sh destroy
cd /home/gkh/projects/s2script-gate && bash scripts/gate.sh destroy
df -h /home | tail -1
docker inspect s2script-cs2 --format 'RestartCount={{.RestartCount}}'
```

Expected: both instances gone, free space back to the Step-1 baseline, primary still `Up` with `RestartCount=0`.

- [ ] **Step 9: Commit the evidence**

```bash
git commit --allow-empty -m "test(gate): live gate — two CS2 instances on one 74G install

- reflink clone: 0.5s, df free space unchanged
- instance answers rcon on its own port; primary untouched (RestartCount=0)
- pre.sh self-healed a deliberately wiped gameinfo.gi on reboot"
```

---

## Notes for the implementer

- **`du` lies about clones, `df` does not.** A reflink clone reports ~74 G under `du` because extents are counted per-file. Only `df` free space proves the sharing. Do not "fix" this.
- **`cp --reflink=always` never falls back** to a real copy — it fails instead. That is deliberate: a silent 74 G copy would be far worse than an error.
- **Do not restart or recreate the primary's container.** A live gate may be running. Task 2 changes the primary's compose file, but the change only takes effect the next time *the user* restarts it.
- **The accepted risk** (documented in the spec): if steamcmd fails twice on an instance boot, `entry.sh` does `rm -rf steamapps` and re-downloads ~74 G into that clone's real disk. It is contained to the one instance. The response is `gate.sh destroy`, not a code change.
