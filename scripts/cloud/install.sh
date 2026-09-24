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
# Metamod: a present metamod.2.cs2.so is not enough. Refresh is gated by
# scripts/verify-metamod-artifact.py against an independently supplied build
# manifest. Operators can select an official stock archive with an expected
# checksum and confirmed PLAPI, or supply a tree and independent manifest.
# An unmodified pinned source build is an optional test input. Automatic
# moving-latest selection is disabled. Stage on the destination filesystem, write
# the receipt into the staged tree, then rename; a failed swap rolls back to
# the complete previous tree. `docker/s2script.vdf` is restored after every
# successful swap. Marker strings and pin-origin directory names are not trust.
set -euo pipefail

# ---------------------------------------------------------------------------
# Paths. When sourced (fixture tests), skip the top-level `cd` so the caller
# keeps their cwd; functions always resolve through S2_SCRIPT_REPO / this file.
# ---------------------------------------------------------------------------
_S2_CLOUD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
S2_SCRIPT_REPO="${S2_SCRIPT_REPO:-$(cd "$_S2_CLOUD_DIR/../.." && pwd)}"

# Stock Metamod supports the plugin API required by this shim.
S2_METAMOD_IDENTITY_NAME=".s2script-metamod-identity"
S2_METAMOD_BUILD_COPY_NAME=".s2script-metamod-build.json"
S2_METAMOD_SO_REL="bin/linuxsteamrt64/metamod.2.cs2.so"
S2_METAMOD_VERIFY_PY="$S2_SCRIPT_REPO/scripts/verify-metamod-artifact.py"
# Official release 2.0.0.1469 targets the checked upstream pin fa6f80e4662e.
# Its source declares PLAPI 18; GitHub publishes this asset checksum (not a signature).
S2_METAMOD_STOCK_RELEASE_URL="https://github.com/alliedmodders/metamod-source/releases/download/2.0.0.1469/mmsource-2.0.0-git1469-linux.tar.gz"
S2_METAMOD_STOCK_RELEASE_SHA256="a552e4cb1399ced15a1192880f1f6bdd3d16f930a9cb33bc9e45396a26f177ad"

s2_metamod_so() { echo "$1/$S2_METAMOD_SO_REL"; }
s2_metamod_identity() { echo "$1/$S2_METAMOD_IDENTITY_NAME"; }
s2_metamod_build_copy() { echo "$1/$S2_METAMOD_BUILD_COPY_NAME"; }

s2_sha256() {
  sha256sum "$1" | awk '{print $1}'
}

s2_inject_fail() {
  local point="$1"
  if [ "${S2_METAMOD_INJECT_FAIL:-}" = "$point" ]; then
    echo "    injected failure at $point" >&2
    return 0
  fi
  return 1
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

s2_identity_get() {
  local file="$1" key="$2"
  [ -f "$file" ] || return 1
  awk -F= -v k="$key" '$1==k {print $2; found=1; exit} END{exit found?0:1}' "$file"
}

# Independent verifier. Never invents a manifest from the candidate.
# S2_METAMOD_VERIFY=inject-pass|inject-fail is a transaction-test hook only.
s2_metamod_verify() {
  local tree="$1" manifest="$2"
  case "${S2_METAMOD_VERIFY:-}" in
    inject-pass)
      echo "    verification injected: pass (transaction test; not a binary proof)"
      return 0
      ;;
    inject-fail)
      echo "error: injected_verification_failure: S2_METAMOD_VERIFY=inject-fail" >&2
      return 1
      ;;
  esac
  if [ -z "$manifest" ] || [ ! -f "$manifest" ]; then
    echo "error: missing_manifest: independent build manifest is required (never invented from the candidate)" >&2
    return 1
  fi
  if [ ! -d "$tree" ]; then
    echo "error: missing_tree: $tree" >&2
    return 1
  fi
  if ! command -v python3 >/dev/null 2>&1; then
    echo "error: python3_unavailable: python3 is required to verify Metamod artifacts" >&2
    return 1
  fi
  if [ ! -f "$S2_METAMOD_VERIFY_PY" ]; then
    echo "error: missing_verifier: $S2_METAMOD_VERIFY_PY" >&2
    return 1
  fi
  python3 "$S2_METAMOD_VERIFY_PY" --tree "$tree" --manifest "$manifest"
}

