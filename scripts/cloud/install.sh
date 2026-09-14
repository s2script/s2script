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
#
# Metamod: a present metamod.2.cs2.so is not enough. Snapshots commonly still carry
# pre-PR-223 (PLAPI 17 / SourceHook) drops. Refresh is identity-gated against the
# tested pin (PLAPI 18) or a verified PLAPI 18 mmsdrop, staged then swapped only
# while CS2 is stopped. A failed/stale download never loops and never destroys the
# previous tree. `docker/s2script.vdf` is restored after every successful swap.
set -euo pipefail

# ---------------------------------------------------------------------------
# Paths. When sourced (fixture tests), skip the top-level `cd` so the caller
# keeps their cwd; functions always resolve through S2_SCRIPT_REPO / this file.
# ---------------------------------------------------------------------------
_S2_CLOUD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
S2_SCRIPT_REPO="${S2_SCRIPT_REPO:-$(cd "$_S2_CLOUD_DIR/../.." && pwd)}"

# Tested Metamod pin (vendored gitlink) + plugin API floor.
S2_METAMOD_PIN="${S2_METAMOD_PIN:-7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33}"
S2_METAMOD_PLAPI="${S2_METAMOD_PLAPI:-18}"
S2_MMSDROP_BASE="${S2_MMSDROP_BASE:-https://mms.alliedmods.net/mmsdrop/2.0}"
S2_METAMOD_IDENTITY_NAME=".s2script-metamod-identity"
S2_METAMOD_SO_REL="bin/linuxsteamrt64/metamod.2.cs2.so"

s2_metamod_so() { echo "$1/$S2_METAMOD_SO_REL"; }
s2_metamod_identity() { echo "$1/$S2_METAMOD_IDENTITY_NAME"; }

s2_sha256() {
  sha256sum "$1" | awk '{print $1}'
}

s2_curl() {
  if [ -n "${S2_METAMOD_CURL:-}" ]; then
    "$S2_METAMOD_CURL" "$@"
  else
    curl --retry 3 --retry-delay 5 "$@"
  fi
}

# CS2 bind-mounts docker/metamod/. Replacing that tree while the container is
# running is unsafe; Step 3 (restart) is a later package. Tests override this.
s2_cs2_is_running() {
  if [ -n "${S2_METAMOD_CS2_RUNNING:-}" ]; then
    [ "${S2_METAMOD_CS2_RUNNING}" = "1" ]
    return
  fi
  command -v docker >/dev/null 2>&1 || return 1
  [ "$(docker inspect -f '{{.State.Running}}' s2script-cs2 2>/dev/null || true)" = "true" ]
}

# strings | GetDetourInterface is a heuristic only — stripped binaries can omit
# the symbol name. fail = definitely pre-18; pass = likely PLAPI 18; unknown =
# cannot decide from the bytes (do not treat as verified).
s2_metamod_heuristic() {
  local so="$1"
  if [ ! -f "$so" ]; then
    echo missing
    return
  fi
  if grep -aF -q "GetDetourInterface" "$so"; then
    echo pass
    return
  fi
  if grep -aF -q "SourceHook version" "$so"; then
    echo fail
    return
  fi
  echo unknown
}

s2_identity_get() {
  local file="$1" key="$2"
  [ -f "$file" ] || return 1
  awk -F= -v k="$key" '$1==k {print $2; found=1; exit} END{exit found?0:1}' "$file"
}

s2_write_identity() {
  local dest="$1" source="$2" artifact="$3"
  local so identity sha
  so="$(s2_metamod_so "$dest")"
  identity="$(s2_metamod_identity "$dest")"
  sha="$(s2_sha256 "$so")"
  cat >"$identity" <<EOF
# Written by scripts/cloud/install.sh after a verified Metamod install.
# Authoritative later check is the loaded host: meta version → plugin interface ${S2_METAMOD_PLAPI}.
pin=${S2_METAMOD_PIN}
plapi=${S2_METAMOD_PLAPI}
source=${source}
artifact=${artifact}
sha256=${sha}
EOF
}

