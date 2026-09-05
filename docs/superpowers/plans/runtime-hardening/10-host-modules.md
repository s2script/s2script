# Slice 10: Extract host responsibilities without changing behavior

**Status:** Implemented and independently reviewed; final whole-stack review, benchmarks, Linux native/shim linking and installed game-symbol checks pass. Final mixed-soak and human acceptance remain pending.
**Branch:** `refactor/hardening-10-host-modules`
**Parent / PR base:** `core/hardening-09-loader-worker`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/v8host.rs, core/src/lib.rs, shim/src/s2script_mm.cpp, shim/CMakeLists.txt.
**Create proposed destinations:** core/src/v8host/{lifecycle,timers,natives,tests}.rs and
shim/src/config_ops.{h,cpp}. Move existing code only after validating each responsibility's
imports and state ownership; keep the isolate-owning entry point in v8host.rs.

**Boundary:** Child modules use narrow adapters. Keep HOST/PLUGINS/REGISTRY ownership explicit;
do not create a universal context object or expose mutable global stores for convenience.

- [ ] Record exports, native registration names, reset order, ledger teardown order, and shared
  engine-op ABI field order before moving code. Use existing boundary/ABI checks as witnesses.
- [ ] Move the existing test module to v8host/tests.rs, preserving test names and execution
  constraints. Move lifecycle/loading, timer adapters, and native installation in separate
  mechanical commits within this slice; run focused tests after each move.
- [ ] Extract C++ config operations and path handling into config_ops.h/.cpp, keeping their
  owned buffers and signatures together. Update CMake and all call sites atomically.
- [ ] Review each move for new dependencies, accidental public visibility, changed initialization
  order, nested RefCell borrows, and handles dropped after the isolate. Preserve assertions.
- [ ] Run make ci and live lifecycle/reentry/hook gates. Re-run the exact benchmark workloads;
  investigate regressions. File-size reduction is a maintainability result, not speed evidence.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.

## Reviewed extraction and local evidence

Separate commits moved tests, lifecycle/loading, timer adapters, native installation, and
C++ config operations. v8host.rs retains HOST, PLUGINS, REGISTRY, isolate ownership and
reset sequencing. It decreased from 16,365 to 6,239 lines; this is an organization result,
not a speed measurement. Existing module/test names and narrow parent paths are preserved.
The two native-name lint configurations and deferred-selftest scanner follow the moved code;
CMake includes the new config translation unit.

Independent review compared normalized source and order and found no semantic changes.
All 240 registered native names and 49 C exports are preserved. FFI, generated engine-op
source, shim ABI header and the complete shutdown block are byte-identical. The config
function bodies are unchanged apart from the linkage required between hidden translation
units. Parent-failing regression testing is not applicable to this mechanical move;
existing lifecycle, handoff, teardown, timer and native-registration tests are the witnesses.

- Core: 791 passed, zero failed, two intentionally ignored pressure tests.
- Both pressure tests pass in their own fresh processes.
- Full JavaScript/Docker gate passes, including 584 SDK tests and all plugin/example,
  typecheck, generated-code and native-name gates.
- Boundary/name/invoke-ABI checks pass. The new C++ translation unit passes a C++17
  syntax check with Wall/Wextra and no diagnostics.
- Linux container static checks pass: 126 engine ops with matching names/order/arity,
  sentinel -1000, and guarded deferred-selftest registration.

The independently reviewed local code was b6a023f0f16313bf00d466bddbeacf1201381872;
restacking onto the evidence updates produced 2158b5d5b468ac3a6f5a62be91bd87ab22e4e0f1.
The tree difference is only the four parent evidence documents, with no production changes.
Full Linux Rust/shim linking, installed-engine acceptance and the final 60-minute mixed
soak remain required. Final benchmarks and whole-stack review are tracked in the shared plan.

## Final integrated correction review

Final production source `ba6c7c19548fdad46d14bb5c2aa342ce94804ae1` incorporates the
consolidated slice-six and slice-nine corrections. The extraction conflicts were resolved
with source/test equivalence witnesses, and independent re-review passes with no residual
actionable finding. Final local core tests pass 797/0 with three intentionally ignored
pressure cases; each pressure case passes separately. Boundary, ABI, C++ syntax and
Linux Docker static checks also pass. See [the final report](final-review/final-fix-review.md)
and [paired benchmarks](../../../benchmarks/2026-09-runtime-hardening/README.md).
Full Linux compilation/linking and installed-engine/60-minute soak acceptance remain pending.

The subsequent [final Linux native gate](final-review/linux-native-acceptance.md) passes
797 core tests, three pressure cases, sanitizer checks, full shim linking and core-entry-point
checks. Game-library resolution explicitly skipped without a CS2 installation; that phase
and installed-engine/soak acceptance remain pending.
