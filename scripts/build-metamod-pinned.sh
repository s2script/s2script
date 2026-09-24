#!/usr/bin/env bash
# Optional test build of unmodified, pinned upstream Metamod and bundled KHook.
#
# Never rewrites the developer's submodule checkout. Prepares a fresh tree at the
# exact SHAs, then AMBuilds the unchanged source in the sniper
# environment. Runtime output stays untracked under build/metamod-pinned/.
#
# Missing AMBuild, hl2sdk, or sniper tools fail with a precise named reason. A
# planted prebuilt file is not success.
set -euo pipefail

PREPARE_ONLY=0
case "${1:-}" in
  --prepare-only) PREPARE_ONLY=1 ;;
  "") ;;
  *) echo "usage: $0 [--prepare-only]" >&2; exit 2 ;;
esac

REPO="$(cd "$(dirname "$0")/.." && pwd)"
S2_REPO="${S2_REPO:-$REPO}"

MMS_PIN="fa6f80e4662e5b96cc2e97722d812f374581dfd8"
KHOOK_PIN="40d233d160b5bf60cc3e732939142b222fbd8ece"
EXPECTED_PLAPI=18
TARGET="linux-x86_64"
GLIBC_MAX="2.31"

MMS_SUB="$S2_REPO/third_party/metamod-source"
OUT_ROOT="$S2_REPO/build/metamod-pinned"
TREE="$OUT_ROOT/tree"
MANIFEST="$OUT_ROOT/metamod-build.json"
BUILD_LOG="$OUT_ROOT/build.log"
ISOLATED="$OUT_ROOT/source"

log() { echo "$*"; echo "$*" >>"$BUILD_LOG"; }
fail() {
  echo "error: $*" >&2
  if [[ -n "${BUILD_LOG:-}" ]]; then
    echo "error: $*" >>"$BUILD_LOG" || true
  fi
  exit 1
}

mkdir -p "$OUT_ROOT"
# A failed rebuild must not leave an old success receipt usable by the installer.
rm -f "$MANIFEST"
: >"$BUILD_LOG"
log "==> build-metamod-pinned.sh"
log "repo=$S2_REPO"
log "date=$(date -u +%Y-%m-%dT%H:%M:%SZ)"

[[ -d "$MMS_SUB/core" ]] || fail "pinned Metamod source missing at $MMS_SUB"

mms_head="$(git -C "$MMS_SUB" rev-parse HEAD)"
khook_head="$(git -C "$MMS_SUB/third_party/khook" rev-parse HEAD)"
[[ "$mms_head" == "$MMS_PIN" ]] || fail "Metamod pin drift: HEAD=$mms_head expected $MMS_PIN"
[[ "$khook_head" == "$KHOOK_PIN" ]] || fail "nested KHook pin drift: HEAD=$khook_head expected $KHOOK_PIN"

if [[ -n "$(git -C "$MMS_SUB" status --porcelain)" ]]; then
  fail "Metamod submodule is dirty; refuse to rewrite the developer's checkout"
fi
if [[ -n "$(git -C "$MMS_SUB/third_party/khook" status --porcelain)" ]]; then
  fail "nested KHook checkout is dirty; refuse to rewrite the developer's checkout"
fi
log "pins ok metamod=$MMS_PIN khook=$KHOOK_PIN"

# Isolated copy. Do not touch third_party/metamod-source.
rm -rf "$ISOLATED"
mkdir -p "$ISOLATED"
cp -a "$MMS_SUB"/. "$ISOLATED"/
rm -rf "$ISOLATED/.git"
# PLAPI from the checked source, never from a filename.
PLAPI="$(python3 - "$ISOLATED" <<'PY'
from pathlib import Path
import re, sys
text = Path(sys.argv[1], "core/ISmmPluginExt.h").read_text(encoding="utf-8")
m = re.search(r"#define\s+METAMOD_PLAPI_VERSION\s+(\d+)", text)
if not m:
    raise SystemExit("error: METAMOD_PLAPI_VERSION not found in checked source")
