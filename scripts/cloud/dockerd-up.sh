#!/usr/bin/env bash
# Start the Docker daemon if it is not already running. Idempotent: a no-op when
# `docker info` already works. There is no systemd in this VM (PID 1 is not
# systemd), so dockerd is launched directly and detached, logging to /tmp/dockerd.log.
set -euo pipefail

if sudo docker info >/dev/null 2>&1; then
  echo "    dockerd already running"
  exit 0
fi

echo "    starting dockerd (no systemd — launching directly)"
sudo bash -c 'nohup dockerd >/tmp/dockerd.log 2>&1 &'

for i in $(seq 1 30); do
  if sudo docker info >/dev/null 2>&1; then
    echo "    dockerd up"
    exit 0
  fi
  sleep 1
done

echo "ERROR: dockerd did not become ready in 30s — see /tmp/dockerd.log" >&2
tail -20 /tmp/dockerd.log >&2 || true
exit 1
