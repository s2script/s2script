# Slice 5: Retain cookie writes until acknowledged

**Status:** Planned; implementation has not started.
**Branch:** `plugins/hardening-05-cookie-persistence`
**Parent / PR base:** `core/hardening-04-socket-lifecycle`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** plugins/clientprefs/src/plugin.ts, core/src/cookies.rs and their tests; DB adapters
only if a transaction primitive is necessary. **Test:** plugins/clientprefs/src/plugin.test.mjs.

**Boundary:** Separate connection cache eviction from persistence ownership. Pending writes
carry SteamID, cookie name, value, and monotonic revision; retry order cannot restore old values.

- [ ] Test a database rejection on the first and middle write, delayed query versus a newer
  local change, reconnect during retry, and offline updates for the same key while a write is
  in flight. Assert the newest accepted value eventually reaches storage.
- [ ] Move disconnected/offline dirty values into a host-owned pending outbox before clearing
  connection state. Keep it across clientprefs reload; acknowledge only the written revision.
- [ ] Drain bounded batches with capped exponential retry delay (100ms initial, 5s maximum).
  Coalesce unsent updates by key without acknowledging newer updates on an older completion.
  Do not use second-resolution wall time as the ordering authority.
- [ ] Define outbox capacity and overload reporting with slice 6's admission policy. Until that
  slice, expose counters and preserve values; do not silently drop accepted writes on failure.
- [ ] Test plugin reload during an in-flight batch and document shutdown/crash durability limits.
  Run plugin/core tests and a live SQLite contention/recovery scenario.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.