# Installed tree matches the selected pin / a previously verified PLAPI 18 drop.
# Identity + sha256 of the live .so is the skip gate. A SourceHook heuristic
# fail still forces refresh even if a sidecar claims PLAPI 18.
s2_metamod_tree_is_verified() {
  local dest="$1"
  local so identity sha pin plapi recorded
  so="$(s2_metamod_so "$dest")"
  identity="$(s2_metamod_identity "$dest")"
  [ -f "$so" ] && [ -f "$identity" ] || return 1
  pin="$(s2_identity_get "$identity" pin || true)"
  plapi="$(s2_identity_get "$identity" plapi || true)"
  recorded="$(s2_identity_get "$identity" sha256 || true)"
  sha="$(s2_sha256 "$so")"
  [ "$pin" = "$S2_METAMOD_PIN" ] || return 1
  [ "$plapi" = "$S2_METAMOD_PLAPI" ] || return 1
  [ -n "$recorded" ] && [ "$recorded" = "$sha" ] || return 1
  case "$(s2_metamod_heuristic "$so")" in
    fail) return 1 ;;
  esac
  return 0
}

# A staged tree is PLAPI 18 iff we can prove it before swapping:
#   * identity sidecar already names this pin + PLAPI + matching sha, or
#   * heuristic pass (GetDetourInterface present).
# Unknown/stripped drops are NOT verified — fall back to the pin rather than
# looping mmsdrop.
s2_metamod_stage_is_plapi18() {
  local stage="$1"
  local so identity
  so="$(s2_metamod_so "$stage")"
  [ -f "$so" ] || return 1
  identity="$(s2_metamod_identity "$stage")"
  if [ -f "$identity" ]; then
    local pin plapi recorded sha
    pin="$(s2_identity_get "$identity" pin || true)"
    plapi="$(s2_identity_get "$identity" plapi || true)"
    recorded="$(s2_identity_get "$identity" sha256 || true)"
    sha="$(s2_sha256 "$so")"
    if [ "$pin" = "$S2_METAMOD_PIN" ] && [ "$plapi" = "$S2_METAMOD_PLAPI" ] && [ "$recorded" = "$sha" ]; then
      case "$(s2_metamod_heuristic "$so")" in
        fail) return 1 ;;
        *) return 0 ;;
      esac
    fi
  fi
  [ "$(s2_metamod_heuristic "$so")" = "pass" ]
}

s2_find_extracted_metamod_root() {
  local extract="$1"
  if [ -f "$(s2_metamod_so "$extract")" ]; then
    echo "$extract"
    return 0
  fi
  if [ -f "$(s2_metamod_so "$extract/addons/metamod")" ]; then
    echo "$extract/addons/metamod"
    return 0
  fi
  return 1
}

s2_copy_tree() {
  local src="$1" dest="$2"
  rm -rf "$dest"
  mkdir -p "$dest"
  # Trailing /. copies contents even when dest already exists.
  cp -a "$src"/. "$dest"/
}

s2_preserve_s2script_vdf() {
  local dest="$1" backup="$2"
  if [ -f "$dest/s2script.vdf" ]; then
    cp -a "$dest/s2script.vdf" "$backup"
    return 0
  fi
  if [ -f "$S2_SCRIPT_REPO/docker/s2script.vdf" ]; then
    cp -a "$S2_SCRIPT_REPO/docker/s2script.vdf" "$backup"
    return 0
  fi
  return 1
}

s2_restore_s2script_vdf() {
  local dest="$1" backup="$2"
  mkdir -p "$dest"
  if [ -f "$backup" ] && [ -s "$backup" ]; then
    cp -a "$backup" "$dest/s2script.vdf"
  fi
  if [ -f "$S2_SCRIPT_REPO/docker/s2script.vdf" ]; then
    cp -a "$S2_SCRIPT_REPO/docker/s2script.vdf" "$dest/s2script.vdf"
  fi
}

