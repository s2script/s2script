#!/usr/bin/env bash
# Cross-builds the Windows x64 runtime from Linux and packages it as dist/addons (bin/win64/).
#
#   core: cargo xwin build --release --target x86_64-pc-windows-msvc  → s2script_core.dll (+ .dll.lib)
#   shim: cmake with shim/cmake/windows-x64-clang-cl.cmake (clang-cl + lld-link) → s2script.dll
#
# Needs: rustup target x86_64-pc-windows-msvc, cargo-xwin, clang/clang-cl/lld-link/llvm-lib (18 or
# 19), cmake, and llvm-readobj for the import checks. The MSVC CRT and Windows SDK come from xwin,
# which cargo-xwin splats into ~/.cache/cargo-xwin/xwin on first use (XWIN_DIR overrides).
#
#   scripts/build-windows.sh            # build core + shim, check imports, package
#   scripts/build-windows.sh --no-package
set -euo pipefail
cd "$(dirname "$0")/.."

PACKAGE=1
[[ "${1:-}" == --no-package ]] && PACKAGE=0

TARGET=x86_64-pc-windows-msvc
JOBS="${S2_BUILD_JOBS:-$(nproc)}"
export XWIN_DIR="${XWIN_DIR:-$HOME/.cache/cargo-xwin/xwin}"

pick() { for c in "$@"; do command -v "$c" >/dev/null 2>&1 && { echo "$c"; return; }; done; }
READOBJ="$(pick llvm-readobj-19 llvm-readobj-18 llvm-readobj)"
[[ -n "$READOBJ" ]] || { echo "error: llvm-readobj not found" >&2; exit 1; }
command -v cargo-xwin >/dev/null 2>&1 || { echo "error: cargo-xwin not found (cargo install --locked cargo-xwin)" >&2; exit 1; }

# cargo-xwin drives clang-cl by its plain name; on a distro that only ships versioned binaries,
# put unversioned links on PATH for this run.
if ! command -v clang-cl >/dev/null 2>&1; then
  shimbin="$PWD/build/win-toolbin"
  mkdir -p "$shimbin"
  for tool in clang-cl lld-link llvm-lib clang; do
    real="$(pick "$tool-19" "$tool-18")"
    [[ -n "$real" ]] || { echo "error: $tool (18 or 19) not found" >&2; exit 1; }
    ln -sf "$(command -v "$real")" "$shimbin/$tool"
  done
  export PATH="$shimbin:$PATH"
fi

rustup target list --installed | grep -qx "$TARGET" || rustup target add "$TARGET"

echo "== core: cargo xwin build --release ($TARGET) =="
cargo xwin build --release --locked -p s2script-core --target "$TARGET"
CORE_DIR="target/$TARGET/release"
[[ -f "$CORE_DIR/s2script_core.dll" && -f "$CORE_DIR/s2script_core.dll.lib" ]] || {
  echo "error: $CORE_DIR has no s2script_core.dll(.lib)" >&2; exit 1; }

echo "== shim: cmake (clang-cl toolchain) =="
cmake -S shim -B build/shim-win64 \
  -DCMAKE_TOOLCHAIN_FILE="$PWD/shim/cmake/windows-x64-clang-cl.cmake" \
  -DCMAKE_BUILD_TYPE=Release \
  -DXWIN_DIR="$XWIN_DIR" \
  -DS2_CORE_WIN_DIR="$PWD/$CORE_DIR"
cmake --build build/shim-win64 -j"$JOBS"

# The server has no MSVC redistributable to lean on, and Metamod loads the shim by path with no
# search of the addon directory. So: core may import only system DLLs, and the shim may import
# only system DLLs and tier0 (from the game's bin), with core strictly delay-loaded.
echo "== import checks =="
dll_imports() { "$READOBJ" --coff-imports "$1" | awk '/^Import \{/{m="import"} /^DelayImport \{/{m="delay"} m && /Name: .*\.dll/{print m, tolower($2); m=""}' | sort -u; }
system_dll='^(kernel32|advapi32|winmm|bcrypt|bcryptprimitives|combase|dbghelp|ntdll|shell32|ws2_32|user32|ole32|oleaut32|psapi|api-ms-win-core-[a-z0-9-]+)\.dll$'
fail=0
while read -r kind name; do
  if [[ ! "$name" =~ $system_dll ]]; then echo "FAIL s2script_core.dll imports $name ($kind)"; fail=1; fi
done < <(dll_imports "$CORE_DIR/s2script_core.dll")
while read -r kind name; do
  case "$kind:$name" in
    delay:s2script_core.dll|import:tier0.dll) ;;
    import:*) [[ "$name" =~ $system_dll ]] || { echo "FAIL s2script.dll imports $name"; fail=1; } ;;
    *) echo "FAIL s2script.dll $kind-imports $name"; fail=1 ;;
  esac
done < <(dll_imports build/shim-win64/s2script.dll)
dll_imports build/shim-win64/s2script.dll | grep -qx "delay s2script_core.dll" || {
  echo "FAIL s2script.dll does not delay-load s2script_core.dll"; fail=1; }
"$READOBJ" --coff-exports build/shim-win64/s2script.dll | grep -q "Name: CreateInterface$" || {
  echo "FAIL s2script.dll does not export CreateInterface"; fail=1; }
[[ $fail == 0 ]] || exit 1
echo "ok: s2script_core.dll imports only system DLLs; s2script.dll imports system DLLs + tier0 and delay-loads core"

if [[ $PACKAGE == 1 ]]; then
  S2_PLATFORM=win64 S2_CORE_FILE="$CORE_DIR/s2script_core.dll" S2_SHIM_FILE=build/shim-win64/s2script.dll \
    S2_SHIM_BUILD_DIR=build/shim-win64 bash scripts/package-addon.sh
fi
