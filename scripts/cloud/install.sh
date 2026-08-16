#!/usr/bin/env bash
# Cloud Agent `install` — idempotent, durable setup run after checkout.
#
# Covers BOTH sides of the project:
#   1. the TypeScript / npm dev loop (always) — `npm install` across the workspaces;
#   2. the live CS2 gate (best-effort) — the native "sniper" build + Metamod:Source so
#      s2script can load on a real CS2 server in the VM.
#
# It is safe to re-run: every step is guarded so a second run is fast. The ~71 GB CS2
# game download is NOT here — that is runtime data brought up by start.sh and kept on
# disk (persisted by the environment snapshot), so `install` stays terminating.
set -euo pipefail
cd "$(dirname "$0")/../.."
REPO="$PWD"

echo "==> [install] npm workspaces"
npm install --no-fund --no-audit

# ---------------------------------------------------------------------------
# Live CS2 gate. Everything below is best-effort: if Docker cannot be installed
# or started in this context, the npm loop above still succeeded, so we do not
# fail the whole install — we warn and continue. A task that needs the live gate
# can finish the setup by hand (see AGENTS.md → "Live CS2 gate").
# ---------------------------------------------------------------------------
live_gate() {
  echo "==> [install] git submodules (native build needs hl2sdk + metamod-source)"
  git submodule update --init --recursive

  echo "==> [install] Docker engine (DinD: fuse-overlayfs + iptables-legacy)"
  if ! command -v docker >/dev/null 2>&1; then
    export DEBIAN_FRONTEND=noninteractive
    sudo install -m 0755 -d /etc/apt/keyrings
    curl --retry 3 --retry-delay 5 -fsSL https://download.docker.com/linux/ubuntu/gpg \
      | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    sudo chmod a+r /etc/apt/keyrings/docker.gpg
    echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu $(. /etc/os-release && echo "$VERSION_CODENAME") stable" \
      | sudo tee /etc/apt/sources.list.d/docker.list >/dev/null
    sudo apt-get update -qq
    sudo apt-get install -y -o Dpkg::Options::=--force-confold \
      docker-ce docker-ce-cli containerd.io docker-compose-plugin fuse-overlayfs iptables
  fi
  # fuse-overlayfs is the only storage driver that works nested in this VM; Docker 29
  # defaults containerd-snapshotter=false, which is what makes fuse-overlayfs usable.
  sudo mkdir -p /etc/docker
  printf '{\n  "storage-driver": "fuse-overlayfs"\n}\n' | sudo tee /etc/docker/daemon.json >/dev/null
  sudo update-alternatives --set iptables /usr/sbin/iptables-legacy >/dev/null 2>&1 || true
  sudo update-alternatives --set ip6tables /usr/sbin/ip6tables-legacy >/dev/null 2>&1 || true
  sudo usermod -aG docker "$USER" 2>/dev/null || true

  # dockerd is needed for the sniper build below; start.sh owns the per-boot start.
  bash scripts/cloud/dockerd-up.sh

  echo "==> [install] sniper build (loadable glibc<=2.31 addon binaries)"
  if [ ! -f dist/addons/s2script/bin/linuxsteamrt64/s2script.so ]; then
    sudo docker run --rm -v "$REPO:/repo" -w /repo \
      -v s2script-cargo:/usr/local/cargo/registry \
      rust:bullseye bash /repo/scripts/build-sniper.sh
  else
    echo "    addon binaries present — skipping (rebuild by hand after core/shim changes)"
  fi

  echo "==> [install] base plugins -> addon drop zone"
  bash scripts/build-base-plugins.sh >/dev/null
  cp plugins/*/dist/*.s2sp dist/addons/s2script/plugins/ 2>/dev/null || true

  echo "==> [install] Metamod:Source (CS2)"
  if [ ! -f docker/metamod/bin/linuxsteamrt64/metamod.2.cs2.so ]; then
    mkdir -p docker/metamod
    latest="$(curl -fsSL https://mms.alliedmods.net/mmsdrop/2.0/mmsource-latest-linux)"
    curl -fsSL "https://mms.alliedmods.net/mmsdrop/2.0/${latest}" -o /tmp/mms.tar.gz
    rm -rf /tmp/mms && mkdir -p /tmp/mms && tar xzf /tmp/mms.tar.gz -C /tmp/mms
    cp -r /tmp/mms/addons/metamod/* docker/metamod/
  else
    echo "    metamod.2.cs2.so present — skipping"
  fi

  echo "==> [install] live CS2 gate ready (run start.sh to boot the server)"
}

if ! live_gate; then
  echo "WARN: live CS2 gate setup did not complete — npm loop is still ready." >&2
  echo "WARN: finish the gate by hand per AGENTS.md → 'Live CS2 gate'." >&2
fi

echo "==> [install] done"
