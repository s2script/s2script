#!/usr/bin/env bash
# Compile + run the descriptor-validator unit test with the host compiler (no sniper container, no
# SDK, no game binary — call_validate.cpp's only outside contact is the injected ModuleView + Ops).
#
# WHY THIS GATE EXISTS. `vtable-member` and `string-xref` are the two SEMANTIC gates that turn a
# borrowed signature's unique-but-WRONG match into a named per-descriptor degrade. Every cheaper
# check (uniqueness, .text range) passes that case by construction, so if the rejection is never
# exercised a refactor can quietly turn either validator into a no-op and nothing goes red until a
# live server calls the wrong engine function. The test drives the SHIPPED code over a synthetic
# module image, so both gates are proven to FAIL when they should.
#
# The sanitizers are not decoration either: the validators do uintptr arithmetic on a target that is
# legitimately BELOW the text base (.rodata precedes .text in a real mapping), which is exactly the
# shape a sloppy bounds check reads out of.
set -euo pipefail
cd "$(dirname "$0")/.."

out="$(mktemp -d)/call_validate_test"
flags=(-std=c++17 -O1 -g -Wall -Wextra)

# Probe for the sanitizer runtime rather than assuming it: a missing libasan must not fail the gate.
if echo 'int main(){return 0;}' | g++ -x c++ -fsanitize=address,undefined -o /dev/null - 2>/dev/null; then
  flags+=(-fsanitize=address,undefined -fno-sanitize-recover=all)
  echo "   (with -fsanitize=address,undefined)"
else
  echo "   (no sanitizer runtime)"
fi

# hde64 is linked in now: the arg-width validator decodes the callee's prologue to check a declared
# shape against the machine code (see call_validate.h).
#
# It is compiled SEPARATELY with the alignment check off, rather than passing -fno-sanitize=alignment
# to the whole link. HDE64 reads immediates straight out of instruction bytes at whatever offset the
# encoding puts them — UB by the letter of the standard, architectural on x86-64, and third-party
# code we ship as-is. Blanketing the binary would also switch alignment off for OUR decode
# arithmetic, which is precisely the code this gate exists to police.
hde_o="$(dirname "$out")/hde64.o"
g++ "${flags[@]}" -fno-sanitize=alignment -I third_party/hde -c -o "$hde_o" third_party/hde/hde64.c
g++ "${flags[@]}" -I shim/src -I third_party/hde \
    -o "$out" shim/src/call_validate.cpp shim/src/sigscan.cpp "$hde_o" \
    shim/tests/call_validate_test.cpp
"$out"
