#!/usr/bin/env bash
# Isolated, reproducible build of the pinned Metamod host + KHook retirement patch.
#
# Never rewrites the developer's submodule checkout. Prepares a fresh tree at the
# exact SHAs, applies patches/metamod-source/series, then AMBuilds in the sniper
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

MMS_PIN="7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33"
KHOOK_PIN="1e200e4cc8e0badcb7cf941525268d6977f6a4e6"
EXPECTED_PLAPI=18
TARGET="linux-x86_64"
GLIBC_MAX="2.31"

MMS_SUB="$S2_REPO/third_party/metamod-source"
PATCH_DIR="$S2_REPO/patches/metamod-source"
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
[[ -f "$PATCH_DIR/series" ]] || fail "patches/metamod-source/series missing"

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
# Without its own repository, git apply discovers the outer s2script checkout
# and silently skips patch paths outside this build subdirectory.
git init --quiet "$ISOLATED"

python3 - "$PATCH_DIR" "$ISOLATED" "$OUT_ROOT/patchset.sha256" <<'PY'
import hashlib, pathlib, subprocess, sys

patch_dir = pathlib.Path(sys.argv[1])
mms = pathlib.Path(sys.argv[2])
out = pathlib.Path(sys.argv[3])
series_path = patch_dir / "series"
if not series_path.is_file():
    raise SystemExit("error: series file missing")

seen = set()
entries = []
for raw in series_path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line or line.startswith("#"):
        continue
    if line in seen:
        raise SystemExit(f"error: duplicate series entry {line}")
    seen.add(line)
    rel = pathlib.Path(line)
    if rel.is_absolute() or ".." in rel.parts:
        raise SystemExit(f"error: series path escapes patch directory: {line}")
    full = patch_dir / line
    if not full.is_file():
        raise SystemExit(f"error: series entry missing: {full}")
    entries.append((line, full))

digest = hashlib.sha256()
for name, full in entries:
    digest.update(name.encode("utf-8"))
    digest.update(b"\0")
    digest.update(full.read_bytes())
    digest.update(b"\0")
    check = subprocess.run(
        ["git", "apply", "--check", str(full)],
        cwd=mms,
        capture_output=True,
        text=True,
    )
    if check.returncode != 0:
        sys.stderr.write(check.stderr)
        raise SystemExit(f"error: git apply --check failed for {name}")
    subprocess.check_call(["git", "apply", str(full)], cwd=mms)

out.write_text(digest.hexdigest() + "\n", encoding="utf-8")
print(digest.hexdigest())
PY
PATCHSET_SHA256="$(tr -d '[:space:]' <"$OUT_ROOT/patchset.sha256")"
[[ "${#PATCHSET_SHA256}" -eq 64 ]] || fail "patchset_sha256 is not 64 hex characters"
log "patchset_sha256=$PATCHSET_SHA256"

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

# Second isolated identity: re-hash the same series against a second copy.
python3 - "$PATCH_DIR" "$PATCHSET_SHA256" <<'PY'
import hashlib, pathlib, sys
patch_dir = pathlib.Path(sys.argv[1])
expected = sys.argv[2]
digest = hashlib.sha256()
seen = set()
for raw in (patch_dir / "series").read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line or line.startswith("#"):
        continue
    if line in seen:
        raise SystemExit(f"error: duplicate series entry {line}")
    seen.add(line)
    rel = pathlib.Path(line)
    if rel.is_absolute() or ".." in rel.parts:
        raise SystemExit(f"error: series path escapes patch directory: {line}")
    full = patch_dir / line
    digest.update(line.encode("utf-8"))
    digest.update(b"\0")
    digest.update(full.read_bytes())
    digest.update(b"\0")
got = digest.hexdigest()
if got != expected:
    raise SystemExit(f"error: second isolated patchset identity drifted: {got} != {expected}")
print("second isolated patchset identity matches")
PY
log "second isolated source/patch identity matches"

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
  fail "$(printf '%s; ' "${missing[@]}")isolated patched source is at $ISOLATED"
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
MMS_PATCHED_SOURCE="$ISOLATED"
export MMS_PATCHED_SOURCE S2_REPO HL2SDKCS2="$HL2SDK_PATH"

log "configure: HL2SDKCS2=$HL2SDK_PATH python3 $MMS_PATCHED_SOURCE/configure.py --sdks=cs2 --targets=x86_64 --enable-optimize --disable-auto-versioning"
(
  cd "$BUILD_DIR"
  # This isolated repository intentionally has no fabricated commit history.
  # The manifest carries the checked upstream commits and ordered patch digest.
  HL2SDKCS2="$HL2SDK_PATH" python3 "$MMS_PATCHED_SOURCE/configure.py" --sdks=cs2 --targets=x86_64 --enable-optimize --disable-auto-versioning
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
python3 - "$MANIFEST.tmp" "$PLAPI" "$MMS_PIN" "$KHOOK_PIN" "$PATCHSET_SHA256" "$TARGET" "$GLIBC_MAX" "$ARTIFACTS_JSON" <<'PY'
import json, sys
path = sys.argv[1]
doc = {
    "schema": 1,
    "plapi": int(sys.argv[2]),
    "metamod_commit": sys.argv[3],
    "khook_commit": sys.argv[4],
    "patchset_sha256": sys.argv[5],
    "target": sys.argv[6],
    "glibc_max": sys.argv[7],
    "artifacts": json.loads(sys.argv[8]),
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
