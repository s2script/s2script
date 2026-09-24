#!/usr/bin/env python3
"""Check the produced standalone target ELF, not source-level visibility flags."""
import pathlib
import re
import subprocess
import sys

fixture = pathlib.Path(sys.argv[1]).resolve(strict=True)
dynamic = subprocess.check_output(["readelf", "-dW", str(fixture)], text=True)
exports = subprocess.check_output(["nm", "-D", "--defined-only", str(fixture)], text=True)
symbols = subprocess.check_output(["nm", "-anC", "--defined-only", str(fixture)], text=True)
allowed = {
    "s2fn_member_fixture_target", "s2fn_fixture_targets",
    "s2fn_fixture_set_original_calls", "s2fn_fixture_peer_calls",
}
exported = {line.split()[-1] for line in exports.splitlines() if line.strip()}
if exported != allowed:
    raise SystemExit(f"FAIL fixture exports: unexpected={exported - allowed}, missing={allowed - exported}")
dependencies = re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic)
for dependency in dependencies:
    if not re.fullmatch(r"lib(?:c|m|stdc\+\+|gcc_s)\.so(?:\.\d+)*|ld-linux-x86-64\.so\.2", dependency):
        raise SystemExit(f"FAIL fixture implementation dependency: {dependency}")
# A statically linked provider/runtime would evade DT_NEEDED. Only the existing
# header-only member-pointer extraction helper from KHook is permitted here.
for line in symbols.splitlines():
    parts = line.split(maxsplit=2)
    if len(parts) != 3 or parts[1] not in "tTwW":
        continue
    symbol = parts[2]
    if ("safetyhook::" in symbol or "s2fn::" in symbol or re.search(r"\bffi_\w+", symbol)
            or ("KHook::" in symbol and "KHook::ExtractMFP<" not in symbol)):
        raise SystemExit(f"FAIL fixture contains implementation code: {symbol}")
print(f"fixture-elf={fixture}")
print("fixture-needed=" + ",".join(dependencies))
print(exports, end="")
print("PASS fixture ELF has only four accessors and no provider/SafetyHook/libffi/runtime implementation")