print(m.group(1))
PY
)"
[[ "$PLAPI" == "$EXPECTED_PLAPI" ]] || fail "PLAPI from source is $PLAPI, expected $EXPECTED_PLAPI"
log "plapi=$PLAPI (from core/ISmmPluginExt.h)"

if [[ "$PREPARE_ONLY" == "1" ]]; then
  log "prepared source only; no runtime artifact or success manifest generated"
  exit 0
fi

# Toolchain. Named failures; never plant a fake tree.
missing=()
if ! command -v python3 >/dev/null 2>&1; then
  missing+=("python3 not found")
fi
AMBUILD_OK=0
if command -v ambuild >/dev/null 2>&1; then
  AMBUILD_OK=1
elif python3 -c "from ambuild2 import run" >/dev/null 2>&1; then
  AMBUILD_OK=1
fi
if [[ "$AMBUILD_OK" -ne 1 ]]; then
  missing+=("AMBuild not found (need AMBuild 2.2+; pip install ambuild2)")
fi
HL2SDK_PATH="${HL2SDKCS2:-$S2_REPO/third_party/hl2sdk}"
if [[ ! -d "$HL2SDK_PATH/public" ]]; then
  missing+=("hl2sdk missing at $HL2SDK_PATH (HL2SDKCS2 or third_party/hl2sdk with public/ headers)")