# Atomic-ish swap: dest → dest.prev (one generation), stage → dest, restore VDF.
# On any failure after dest has moved, restore dest from dest.prev.
s2_replace_metamod_tree() {
  local dest="$1" stage="$2"
  local prev="${S2_METAMOD_PREV:-${dest}.prev}"
  local vdf_backup
  vdf_backup="$(mktemp)"

  if s2_cs2_is_running; then
    echo "    CS2 container is running — refusing to replace $dest (stop it first; do not --force-recreate)" >&2
    rm -f "$vdf_backup"
    return 1
  fi

  s2_preserve_s2script_vdf "$dest" "$vdf_backup" || true

  mkdir -p "$(dirname "$dest")"
  rm -rf "$prev"
  if [ -e "$dest" ]; then
    mv "$dest" "$prev"
  fi
  if ! mv "$stage" "$dest"; then
    echo "    ERROR: failed to move staged Metamod into $dest — restoring previous tree" >&2
    if [ -e "$prev" ]; then
      mv "$prev" "$dest"
    fi
    rm -f "$vdf_backup"
    return 1
  fi
  s2_restore_s2script_vdf "$dest" "$vdf_backup"
  rm -f "$vdf_backup"
  echo "    previous tree preserved at $prev"
  return 0
}

# One mmsdrop attempt. Never called in a loop. Returns 0 with $1 populated as a
# staged metamod root; non-zero on download/extract/layout failure.
s2_stage_mmsdrop() {
  local stage="$1"
  local work tarball latest root
  work="$(mktemp -d)"
  tarball="$work/mms.tar.gz"

  echo "    fetching $S2_MMSDROP_BASE/mmsource-latest-linux"
  if ! latest="$(s2_curl -fsSL "$S2_MMSDROP_BASE/mmsource-latest-linux")"; then
    echo "    mmsdrop latest pointer failed" >&2
    rm -rf "$work"
    return 1
  fi
  latest="${latest//$'\r'/}"
  latest="${latest//$'\n'/}"
  if [ -z "$latest" ]; then
    echo "    mmsdrop latest pointer was empty" >&2
    rm -rf "$work"
    return 1
  fi
  echo "    downloading $S2_MMSDROP_BASE/${latest}"
  if ! s2_curl -fsSL "$S2_MMSDROP_BASE/${latest}" -o "$tarball"; then
    echo "    mmsdrop tarball download failed" >&2
    rm -rf "$work"
    return 1
  fi
  if ! tar xzf "$tarball" -C "$work"; then
    echo "    mmsdrop tarball was not a valid archive" >&2
    rm -rf "$work"
    return 1
  fi
  if ! root="$(s2_find_extracted_metamod_root "$work")"; then
    echo "    mmsdrop tarball missing $S2_METAMOD_SO_REL" >&2
    rm -rf "$work"
    return 1
  fi
  s2_copy_tree "$root" "$stage"
  printf '%s\n' "$latest" >"$stage/.s2script-drop-artifact"
  rm -rf "$work"
  return 0
}

# Pinned submodule build. We do not AMBuild Metamod inside the default cloud
# install (heavy, sniper/AMBuild-specific). Operators/tests supply a prebuilt
# tree via S2_METAMOD_PINNED_TREE or docker/metamod-pin/.
s2_stage_pinned_tree() {
  local stage="$1"
  local pin_src="${S2_METAMOD_PINNED_TREE:-}"
  if [ -z "$pin_src" ] && [ -f "$(s2_metamod_so "$S2_SCRIPT_REPO/docker/metamod-pin")" ]; then
    pin_src="$S2_SCRIPT_REPO/docker/metamod-pin"
  fi
  if [ -z "$pin_src" ] || [ ! -f "$(s2_metamod_so "$pin_src")" ]; then
    echo "    no prebuilt pin tree (set S2_METAMOD_PINNED_TREE or docker/metamod-pin/)" >&2
    return 1
  fi
  echo "    staging pinned Metamod from $pin_src"
  s2_copy_tree "$pin_src" "$stage"
  return 0
}

