# Slice 10: Extract host responsibilities without changing behavior

**Status:** Planned; implementation has not started.
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