fi
if [[ ${#missing[@]} -gt 0 ]]; then
  fail "$(printf '%s; ' "${missing[@]}")isolated unmodified source is at $ISOLATED"
fi
log "hl2sdk=$HL2SDK_PATH"

# Sniper: Steam Runtime 3 / Debian bullseye, GLIBC <= 2.31.
if [[ -f /etc/os-release ]]; then
  # shellcheck disable=SC1091
  . /etc/os-release
  log "os=$PRETTY_NAME"
fi
if [[ "${S2_SNIPER_ENV:-}" != "1" && "${VERSION_CODENAME:-}" != "bullseye" ]]; then
  fail "sniper environment required (Steam Runtime 3 / Debian bullseye, GLIBC <= $GLIBC_MAX). Host compiler output is not evidence."
fi

if ! command -v ambuild >/dev/null 2>&1; then
  fail "AMBuild not found on PATH after import check"
fi
log "ambuild=$(command -v ambuild)"
log "ambuild_api=$(python3 -c 'from ambuild2 import run; print(run.CURRENT_API)')"
log "compiler=$(${CC:-cc} --version 2>/dev/null | head -1 || echo missing)"
log "cxx=$(${CXX:-c++} --version 2>/dev/null | head -1 || echo missing)"

BUILD_DIR="$OUT_ROOT/ambuild"
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"
MMS_STOCK_SOURCE="$ISOLATED"
export MMS_STOCK_SOURCE S2_REPO HL2SDKCS2="$HL2SDK_PATH"

log "configure: HL2SDKCS2=$HL2SDK_PATH python3 $MMS_STOCK_SOURCE/configure.py --sdks=cs2 --targets=x86_64 --enable-optimize --disable-auto-versioning"
(
  cd "$BUILD_DIR"
  # This isolated source copy has no fabricated commit history.
  # The manifest records the checked, unchanged upstream commits.
  HL2SDKCS2="$HL2SDK_PATH" python3 "$MMS_STOCK_SOURCE/configure.py" --sdks=cs2 --targets=x86_64 --enable-optimize --disable-auto-versioning
  ambuild
) >>"$BUILD_LOG" 2>&1

# Stage the complete loader/runtime layout (PackageScript output).
rm -rf "$TREE"
mkdir -p "$TREE"
PACKAGE_ROOT=""
for cand in \
  "$BUILD_DIR/package/addons/metamod" \
  "$BUILD_DIR/package/addons/metamod" \
  "$ISOLATED/package/addons/metamod"
do
  if [[ -d "$cand" ]]; then
    PACKAGE_ROOT="$cand"
    break
  fi
done
# AMBuild 2.2 package folder is typically <build>/package/addons/metamod
if [[ -z "$PACKAGE_ROOT" ]]; then
  PACKAGE_ROOT="$(find "$BUILD_DIR" -type d -name metamod -path '*/package/addons/metamod' 2>/dev/null | head -1 || true)"
fi
[[ -n "$PACKAGE_ROOT" && -d "$PACKAGE_ROOT" ]] || fail "AMBuild package layout missing (expected package/addons/metamod)"
cp -a "$PACKAGE_ROOT"/. "$TREE"/

SO_REL="bin/linuxsteamrt64/metamod.2.cs2.so"
LOADER_REL="bin/linuxsteamrt64/libserver.so"
[[ -f "$TREE/$SO_REL" ]] || fail "required loader file missing: $SO_REL"
[[ -f "$TREE/$LOADER_REL" ]] || fail "required loader file missing: $LOADER_REL"

sha256_file() { sha256sum "$1" | awk '{print $1}'; }

validate_elf() {
  local f="$1"
  command -v readelf >/dev/null 2>&1 || fail "readelf not found (need binutils to validate ELF)"
  readelf -h "$f" | grep -q "ELF64" || fail "$f is not ELF64"
  readelf -h "$f" | grep -q "X86-64\|x86-64" || fail "$f is not x86_64"
  readelf -h "$f" | grep -q "DYN" || fail "$f is not a shared object"
  local glibc
  glibc="$(readelf -W -s "$f" 2>/dev/null | grep -oE 'GLIBC_[0-9]+\.[0-9]+' | sort -V | tail -1 || true)"
  if [[ -n "$glibc" ]]; then
    python3 - "$glibc" "$GLIBC_MAX" "$f" <<'PY'
import sys
def parse(v):
    return tuple(int(x) for x in v.split("."))
need = sys.argv[1].replace("GLIBC_", "")
limit = sys.argv[2]
if parse(need) > parse(limit):
    raise SystemExit(f"error: {sys.argv[3]} requires {sys.argv[1]} > GLIBC {limit}")
PY
  fi
  log "validated $f glibc_need=${glibc:-none}"
}

validate_elf "$TREE/$SO_REL"
validate_elf "$TREE/$LOADER_REL"

# Required artifacts: plugin + loader + packaged support files that exist.
ARTIFACTS_JSON="$(python3 - "$TREE" "$SO_REL" "$LOADER_REL" <<'PY'
import hashlib, json, os, sys
tree = sys.argv[1]
required = [sys.argv[2], sys.argv[3]]
optional = ["metaplugins.ini", "README.txt"]
items = []
seen = set()
for rel in required + optional:
    path = os.path.join(tree, rel)
    if not os.path.isfile(path):
        if rel in required:
            raise SystemExit(f"error: required artifact missing: {rel}")
        continue
    if rel in seen:
        continue
    seen.add(rel)
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            h.update(chunk)
    items.append({"path": rel, "sha256": h.hexdigest()})
print(json.dumps(items))
PY
)"

# Manifest last, after successful build/verification. Real digests only.
python3 - "$MANIFEST.tmp" "$PLAPI" "$MMS_PIN" "$KHOOK_PIN" "$TARGET" "$GLIBC_MAX" "$ARTIFACTS_JSON" <<'PY'
import json, sys
path = sys.argv[1]
doc = {
    "schema": 2,
    "plapi": int(sys.argv[2]),
    "provenance": {"kind": "unmodified-source", "metamod_commit": sys.argv[3], "khook_commit": sys.argv[4]},
    "target": sys.argv[5],
    "glibc_max": sys.argv[6],
    "artifacts": json.loads(sys.argv[7]),
}
with open(path, "w", encoding="utf-8") as fh:
    json.dump(doc, fh, indent=2)
    fh.write("\n")
PY

python3 "$REPO/scripts/verify-metamod-artifact.py" --tree "$TREE" --manifest "$MANIFEST.tmp"
mv "$MANIFEST.tmp" "$MANIFEST"

log "wrote $MANIFEST"
log "DONE"
echo "pinned Metamod build ok: $MANIFEST"
