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
                  addons_dir="$(CDPATH= cd -- "$2" >/dev/null && pwd)" || die "--addons: no such directory: $2"; shift 2 ;;
      --s2script) [ $# -ge 2 ] || die "--s2script needs a directory"
                  s2_dir="$(CDPATH= cd -- "$2" >/dev/null && pwd)" || die "--s2script: no such directory: $2"; shift 2 ;;
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

  # Validated BEFORE gate_clone so a validation failure (the common first-run case) leaves
  # no side effects behind for `destroy` to clean up.
  [ -d "$s2" ] || die "no addon at $s2 — run 'npx s2script build' + scripts/package-addon.sh first"
  for sub in configs data; do
    [ -d "$s2/$sub" ] || die "missing $s2/$sub — package-addon.sh creates it; Docker would
       otherwise create it root-owned and the container (uid 1000) could not write it"
    [ -w "$s2/$sub" ] || die "$s2/$sub is not writable by you (uid $(id -u)); the container runs as uid 1000"
  done

  gate_clone
  [ -d "$mm/bin" ] || say "WARN: $mm has no bin/ — Metamod will not load (is this really a metamod dir?)"

  # Claim a port under a lock so two worktrees cannot race onto the same one. A port already
  # recorded in gate.env is reused if still free, so an instance keeps its port across up/down.
  # The lock is held through `docker compose up -d` below — that bind is the moment the port
  # actually becomes observable to another worktree's `ss`/`docker ps` read, so releasing the
  # lock any earlier would serialize nothing.
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

  say "worktree : $WORKTREE"
  say "instance : $name"
  say "port     : $port"
  say "addon    : $s2"
  say "metamod  : $mm"
  docker compose --env-file "$GATE_DIR/gate.env" -f "$COMPOSE" -p "$name" up -d
  flock -u 9
  exec 9>&-
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
  gate_require_worktree
  # gate_compose_do dies (exit — not caught by `|| true`, exit terminates the process even
  # inside a function) when gate.env is missing, which a failed first `up` can leave behind
  # (gate_clone ran but validation died before gate.env was written). Only call it when
  # there's actually an env file to compose down; the removal below must always run.
  if [ -f "$GATE_DIR/gate.env" ]; then
    gate_compose_do down || true
  fi
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
