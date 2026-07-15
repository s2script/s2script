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