s2_ensure_metamod() {
  local dest="${1:-$S2_SCRIPT_REPO/docker/metamod}"
  local so stage artifact source
  so="$(s2_metamod_so "$dest")"

  echo "==> [install] Metamod:Source (CS2) — pin ${S2_METAMOD_PIN:0:12} / PLAPI ${S2_METAMOD_PLAPI}"

  if s2_metamod_tree_is_verified "$dest"; then
    echo "    verified pin/PLAPI ${S2_METAMOD_PLAPI} identity matches $(s2_sha256 "$so") — skipping"
    # Keep the VDF even on the skip path (package-addon.sh also copies it).
    if [ ! -f "$dest/s2script.vdf" ] && [ -f "$S2_SCRIPT_REPO/docker/s2script.vdf" ]; then
      cp -a "$S2_SCRIPT_REPO/docker/s2script.vdf" "$dest/s2script.vdf"
    fi
    return 0
  fi

  if [ -f "$so" ]; then
    echo "    installed tree is not the verified pin/PLAPI ${S2_METAMOD_PLAPI} identity (heuristic=$(s2_metamod_heuristic "$so")) — refreshing"
  else
    echo "    $S2_METAMOD_SO_REL missing — installing"
  fi

  stage="$(mktemp -d)"
  artifact=""
  source=""

  # Exactly one drop attempt. An unverified latest must not be re-fetched.
  if s2_stage_mmsdrop "$stage"; then
    if s2_metamod_stage_is_plapi18 "$stage"; then
      source="drop"
      artifact="$(cat "$stage/.s2script-drop-artifact" 2>/dev/null || echo unknown-drop)"
      rm -f "$stage/.s2script-drop-artifact"
    else
      echo "    staged mmsdrop is not a verified PLAPI ${S2_METAMOD_PLAPI} tree — not retrying the drop" >&2
      rm -rf "$stage"
      stage="$(mktemp -d)"
    fi
  else
    echo "    mmsdrop unavailable — not retrying the drop" >&2
    rm -rf "$stage"
    stage="$(mktemp -d)"
  fi

  if [ -z "$source" ]; then
    if s2_stage_pinned_tree "$stage"; then
      if s2_metamod_stage_is_plapi18 "$stage"; then
        source="pin"
        artifact="pin:${S2_METAMOD_PIN}"
      else
        echo "    pinned tree is not a verified PLAPI ${S2_METAMOD_PLAPI} artifact" >&2
        rm -rf "$stage"
        echo "    previous installation preserved at $dest" >&2
        return 1
      fi
    else
      echo "    no verified PLAPI ${S2_METAMOD_PLAPI} source available" >&2
      rm -rf "$stage"
      echo "    previous installation preserved at $dest" >&2
      return 1
    fi
  fi

  if ! s2_replace_metamod_tree "$dest" "$stage"; then
    rm -rf "$stage"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi

  s2_write_identity "$dest" "$source" "$artifact"
  echo "    installed $source ($artifact) sha256=$(s2_identity_get "$(s2_metamod_identity "$dest")" sha256)"
  return 0
}

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
    sudo docker run --rm -v "$S2_SCRIPT_REPO:/repo" -w /repo \
      -v s2script-cargo:/usr/local/cargo/registry \
      rust:bullseye bash /repo/scripts/build-sniper.sh
  else
    echo "    addon binaries present — skipping (rebuild by hand after core/shim changes)"
  fi

  echo "==> [install] base plugins -> addon drop zone"
  bash scripts/build-base-plugins.sh >/dev/null
  cp plugins/*/dist/*.s2sp dist/addons/s2script/plugins/ 2>/dev/null || true

  s2_ensure_metamod "$S2_SCRIPT_REPO/docker/metamod"

  echo "==> [install] live CS2 gate ready (run start.sh to boot the server)"
}

s2_install_main() {
  cd "$S2_SCRIPT_REPO"

  echo "==> [install] npm workspaces"
  npm install --no-fund --no-audit

  if ! live_gate; then
    echo "WARN: live CS2 gate setup did not complete — npm loop is still ready." >&2
    echo "WARN: finish the gate by hand per AGENTS.md → 'Live CS2 gate'." >&2
  fi

  echo "==> [install] done"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --metamod-only)
      s2_ensure_metamod "${S2_METAMOD_DEST:-$S2_SCRIPT_REPO/docker/metamod}"
      ;;
    "")
      s2_install_main
      ;;
    *)
      echo "usage: $0 [--metamod-only]" >&2
      exit 2
      ;;
  esac
fi
