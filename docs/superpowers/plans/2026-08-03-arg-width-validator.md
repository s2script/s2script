# The arg-width validator — implementation plan

Spec: `docs/superpowers/specs/2026-08-03-arg-width-validator-design.md`.

Four tasks. Tasks 1–2 are the whole feature; 3 wires it in; 4 is docs + the treadmill note. Each task
ends green on `CI=1 make ci`, so any of them is a safe stopping point.

## Global constraints

- **Engine-free.** Everything decodable lives in `shim/src/call_validate.cpp`, which cannot include an
  engine header and whose every outside contact is injected. If a rule cannot fail in
  `shim/tests/call_validate_test.cpp`, it is decoration.
- **Never read past the `ModuleView`.** Every byte fetched is bounds-checked against `mv` first —
  the three existing validators all do this and it is the reason they can be fuzzed safely.
- **Pass on ambiguity.** Any decode failure, any window exhaustion, any register whose provenance is
  unclear → PASS with a reason string saying so. The only FAIL is a positively-observed narrowing.
- **No new gamedata.** The expectation comes from the shape. If you find yourself adding a `validate`
  key, stop — spec §5 explains why that would reintroduce the bug.
- Do not run docker, the live gate, or `scripts/build-sniper.sh`; the human runs those.

---

### Task 1 — the decoder: which argument register does this instruction store, and how wide?

**Files:** `shim/src/call_validate.{h,cpp}`, `shim/tests/call_validate_test.cpp`.

Add an internal helper, exposed to the test:

```cpp
// One decoded observation about an instruction at `p`.
struct ArgUse {
    int  argIndex   = -1;   // 0 = rdi(this), 1 = rsi, ... ; -1 = not an arg-register store
    bool wide       = false;// REX.W set: a 64-bit store
    int  redefines  = -1;   // arg index this instruction OVERWRITES, or -1
    unsigned length = 0;    // 0 = undecodable
};
ArgUse S2Validate_DecodeArgUse(const uint8_t* p, size_t avail);
```

Rules, per spec §3/§4 — implement exactly these and nothing more:

- Decode with `hde64_disasm`. `len == 0` or `F_ERROR` → `length = 0` (caller stops).
- **A store**: `opcode == 0x89` (MOV r/m, r) with `modrm_mod != 3` (memory destination). Source
  register = `modrm_reg` combined with `REX.R`. `wide = hs.rex_w`.
- **A redefinition**: any instruction whose *destination* is an arg register. Cover the two that
  matter and be conservative elsewhere:
  - `opcode == 0x8B` (MOV r, r/m) → destination is `modrm_reg` (+`REX.R`).
  - `opcode == 0x89` with `modrm_mod == 3` (register destination) → destination is `modrm_rm`
    (+`REX.B`).
  - `0xB8..0xBF` (MOV imm32/64 to reg) → destination is the low opcode bits (+`REX.B`).
  Anything else: no redefinition recorded. **That is deliberately unsound in the safe direction** — a
  missed redefinition can only cause a FALSE FAIL, so §4's guard is the thing under test, and Task 2
  bounds the blast radius by scanning a short window.

Map x86-64 register numbers to SysV integer-argument order: `rdi=7, rsi=6, rdx=2, rcx=1, r8=8, r9=9`
→ arg 0..5. Write this as a table, not a switch — the numbers are not in argument order and a
hand-written switch is where an off-by-one lives.

**Tests (this task):** each decode form above as a byte literal, asserting `argIndex`/`wide`/
`redefines`; a non-arg register store (`mov %rbx,mem`) → `argIndex == -1`; truncated input
(`avail` shorter than the instruction) → `length == 0` and no read past `avail`.

**Verify:** `bash scripts/test-call-validate.sh` (it already runs ASan/UBSan), `CI=1 make ci`.

---

### Task 2 — the scan: walk a prologue and render a verdict

**Files:** `shim/src/call_validate.{h,cpp}`, `shim/tests/call_validate_test.cpp`.

```cpp
int S2Validate_ArgWidths(int shape, const void* fn, const ModuleView& mv, char* out, int cap);
```

- Bounds: `constexpr int kScanBytes = 64; constexpr int kScanInsns = 24;` Stop at whichever hits
  first, at a decode failure, or at the end of the view. **Short on purpose** — an argument spilled
  far from entry is not prologue evidence, and a long scan is where a redefinition gets missed.
