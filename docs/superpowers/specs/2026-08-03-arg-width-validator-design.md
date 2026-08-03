# The arg-width validator — checking a declared shape against the callee's actual machine code

**Status:** Draft — ready for review.
**Audience:** shim maintainers; anyone declaring an inbound hook.
**Builds on:** the closed validator vocabulary (`shim/src/call_validate.{h,cpp}` — `prologue`,
`string-xref`, `vtable-member`); the hook shape vocabulary (`shim/src/hook_dispatch.h`); the
vendored HDE64 length disassembler.
**Motivated by:** the live-server SIGSEGV in
`docs/superpowers/specs/2026-08-02-detour-relocation-design.md`'s gate — `TerminateRound`'s third
argument is a pointer, was declared `int32_t`, and got truncated.

---

## 1. Why

A hook descriptor names a **shape** — the callee's exact C signature — and nothing checks that name
against the function it is about to detour. `check-hook-shapes.sh` proves core and the shim agree
with *each other*; both were confidently wrong together, and the only thing that noticed was a
segfault on a live server.

The failure is asymmetric and that asymmetry is the whole design:

- Declaring a parameter **narrower** than the engine's is a **memory-safety bug**. The thunk reads
  32 bits of a 64-bit register, calls the original with the truncated value zero-extended, the engine
  banks half a pointer, and something dereferences it later — far from the hook, long after.
- Declaring it **wider** is harmless. SysV leaves the upper half of a 32-bit argument undefined, so
  copying the whole register back preserves whatever was actually there.

So there is exactly one direction to police, and it is mechanically detectable.

**On the prior art:** SourceMod does not have this check, and does not need it — its detour signatures
are C++ (`DETOUR_DECL_MEMBER*`), written by a human who read the disassembly. Making the shape *data*
is what removed that human step. This validator buys back the safety the data-driven design gave
away; it is not a place where SM did something cleverer.

## 2. The idea in one line

**A callee tells you how wide its arguments are by how it stores them.** `mov %rdx,-0xe0(%rbp)` is a
64-bit store — only a 64-bit value needs that. `mov %esi,%r15d` is 32-bit. Read the prologue, compare
against the shape, refuse a narrowing mismatch by name.

Concretely, from the bug that motivated this:

```
1384ae8:  movss %xmm0,-0xd8(%rbp)     ; arg0  32-bit  agrees with f32
1384ac6:  mov   %esi,%r15d            ; arg1  32-bit  agrees with i32
1384af0:  mov   %rdx,-0xe0(%rbp)      ; arg2  64-BIT  the shape said i32  ->  REFUSE
```

## 3. Scope: narrow on purpose

**Only opcode `0x89` (`MOV r/m, r`) with a memory destination**, i.e. "the callee stashed an incoming
argument register somewhere". That is the pattern that produced the bug, it is overwhelmingly how a
prologue spills its arguments, and it is unambiguous to decode.

Deliberately NOT attempted:

- Full dataflow. We are not writing a decompiler.
- Inferring *what* an argument is. A pointer and a 64-bit count are indistinguishable, and that is
  fine — **width is what was wrong**.
- Floating-point argument classification. `xmm` slots are a separate register file and a separate
  (unrelated) failure mode; out of scope.

**Absence of evidence is a PASS.** A function that never spills an argument in the scanned window
yields no observation, and the validator must succeed. A validator that failed on "I could not tell"
would refuse most functions and be turned off within a week — which is worse than not having it.

This makes it a smoke detector, not a fire inspection: it catches the class that burned us and is
silent about the rest. Say so in the failure text so nobody reads a pass as a proof.

## 4. What it checks

Integer arguments arrive in `rdi, rsi, rdx, rcx, r8, r9`. For a member function `rdi` is `this`, so a
shape's declared param *k* maps to arg register *k+1*.

For each declared param of class `i32`:

1. Walk instructions from the function entry, up to `kScanBytes` or `kScanInsns`, whichever first.
2. Track whether that param's register has been **redefined** (written) — after a redefinition the
   register no longer holds the incoming argument and every later use is irrelevant.
