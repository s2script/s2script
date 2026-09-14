#!/usr/bin/env bash
# Compile + run the engine-free host KHook provider/Unloader lifetime harness.
#
#   bash scripts/test-khook-host-lifetime.sh --baseline
#       Compiles the real unpatched provider/unloader from the pinned submodule.
#
#   bash scripts/test-khook-host-lifetime.sh
#       Applies patches/metamod-source/series to an isolated copy of that pin
#       and compiles the patched provider/unloader. Never rewrites the
#       developer's submodule checkout.
#
# ASan/UBSan are not decoration: provider UAF during POST/removal/dlclose is
# the F1 shape. Missing sanitizer runtime is reported, not treated as success.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$(pwd)"

MMS_PIN="7e24ce9e7a03bfeb5c8ab1e4dd55d5d5747f3d33"
KHOOK_PIN="1e200e4cc8e0badcb7cf941525268d6977f6a4e6"
MMS_SUB="$REPO/third_party/metamod-source"
PATCH_DIR="$REPO/patches/metamod-source"

MODE="patched"
if [[ "${1:-}" == "--baseline" ]]; then
  MODE="baseline"
  shift
fi

CASES=(
  provider_during_post
  provider_during_remove
  provider_during_dlclose
  inline_and_virtual
  explicit_then_unload
  duplicate_remove
  pending_insert
  unknown_id
  reentrant_completion
  release_once
)

if [[ ! -d "$MMS_SUB/core" ]]; then
  echo "error: pinned Metamod source missing at $MMS_SUB" >&2
  exit 1
fi

mms_head="$(git -C "$MMS_SUB" rev-parse HEAD)"
khook_head="$(git -C "$MMS_SUB/third_party/khook" rev-parse HEAD)"
if [[ "$mms_head" != "$MMS_PIN" ]]; then
  echo "error: Metamod pin drift: HEAD=$mms_head expected $MMS_PIN" >&2
  exit 1
fi
if [[ "$khook_head" != "$KHOOK_PIN" ]]; then
  echo "error: nested KHook pin drift: HEAD=$khook_head expected $KHOOK_PIN" >&2
  exit 1
fi

WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT
MMS_SRC="$WORKDIR/mms"

# Isolated tree: never mutate third_party/metamod-source.
mkdir -p "$MMS_SRC"
cp -a "$MMS_SUB"/. "$MMS_SRC"/
rm -rf "$MMS_SRC/.git"

if [[ "$MODE" == "patched" ]]; then
  if [[ ! -f "$PATCH_DIR/series" ]]; then
    echo "error: patches/metamod-source/series missing (patched host required)" >&2
    exit 1
  fi
  python3 - "$PATCH_DIR" "$MMS_SRC" <<'PY'
import pathlib, subprocess, sys
patch_dir = pathlib.Path(sys.argv[1])
mms = pathlib.Path(sys.argv[2])
series = (patch_dir / "series").read_text(encoding="utf-8").splitlines()
seen = set()
entries = []
for raw in series:
    line = raw.strip()
    if not line or line.startswith("#"):
        continue
    if line in seen:
        raise SystemExit(f"error: duplicate series entry {line}")
    seen.add(line)
    p = pathlib.Path(line)
    if p.is_absolute() or ".." in p.parts:
        raise SystemExit(f"error: series path escapes patch directory: {line}")
    full = patch_dir / line
    if not full.is_file():
        raise SystemExit(f"error: series entry missing: {full}")
    entries.append(full)
for patch in entries:
    check = subprocess.run(
        ["git", "apply", "--check", str(patch)],
        cwd=mms,
        capture_output=True,
        text=True,
    )
    if check.returncode != 0:
        sys.stderr.write(check.stderr)
        raise SystemExit(f"error: git apply --check failed for {patch.name}")
    subprocess.check_call(["git", "apply", str(patch)], cwd=mms)
print(f"applied {len(entries)} patch(es) to isolated Metamod tree")
PY
  DEFINES=(-DS2_KHOOK_HOST_PATCHED)
  echo "   (patched isolated tree)"
else
  python3 - "$MMS_SRC/core/metamod_plugins.cpp" "$WORKDIR/unpatched_unloader.h" <<'PY'
from pathlib import Path
import sys
src = Path(sys.argv[1]).read_text(encoding="utf-8")
start = src.find("struct Unloader {")
if start < 0:
    raise SystemExit("error: struct Unloader not found in metamod_plugins.cpp")
end = src.find("#define ITER_PLEVENT", start)
if end < 0:
    raise SystemExit("error: Unloader terminator not found in metamod_plugins.cpp")
body = src[start:end].rstrip() + "\n"
header = """\
/* Generated from the actual unpatched metamod_plugins.cpp Unloader. */
#pragma once
#include <set>
#include <mutex>
#include <khook.hpp>
typedef void *HINSTANCE;
extern "C" int dlclose(void *);

"""
Path(sys.argv[2]).write_text(header + body, encoding="utf-8")
PY
  DEFINES=()
  echo "   (unpatched baseline; Unloader extracted from metamod_plugins.cpp)"
fi

out="$WORKDIR/khook_host_lifetime_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra -pthread -DKHOOK_STANDALONE)
flags+=(-Wl,--wrap=dlclose -Wno-delete-non-virtual-dtor)

SAN=0
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  SAN=1
  echo "   (with -fsanitize=address,undefined)"
else
  echo "error: sanitizer runtime missing; host lifetime tests require ASan/UBSan" >&2
  exit 1
fi

INCLUDE=(
  -I "$WORKDIR"
  -I "$MMS_SRC/core"
  -isystem "$MMS_SRC/third_party/khook/include"
)

g++ "${flags[@]}" "${DEFINES[@]}" "${INCLUDE[@]}" \
  -o "$out" shim/tests/khook_host_lifetime_test.cpp

export ASAN_OPTIONS="abort_on_error=1:detect_leaks=1"
export UBSAN_OPTIONS="halt_on_error=1:print_stacktrace=1"

run_case() {
  local name="$1"
  local log="$WORKDIR/${name}.log"
  local status=0
  set +e
  timeout --signal=KILL 12s "$out" "$name" >"$log" 2>&1
  status=$?
  set -e
  echo "----- $name (exit $status) -----"
  cat "$log"
  return "$status"
}

if [[ "$MODE" == "baseline" ]]; then
  echo "==> baseline: real unpatched provider (expect provider lifetime/tracking failures)"
  failed=0
  observed=0
  for case in "${CASES[@]}"; do
    if run_case "$case"; then
      echo "baseline $case: unexpectedly passed"
    else
      echo "baseline $case: failed as recorded"
      observed=$((observed + 1))
      failed=1
    fi
  done
  echo "==> baseline summary: $observed/${#CASES[@]} named cases failed or aborted"
  if [[ "$failed" -eq 0 ]]; then
    echo "error: baseline produced no failures; harness is not testing unpatched lifetime bugs" >&2
    exit 1
  fi
  # Non-zero: TDD RED. Callers record this output.
  exit 1
fi

echo "==> patched host provider/unloader"
"$out"
echo "==> patched host lifetime: PASS (sanitizers=$SAN)"
