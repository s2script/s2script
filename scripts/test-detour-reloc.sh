#!/usr/bin/env bash
# Compile + run the engine-free detour relocation test with the host compiler (no SDK, no game).
#
# The sanitizers are load-bearing here. This code computes byte offsets INTO instruction encodings
# and writes four bytes at each — the exact shape that walks off the end of a buffer when the offset
# arithmetic is wrong. ASan turns that into a deterministic abort instead of a corrupted trampoline
# that only misbehaves once it is executing on a game server.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/detour_reloc_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  # -fno-sanitize=alignment is NOT a convenience: the vendored HDE64 reads immediates straight out of
  # instruction bytes (`*(uint32_t*)p` at whatever offset the encoding puts them), which is UB by the
  # letter of the standard and completely fine on x86-64, where unaligned loads are architectural.
  # That is third-party code we ship as-is; silencing only `alignment` keeps every other UBSan check
  # live on OUR arithmetic, which is the part that can actually be wrong.
  flags+=(-fsanitize=address,undefined -fno-sanitize=alignment -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined, minus the alignment check — see the comment)"
else
  echo "   (no sanitizer runtime)"
fi

# detour.cpp is linked in too: the end-to-end case installs a real detour on a real function in this
# test binary. It is engine-free (sys/mman and nothing else), so it needs no game to run — and it is
# the only thing here that exercises the patch, the trampoline layout and the jump back.
g++ "${flags[@]}" -I third_party/hde -o "$out" \
    shim/src/detour_reloc.cpp shim/src/detour.cpp third_party/hde/hde64.c \
    shim/tests/detour_reloc_test.cpp
"$out"