3. If, *before* any redefinition, an instruction stores that register to memory with `REX.W` set
   (64-bit), that is a **narrowing mismatch**: FAIL, naming the param index, the register, the
   observed width, and the declared one.

Params declared `i64` are not checked — widening is always safe (§1), so there is nothing to refuse.

**Redefinition tracking is what keeps this honest.** `mov $1,%edx` followed later by `mov %rdx,mem`
is not evidence about the incoming argument; without that step the validator would produce false
failures on ordinary functions, which is the fastest way to get a gate disabled.

## 5. Where it lives

`shim/src/call_validate.cpp`, as a fourth entry in `kVocabulary[]` — same TU, same injected
`ModuleView`, same synthetic-image test harness (`shim/tests/call_validate_test.cpp`). The existing
`check-call-descriptors.sh` reads `kVocabulary[]` out of that file, so the gate learns the new name
with no edit.

**It takes no gamedata.** This is the load-bearing decision: the expectation is derived from the
**shape the descriptor already declares**, not from a second hand-written statement of the same fact.
A validator you have to author is one more thing to get wrong in the same way — the bug this exists
to prevent was a hand-written ABI claim drifting from reality, and adding a second hand-written ABI
claim to check the first would be the same mistake with more steps.

So the entry point is not the generic `validate` dispatch but a shape-aware call:

```cpp
// Returns 0 on pass (including "no evidence"), -1 with a named reason on a narrowing mismatch.
int S2Validate_ArgWidths(int shape, const void* fn, const ModuleView& mv, char* out, int cap);
```

## 6. When it runs

At **install**, in `S2_HookInstall`, after the executable-range check and before `s2detour::Install`
touches a byte. A failure is one more named per-descriptor degrade in a function that already has
six, and the hook simply never installs.

Not at *arm* time: arming is cheap and happens for every declared hook at boot, whereas install is
lazy and already the point where we commit to patching. Validating where we patch keeps the check
next to the risk.

## 7. Testing

Everything here is bytes-in / verdict-out, so it is driven off-server over a synthetic module image
exactly as the other three validators are.

1. **The real regression, byte-for-byte.** `TerminateRound`'s actual prologue as a literal, declared
   `this_f32_i32_i32_i32` → FAIL naming param 2; declared `this_f32_i32_i64_i64` → PASS. This is the
   test that would have caught the live segfault, and it is the reason to build any of this.
2. **`this_void` and a clean 32-bit prologue** → PASS.
3. **Redefinition.** `mov $1,%edx` then `mov %rdx,mem` → PASS (the 64-bit store is not the argument).
   Delete the redefinition tracking and this must fail — the false-positive guard has to be provable.
4. **No evidence** — a prologue that spills nothing → PASS, and the reason string says so.
5. **Every param position**, so a table indexed off-by-one cannot pass.
6. **Truncated / undecodable bytes at the window edge** → PASS, never a read past the view. Driven
   with the module view ending mid-instruction; ASan is the arbiter.

Plus a **mutation pass**: break each rule in turn and confirm the corresponding test fails.

## 8. What this does NOT close

Named here so a pass is not over-read:

- An argument never spilled in the window is unchecked.
- Register-to-register uses (`mov %rdx,%rbx`) are not counted as evidence in v1 — only memory stores.
  Extending to them is possible later and should be its own change with its own tests.
- Argument **count** is still unchecked: a shape with too few params silently ignores the rest. That
  is a separate gap and a separate slice.
- Nothing here helps outbound `calls`, where we choose the values and a `0` is an honest null.

## 9. Risks

- **False positives disable gates.** The redefinition rule (§4) and pass-on-no-evidence (§3) exist to
  make this quiet unless it is right. If it ever misfires on a real target, the correct response is to
  narrow the detection, not to add an opt-out to gamedata — an opt-out would be taken by exactly the
  descriptor that needs the check.
- **Prologues change on the update treadmill.** So does every other validator; this one degrades the
  same way, per-descriptor and by name.
- **It can only ever be a smoke detector.** The honest failure mode is a future truncation bug that
  this does not see. That is an argument for it catching the known class, not against building it.
