# Shared HUD capture implementation plan

- [x] Add an internal engine-generic shared entity switch registry and validated native.
- [x] Register host owner cleanup, frame retry, unconditional disconnect cleanup, and entity/map invalidation.
- [x] Preserve hook bypass and defer JavaScript during owner-store engine follow-ups.
- [x] Route game cursor operations through host tokens, distinguish root and manual ownership, and roll back failed show.
- [x] Add V8 multi-context ownership/failure/lifecycle/reentry tests and game presenter regressions.
- [x] Run full Rust core tests and component suite; inspect portability/boundary gates.
- [ ] Stack on the API contract PR, publish a separate reviewable PR, and await review without merging.

Validation uses a temporary macOS linker wrapper that drops the baseline Linux-only -z,nodelete flag. The wrapper is outside the repository; native portability remains a separate PR. No server changes.
