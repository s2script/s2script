#!/usr/bin/env bash
# Cloud Agent `start` — per-boot reconciliation. Idempotent and non-blocking:
# brings the Docker daemon and the live CS2 gate up, then returns. The CS2 server
# keeps running in its container; watch it via the `cs2-server` terminal or
# `sudo docker compose -f docker/docker-compose.yml logs -f cs2`.
set -euo pipefail
cd "$(dirname "$0")/../.."

# The live gate is optional — if Docker was never installed (npm-only setup), just
# return cleanly so boot succeeds.
if ! command -v docker >/dev/null 2>&1; then
  echo "==> [start] docker not installed — npm-only environment, nothing to start"
  exit 0
fi

echo "==> [start] docker daemon"
bash scripts/cloud/dockerd-up.sh

# The joedwards32/cs2 image runs srcds as uid 1000; a root-owned bind mount makes
# entry.sh fail to write post.sh/cfg. Ensure ownership before boot (cheap no-op once set).
if [ -d docker/cs2-data ]; then
  if [ "$(stat -c %u docker/cs2-data)" != "1000" ]; then
    echo "==> [start] chown docker/cs2-data -> uid 1000 (steam)"
    sudo chown -R 1000:1000 docker/cs2-data
  fi
fi

# The addon must exist for the :ro mount to carry anything. If install's live-gate
# step did not run, skip the server rather than mounting an empty addon.
if [ ! -f dist/addons/s2script/bin/linuxsteamrt64/s2script.so ]; then
  echo "==> [start] addon not built (dist/addons missing) — skipping CS2 server"
  echo "           run scripts/cloud/install.sh to build it, then re-run start.sh"
  exit 0
fi

echo "==> [start] CS2 dedicated server + db sidecars (compose up -d)"
# up -d is idempotent: already-running containers are left alone, and the ~71 GB CS2
# download (first boot only) proceeds inside the container without blocking this script.
sudo docker compose -f docker/docker-compose.yml up -d

echo "==> [start] done — 'meta list' via: python3 scripts/rcon.py \"meta list\""
