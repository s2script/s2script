#!/usr/bin/env bash
# Fails (exit 1) if the plugin-declared-call invoke thunk's float ABI is wrong.
#
# WHY THIS GATE EXISTS. shim/src/engine_calls.cpp calls every declared engine function through ONE
# fixed max-arity prototype (5 GP + 8 xmm, all slots always passed). The arg vocabulary's `float` is
# 32-bit, and SysV passes a 32-bit float in the LOW 32 bits of an xmm register. If those slots are
# declared `double`, the register holds a 64-bit double bit pattern and a callee doing `movss` reads
# the double's low word — `(double)10.0` is 0x4024000000000000, low 32 bits ZERO, so the callee sees
# 0.0f. No crash, no diagnostic, just a silently wrong argument: the exact misbehaviour class the
# plugin-gamedata slice exists to prevent. The original implementation shipped `double` here (it came
# from the plan), and nothing caught it until an adversarial review compiled a repro.
#
# This gate reproduces that repro standalone: it needs no hl2sdk and no game binary, because the only
# thing under test is the calling convention. Caller and callee are separate translation units at -O0
# so the compiler cannot see through the cast and "fix" it.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="shim/src/engine_calls.cpp"
[ -f "$SRC" ] || { echo "check-invoke-abi: missing $SRC" >&2; exit 1; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# --- Guard 1: the prototypes in the real source must use float, not double, for the xmm slots.
# Extract the FnU64/FnF32 aliases and assert no `double` appears in them.
if awk '/^using Fn(U64|F32) = /,/;/' "$SRC" | grep -q 'double'; then
  echo "check-invoke-abi: FAIL — FnU64/FnF32 declare xmm slots as 'double'." >&2
  echo "  The vocabulary's 'float' is 32-bit; a double bit pattern makes the callee read 0.0f." >&2
  echo "  See the note above these aliases in $SRC." >&2
  exit 1
fi
if ! awk '/^using FnU64 = /,/;/' "$SRC" | grep -q 'float'; then
  echo "check-invoke-abi: FAIL — FnU64 has no float xmm slots; did the prototype change shape?" >&2
  exit 1
fi

# --- Guard 2: behavioural. Prove a float argument survives the fixed-prototype cast.
cat >"$TMP/callee.cpp" <<'EOF'
#include <cstdint>
// A callee with the shape of a real engine method: (this, float, int, void*, float).
// CBaseEntity::Ignite on CS2 build 2000877 has exactly this register contract.
extern "C" void Callee(void* self, float lifetime, int flags, void* attacker, float size,
                       float* outLifetime, int* outFlags, float* outSize) {
    (void)self; (void)attacker;
    *outLifetime = lifetime;
    *outFlags    = flags;
    *outSize     = size;
}
EOF

cat >"$TMP/caller.cpp" <<'EOF'
#include <cstdint>
#include <cstdio>
extern "C" void Callee(void*, float, int, void*, float, float*, int*, float*);

// The prototype shape shim/src/engine_calls.cpp uses. xmm slots are float ON PURPOSE.
using FnU64 = uint64_t (*)(void*, uint64_t, uint64_t, uint64_t, uint64_t, uint64_t,
                           float, float, float, float, float, float, float, float);

int main() {
    float  gotLifetime = -1.0f, gotSize = -1.0f;
    int    gotFlags    = -1;
    double fp[8] = { 10.0, 0.0, 0, 0, 0, 0, 0, 0 };   // core hands us f64 (JS numbers are doubles)
    float  f[8]  = { 0, 0, 0, 0, 0, 0, 0, 0 };
    for (int i = 0; i < 8; i++) f[i] = static_cast<float>(fp[i]);   // the narrow under test

    uint64_t g[5] = { 0, 0, 0, 0, 0 };
    g[0] = 4;                                                       // flags (int, GP)
    g[1] = 0;                                                       // attacker (ptr, GP)
    g[2] = reinterpret_cast<uint64_t>(&gotLifetime);
    g[3] = reinterpret_cast<uint64_t>(&gotFlags);
    g[4] = reinterpret_cast<uint64_t>(&gotSize);

    volatile FnU64 fn = reinterpret_cast<FnU64>(&Callee);
    fn(nullptr, g[0], g[1], g[2], g[3], g[4], f[0], f[1], f[2], f[3], f[4], f[5], f[6], f[7]);

    if (gotLifetime != 10.0f || gotFlags != 4) {
        std::printf("ABI MISMATCH: lifetime=%g (want 10) flags=%d (want 4)\n",
                    static_cast<double>(gotLifetime), gotFlags);
        return 1;
    }
    std::printf("ok: lifetime=%g flags=%d\n", static_cast<double>(gotLifetime), gotFlags);
    return 0;
}
EOF

CXX="${CXX:-g++}"
"$CXX" -O0 -std=c++17 -c "$TMP/callee.cpp" -o "$TMP/callee.o"
"$CXX" -O0 -std=c++17 -c "$TMP/caller.cpp" -o "$TMP/caller.o"
"$CXX" "$TMP/caller.o" "$TMP/callee.o" -o "$TMP/abitest"

if ! out="$("$TMP/abitest")"; then
  echo "check-invoke-abi: FAIL — float argument did not survive the fixed-prototype cast:" >&2
  echo "  $out" >&2
  exit 1
fi

echo "check-invoke-abi: ok ($out)"