- Per param of class `i32` (from the shape's `ParamSlot` table — Task 3 injects it; see below): if
  an `ArgUse` with `wide == true` names its register before anything redefines it → FAIL:

  ```
  shape declares param 2 (rdx) 32-bit, but the callee stores it 64-bit at +0x30 — a pointer
  passed through a 32-bit slot is truncated (see hook_dispatch.h)
  ```
- Everything else → 0. When nothing was observed, say so in `out` even on success, so a caller
  logging the reason does not report a silent pass as a proof.

**Dependency note:** `call_validate.cpp` must NOT include `engine_hooks.cpp`'s param tables (that TU
is engine-bound). Pass the widths in as a small injected array — `const uint8_t* wide, int count`
where `wide[k]` is 0 for i32 and 1 for i64 — and let Task 3 supply it from `InfoFor(shape)`. That
keeps this TU testable with hand-written width arrays and no shape enum at all.

**Tests (this task) — spec §7 in full**, including `TerminateRound`'s real prologue as a byte literal
failing under the narrow widths and passing under the wide ones. That single test is the reason this
slice exists; write it first and watch it fail before the implementation lands.

**Mutation pass, reported with real output:** (1) drop the redefinition tracking → the redefinition
test must fail; (2) ignore `rex_w` → the `TerminateRound` test must fail; (3) shift the register
table by one → the per-position test must fail. Restore after each.

---

### Task 3 — wire it into install

**Files:** `shim/src/engine_hooks.cpp`, `shim/src/call_validate.h`.

In `S2_HookInstall`, after the executable-range check and **before** `s2detour::Install`:

- Build the width array from `InfoFor(shape)` (`kParamI64` → 1, else 0).
- Resolve the `ModuleView` for the address. `engine_calls.cpp` already owns module lookup —
  reuse it rather than adding a second phdr walk (the comment at `S2_AddressIsExecutable` explains
  why that walk lives there).
- On FAIL, return the validator's reason through the existing `Fail(reasonOut, reasonCap, ...)` path.
  Core already surfaces that verbatim into the hook's named degrade, so no core change is needed.
- On PASS, keep the existing tier/steal note. Do not append the validator's "no evidence" text to a
  successful install line — it belongs in the failure path, not in every boot log.

**Tests:** extend `shim/tests/hook_dispatch_test.cpp`? No — `S2_HookInstall` is engine-bound. The
verification here is that `make shim` builds and `CI=1 make ci` is green; the logic itself was proven
in Task 2. Say that plainly rather than inventing a test that cannot fail.

---

### Task 4 — documentation and the treadmill

**Files:** `docs/ARCHITECTURE.md`, `docs/re-strategy.md`, `docs/PROGRESS.md`.

- `ARCHITECTURE.md`: add `arg-width` to the validator vocabulary, and state the asymmetry (narrow is
  a crash, wide is safe) since that is the rule an author needs.
- `re-strategy.md`: this is a treadmill fact. A CS2 update that changes an argument's width now
  produces a named refusal at install instead of a truncation — add it to the "what the gates catch"
  list.
- `PROGRESS.md`: one entry, including **what it does not close** (spec §8) so the next reader does not
  over-trust a pass.
- No changeset: nothing under `packages/` changes.

---

## What "done" looks like

- `CI=1 make ci` green; `scripts/test-call-validate.sh` covers the new validator under ASan/UBSan.
- The `TerminateRound` regression test fails without the fix and passes with it.
- All three mutations reported with real output.
- `check-call-descriptors.sh` picks up the new vocabulary entry with no edit to the gate (that is the
  point of it reading `kVocabulary[]`) — confirm by running it.

## Live gate

**Not required for this slice.** It adds a refusal path; it patches nothing new. The existing hooks
must still install on a real server, which the human can confirm on the next gate run — if
`onTerminateRound` and `onRespawn` still report `INSTALLED (near E9, stole 6)`, the validator passed
on the real binary, which is the only live fact this slice needs.

Worth flagging to whoever runs it: if the validator refuses a hook that used to install, that is
either a real width bug it just caught or a false positive from §4's unsound-in-the-safe-direction
redefinition tracking. The reason string distinguishes them — it names the offset, so the byte can be
disassembled and settled in a minute.
