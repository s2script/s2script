#!/usr/bin/env bash
# Fails if a gamedata byte signature bakes in a BUILD-SPECIFIC operand.
#
# WHY THIS GATE EXISTS. A rip-relative displacement (`lea reg,[rip+disp32]`) or a rel32 branch
# operand (`call`/`jmp`) encodes a DISTANCE between two addresses in one particular build. It shifts
# on any recompile even when the function itself is unchanged, so a pattern containing one cannot
# survive a single CS2 update.
#
# That is not hypothetical. `SetModel` shipped with `48 8D 05 3D ED 28 01` baked in to break a tie
# between two identical candidates. It went stale, resolved to nothing, and `EntityRef.setModel`
# became a SILENT no-op — corpses spawned invisible in a downstream plugin. The signature was doing
# its job (it reported "NOT FOUND — regenerate"); the pattern was simply unshippable.
#
# Wildcard the operand instead, and break ties structurally (surrounding opcodes, padding, a
# neighbouring function's prologue) rather than numerically.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - "$@" <<'PY'
import json, re, pathlib, sys

paths = sorted(pathlib.Path("gamedata").rglob("*.jsonc"))
sigs = {}
for path in paths:
    raw = re.sub(r'^\s*//.*$', '', path.read_text(), flags=re.M)
    sigs.update(json.loads(raw).get("signatures", {}))

REL = {"E8": "call rel32", "E9": "jmp rel32"}
bad = []
for name, plats in sigs.items():
    for plat, spec in plats.items():
        toks = spec.get("pattern", "").split()
        for i, t in enumerate(toks):
            T, kind = t.upper(), None
            if T in REL:
                kind, operand = REL[T], toks[i+1:i+5]
            elif T == "48" and i + 2 < len(toks) and toks[i+1].upper() == "8D":
                m = toks[i+2].upper()
                # modrm mod=00 rm=101 => [rip+disp32]
                if m != "?" and (int(m, 16) & 0xC7) == 0x05:
                    kind, operand = "lea rip-relative disp32", toks[i+3:i+7]
            if kind and len(operand) == 4 and all(o != "?" for o in operand):
                bad.append((name, plat, kind, " ".join(operand)))

if bad:
    print("check-gamedata-sigs: FAIL — build-specific operand baked into a signature.", file=sys.stderr)
    print("  These shift on ANY recompile; the signature cannot survive a CS2 update:", file=sys.stderr)
    for n, p, k, o in bad:
        print(f"    {n} [{p}]: {k}, operand {o}", file=sys.stderr)
    print("\n  Wildcard the operand ('? ? ? ?') and break any tie structurally instead.", file=sys.stderr)
    raise SystemExit(1)

print(f"check-gamedata-sigs: ok ({len(sigs)} signatures, no build-specific operands)")
PY
