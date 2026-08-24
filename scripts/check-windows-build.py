#!/usr/bin/env python3
"""Static contract checks for the native Windows build.

The Windows runner remains the authoritative MSVC compile gate.  This cheap host-neutral check
keeps the easy-to-regress parts (CRT, source-built protobuf, narrow SDK linking, DLL pinning,
optional exports, and CI wiring) visible to the Linux native suite too.
"""

from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parent.parent
failures: list[str] = []


def require(path: str, needle: str, why: str) -> None:
    file = ROOT / path
    if not file.is_file():
        failures.append(f"{path}: missing file required for {why}")
        return
    text = file.read_text(encoding="utf-8")
    if needle not in text:
        failures.append(f"{path}: missing {why} ({needle!r})")


def forbid(path: str, needle: str, why: str) -> None:
    file = ROOT / path
    if not file.is_file():
        failures.append(f"{path}: missing file required for {why}")
        return
    if needle in file.read_text(encoding="utf-8"):
        failures.append(f"{path}: forbidden {why} ({needle!r})")


require("core/build.rs", 'CARGO_CFG_TARGET_OS', "target-aware Linux-only ELF nodelete guard")
require("core/src/ffi.rs", "GET_MODULE_HANDLE_EX_FLAG_PIN", "Windows core-DLL lifetime pin")
require("shim/CMakeLists.txt", "MSVC_RUNTIME_LIBRARY", "static MSVC runtime selection")
require(
    "shim/CMakeLists.txt",
    "thirdparty/protobuf-3.21.8",
    "vendored protobuf 3.21.8 source build",
)
require("shim/CMakeLists.txt", "protobuf_MSVC_STATIC_RUNTIME ON", "protobuf static MSVC runtime")
require("shim/CMakeLists.txt", "protobuf::libprotobuf", "source-built protobuf link target")
forbid(
    "shim/CMakeLists.txt",
    "lib/public/win64/2015/libprotobuf.lib",
    "VS2015 protobuf archive",
)
for archive in ("mathlib.lib", "tier1.lib", "interfaces.lib"):
    forbid("shim/CMakeLists.txt", archive, f"blanket Windows SDK archive {archive}")
require("shim/CMakeLists.txt", "lib/public/win64/tier0.lib", "required tier0 Windows dependency")
require("shim/CMakeLists.txt", "s2script_core.dll.lib", "Rust core import-library link")
require("shim/CMakeLists.txt", "s2script_core.dll", "Rust core DLL staging")
require(
    "shim/CMakeLists.txt",
    "crash_handler_win_noop.cpp",
    "explicit temporary Windows crash-reporter degradation",
)
require("shim/src/core_symbols_win.cpp", "GetProcAddress", "optional core export lookup")
require(
    "shim/src/core_symbols_win.cpp",
    "s2script_core_dispatch_hook_post",
    "version-tolerant post-hook lookup",
)
require(
    "shim/src/platform/module_win.cpp",
    "case PAGE_EXECUTE:",
    "execute-only Windows memory rejection",
)
require(
    "shim/tests/platform_test.cpp",
    "PAGE_EXECUTE is not accepted as readable",
    "execute-only Windows memory regression test",
)
require(
    "shim/tests/detour_reloc_test.cpp",
    "AllocateExecutableNear",
    "portable near-memory allocation in detour relocation tests",
)
require(
    "shim/tests/detour_reloc_test.cpp",
    "FreeExecutable",
    "portable executable-memory release in detour relocation tests",
)
forbid(
    "shim/tests/detour_reloc_test.cpp",
    "<sys/mman.h>",
    "POSIX-only detour test memory API",
)
require("scripts/ci-native-windows.ps1", "cargo test -p s2script-core", "Windows Rust tests")
require("scripts/ci-native-windows.ps1", "cargo build --locked --release", "release Rust core build")
require("scripts/ci-native-windows.ps1", "S2_CORE_LIB_DIR=release", "release core/shim link")
require("scripts/ci-native-windows.ps1", "target/release/s2script_core.dll", "release core artifact")
forbid("scripts/ci-native-windows.ps1", "target/debug/", "debug Windows core artifact")
forbid("scripts/ci-native-windows.ps1", "S2_CORE_LIB_DIR=debug", "debug Windows core link")
require("scripts/ci-native-windows.ps1", "ctest", "Windows CTest gate")
require(
    ".github/workflows/ci-native.yml",
    "windows-latest",
    "Windows native CI runner",
)
require(".github/workflows/ci-native.yml", 'node-version: "22"', "repository Node.js version")
require(".github/workflows/ci-native.yml", "packages/sdk/**", "SDK test workflow path coverage")
require(".github/workflows/ci-native.yml", "scripts/test-*.sh", "native test-script path coverage")

if failures:
    print("check-windows-build: FAIL", file=sys.stderr)
    for failure in failures:
        print(f"  {failure}", file=sys.stderr)
    raise SystemExit(1)

print("check-windows-build: ok")