s2_metamod_resolve_source() {
  S2_MM_SRC_TREE=""
  S2_MM_SRC_MANIFEST=""
  S2_MM_SRC_KIND=""

  if [ -n "${S2_METAMOD_TREE:-}" ]; then
    S2_MM_SRC_TREE="$S2_METAMOD_TREE"
    if [ -z "${S2_METAMOD_BUILD_MANIFEST:-}" ]; then
      echo "error: missing_manifest: S2_METAMOD_TREE requires S2_METAMOD_BUILD_MANIFEST (independent artifact manifest)" >&2
      return 1
    fi
    S2_MM_SRC_MANIFEST="$S2_METAMOD_BUILD_MANIFEST"
    if [ ! -f "$S2_MM_SRC_MANIFEST" ]; then
      echo "error: missing_manifest: $S2_MM_SRC_MANIFEST does not exist" >&2
      return 1
    fi
    if [ ! -d "$S2_MM_SRC_TREE" ]; then
      echo "error: missing_tree: $S2_MM_SRC_TREE" >&2
      return 1
    fi
    S2_MM_SRC_KIND="supplied-tree"
    return 0
  fi

  if [ -n "${S2_METAMOD_BUILD_MANIFEST:-}" ]; then
    echo "error: missing_tree: S2_METAMOD_BUILD_MANIFEST needs S2_METAMOD_TREE for a fresh install" >&2
    return 1
  fi
  local cache archive release_url release_sha release_plapi
  if [ -n "${S2_METAMOD_RELEASE_URL:-}${S2_METAMOD_RELEASE_ARCHIVE:-}${S2_METAMOD_RELEASE_SHA256:-}${S2_METAMOD_RELEASE_PLAPI:-}" ]; then
    release_url="${S2_METAMOD_RELEASE_URL:-}"
    release_sha="${S2_METAMOD_RELEASE_SHA256:-}"
    release_plapi="${S2_METAMOD_RELEASE_PLAPI:-}"
  else
    release_url="$S2_METAMOD_STOCK_RELEASE_URL"
    release_sha="$S2_METAMOD_STOCK_RELEASE_SHA256"
    release_plapi=18  # checked source of this exact immutable release, never an arbitrary override
  fi
  cache="$S2_SCRIPT_REPO/build/metamod-release"
  mkdir -p "$cache" || return 1
  rm -f "$cache/metamod-build.json"
  if [ -z "$release_url" ] || [ -z "$release_sha" ] || [ "$release_plapi" != "18" ]; then
    echo "error: missing_release_identity: supply official RELEASE_URL, expected RELEASE_SHA256, and operator-confirmed RELEASE_PLAPI=18" >&2
    return 1
  fi
  # Validate origin before any network request. The checksum is independent
  # operator input; it is not claimed to be an upstream signature.
  python3 - "$S2_METAMOD_VERIFY_PY" "$release_url" <<'PY' || return 1
import importlib.util, sys
spec = importlib.util.spec_from_file_location("metamod_verifier", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.official_release_url(sys.argv[2])
PY
  archive="${S2_METAMOD_RELEASE_ARCHIVE:-$cache/release.tar.gz}"
  if [ -z "${S2_METAMOD_RELEASE_ARCHIVE:-}" ]; then
    "${S2_METAMOD_CURL:-curl}" --fail --location --proto '=https' --proto-redir '=https' \
      "$release_url" --output "$archive.tmp" || { rm -f "$archive.tmp"; return 1; }
    mv "$archive.tmp" "$archive" || return 1
  fi
  python3 "$S2_METAMOD_VERIFY_PY" --prepare-release "$archive" \
    --release-url "$release_url" --archive-sha256 "$release_sha" \
    --release-plapi "$release_plapi" --tree "$cache/tree" \
    --manifest "$cache/metamod-build.json" || return 1
  S2_MM_SRC_TREE="$cache/tree"
  S2_MM_SRC_MANIFEST="$cache/metamod-build.json"
  S2_MM_SRC_KIND="official-release"
  return 0

}

s2_metamod_skip_manifest() {
  local dest="$1"
  if [ -n "${S2_METAMOD_BUILD_MANIFEST:-}" ]; then
    [ -f "$S2_METAMOD_BUILD_MANIFEST" ] || return 1
    echo "$S2_METAMOD_BUILD_MANIFEST"
    return 0
  fi
  [ -z "${S2_METAMOD_TREE:-}" ] || return 1
  local copied
  copied="$(s2_metamod_build_copy "$dest")"
  if [ -f "$copied" ]; then
    if [ -n "${S2_METAMOD_RELEASE_URL:-}${S2_METAMOD_RELEASE_ARCHIVE:-}${S2_METAMOD_RELEASE_SHA256:-}${S2_METAMOD_RELEASE_PLAPI:-}" ]; then
      python3 - "$copied" "${S2_METAMOD_RELEASE_URL:-}" "${S2_METAMOD_RELEASE_SHA256:-}" "${S2_METAMOD_RELEASE_PLAPI:-}" <<'PY' || return 1
import json, sys
doc = json.load(open(sys.argv[1]))
expected = {"kind": "official-release", "url": sys.argv[2], "archive_sha256": sys.argv[3]}
raise SystemExit(0 if doc.get("provenance") == expected and sys.argv[4] == "18" else 1)
PY
    fi
    echo "$copied"
    return 0
  fi
  return 1
}

# Skip only after a previous successful install (copied build manifest present)
# and a real re-check of artifact bytes. Injected verification is for candidate
# trees in transaction tests; it must not mark an unverified dest as skipped.
s2_metamod_tree_is_verified() {
  local dest="$1"
  local so manifest
  so="$(s2_metamod_so "$dest")"
  [ -f "$so" ] || return 1
  [ -f "$(s2_metamod_build_copy "$dest")" ] || return 1
  manifest="$(s2_metamod_skip_manifest "$dest")" || return 1
  python3 "$S2_METAMOD_VERIFY_PY" --tree "$dest" --manifest "$manifest"
}

s2_write_identity() {
  local dest="$1" source="$2" artifact="$3" manifest="$4"
  local so identity sha provenance plapi msha
  so="$(s2_metamod_so "$dest")"
  identity="$(s2_metamod_identity "$dest")"
  [ -f "$so" ] || return 1
  [ -f "$manifest" ] || return 1
  sha="$(s2_sha256 "$so")" || return 1
  msha="$(s2_sha256 "$manifest")" || return 1
  provenance="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["provenance"]["kind"])' "$manifest")" || return 1
  plapi="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["plapi"])' "$manifest")" || return 1
  if ! cat >"$identity" <<EOF
# Installation receipt written by scripts/cloud/install.sh after a verified swap.
# This is not the build manifest. The independent build manifest is copied beside it.
provenance=${provenance}
plapi=${plapi}
source=${source}
artifact=${artifact}
sha256=${sha}
manifest_sha256=${msha}
EOF
  then
    return 1
  fi
  [ -s "$identity" ] || return 1
}

s2_copy_tree() {
  local src="$1" dest="$2"
  rm -rf "$dest" || return 1
  mkdir -p "$dest" || return 1
  cp -a "$src"/. "$dest"/ || return 1
}

s2_preflight_tree() {
  local src="$1" dest_parent="$2"
  [ -d "$src" ] || return 1
  mkdir -p "$dest_parent" || return 1
  [ -w "$dest_parent" ] || return 1
  local f
  while IFS= read -r -d '' f; do
    [ -r "$f" ] || return 1
  done < <(find "$src" -type f -print0)
}

s2_preserve_s2script_vdf() {
  local dest="$1" backup="$2"
  if [ -f "$dest/s2script.vdf" ]; then
    cp -a "$dest/s2script.vdf" "$backup" || return 1
    return 0
  fi
  if [ -f "$S2_SCRIPT_REPO/docker/s2script.vdf" ]; then
    cp -a "$S2_SCRIPT_REPO/docker/s2script.vdf" "$backup" || return 1
    return 0
  fi
  return 1
}

s2_restore_s2script_vdf() {
  local dest="$1" backup="$2"
  mkdir -p "$dest" || return 1
  if [ -f "$backup" ] && [ -s "$backup" ]; then
    cp -a "$backup" "$dest/s2script.vdf" || return 1
  fi
  if [ -f "$S2_SCRIPT_REPO/docker/s2script.vdf" ]; then
    cp -a "$S2_SCRIPT_REPO/docker/s2script.vdf" "$dest/s2script.vdf" || return 1
  fi
}

s2_rollback_to_prev() {
  local dest="$1" prev="$2" stage="$3"
  if [ -e "$dest" ] && [ -e "$prev" ]; then
    if ! rm -rf "$dest"; then
      echo "    ERROR: rollback failed removing incomplete dest $dest" >&2
      rm -rf "$stage" || true
      return 1
    fi
  fi
  if [ -e "$prev" ] && [ ! -e "$dest" ]; then
    if ! mv "$prev" "$dest"; then
      echo "    ERROR: rollback failed restoring $prev -> $dest" >&2
      rm -rf "$stage" || true
      return 1
    fi
  fi
  rm -rf "$stage" || true
  return 0
}

# dest → dest.prev (one generation), complete staged tree → dest.
# Receipt and VDF are already inside stage. On any failure after dest has moved,
# restore dest from dest.prev. Never return success with a partial tree.
s2_replace_metamod_tree() {
  local dest="$1" stage="$2"
  local prev="${S2_METAMOD_PREV:-${dest}.prev}"

  if s2_cs2_is_running; then
    echo "    CS2 container is running — refusing to replace $dest (stop it first; do not --force-recreate)" >&2
    rm -rf "$stage"
    return 1
  fi

  mkdir -p "$(dirname "$dest")" || { rm -rf "$stage"; return 1; }
  rm -rf "$prev" || { rm -rf "$stage"; return 1; }

  if s2_inject_fail before-rename-dest; then
    rm -rf "$stage"
    return 1
  fi

  if [ -e "$dest" ]; then
    if ! mv "$dest" "$prev"; then
      echo "    ERROR: failed to move $dest -> $prev" >&2
      rm -rf "$stage"
      return 1
    fi
  fi

  if s2_inject_fail after-rename-dest; then
    s2_rollback_to_prev "$dest" "$prev" "$stage" || true
    return 1
  fi

  if s2_inject_fail before-rename-stage; then
    s2_rollback_to_prev "$dest" "$prev" "$stage" || true
    return 1
  fi

  if ! mv "$stage" "$dest"; then
    echo "    ERROR: failed to move staged Metamod into $dest — restoring previous tree" >&2
    s2_rollback_to_prev "$dest" "$prev" "$stage" || true
    return 1
  fi

  if s2_inject_fail after-rename-stage; then
    s2_rollback_to_prev "$dest" "$prev" "" || true
    return 1
  fi

  echo "    previous tree preserved at $prev"
  return 0
}

s2_ensure_metamod() {
  local dest="${1:-$S2_SCRIPT_REPO/docker/metamod}"
  local so dest_parent stage vdf_backup source artifact
  so="$(s2_metamod_so "$dest")"

  echo "==> [install] stock Metamod:Source (CS2), required PLAPI 18"

  if s2_metamod_tree_is_verified "$dest"; then
    echo "    verified artifact matches independent build manifest — skipping"
    if [ ! -f "$dest/s2script.vdf" ] && [ -f "$S2_SCRIPT_REPO/docker/s2script.vdf" ]; then
      cp -a "$S2_SCRIPT_REPO/docker/s2script.vdf" "$dest/s2script.vdf" || return 1
    fi
    return 0
  fi

  if [ -f "$so" ]; then
    echo "    installed tree does not match a verified stock artifact manifest — refreshing"
  else
    echo "    $S2_METAMOD_SO_REL missing — installing"
  fi

  if ! s2_metamod_resolve_source; then
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi

  if ! s2_metamod_verify "$S2_MM_SRC_TREE" "$S2_MM_SRC_MANIFEST"; then
    echo "    candidate failed independent verification — previous installation preserved at $dest" >&2
    return 1
  fi

  if s2_cs2_is_running; then
    echo "    CS2 container is running — refusing to replace $dest (stop it first; do not --force-recreate)" >&2
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi

  dest_parent="$(dirname "$dest")"
  if ! s2_preflight_tree "$S2_MM_SRC_TREE" "$dest_parent"; then
    echo "    preflight failed for $S2_MM_SRC_TREE -> $dest_parent" >&2
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi

  stage="${dest}.staging"
  rm -rf "$stage"
  if ! s2_copy_tree "$S2_MM_SRC_TREE" "$stage"; then
    echo "    ERROR: failed to copy candidate onto the destination filesystem" >&2
    rm -rf "$stage"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi
  rm -f "$(s2_metamod_identity "$stage")" "$(s2_metamod_build_copy "$stage")"

  if ! s2_metamod_verify "$stage" "$S2_MM_SRC_MANIFEST"; then
    echo "    staged copy failed independent verification — previous installation preserved at $dest" >&2
    rm -rf "$stage"
    return 1
  fi

  source="$S2_MM_SRC_KIND"
  artifact="manifest:$(s2_sha256 "$S2_MM_SRC_MANIFEST")"

  vdf_backup="$(mktemp)"
  s2_preserve_s2script_vdf "$dest" "$vdf_backup" || true

  if s2_inject_fail receipt-write; then
    rm -rf "$stage"
    rm -f "$vdf_backup"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi
  if ! s2_write_identity "$stage" "$source" "$artifact" "$S2_MM_SRC_MANIFEST"; then
    echo "    ERROR: failed to write installation receipt into the staged tree" >&2
    rm -rf "$stage"
    rm -f "$vdf_backup"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi
  if ! cp -a "$S2_MM_SRC_MANIFEST" "$(s2_metamod_build_copy "$stage")"; then
    echo "    ERROR: failed to copy independent build manifest into the staged tree" >&2
    rm -rf "$stage"
    rm -f "$vdf_backup"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi

  if s2_inject_fail vdf-write; then
    rm -rf "$stage"
    rm -f "$vdf_backup"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi
  if ! s2_restore_s2script_vdf "$stage" "$vdf_backup"; then
    echo "    ERROR: failed to write s2script.vdf into the staged tree" >&2
    rm -rf "$stage"
    rm -f "$vdf_backup"
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi
  rm -f "$vdf_backup"

  if ! s2_replace_metamod_tree "$dest" "$stage"; then
    echo "    previous installation preserved at $dest" >&2
    return 1
  fi

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

  s2_ensure_metamod "$S2_SCRIPT_REPO/docker/metamod" || return 1

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
