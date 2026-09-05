# Slice 3: Bind Client to a connection lifetime

**Status:** Planned; implementation has not started.
**Branch:** `core/hardening-03-client-identity`
**Parent / PR base:** `core/hardening-02-channels`
**Workflow:** [Full workflow and gates](../2026-09-04-runtime-hardening.md)
**Spec:** [Shared scope](../../specs/2026-09-04-runtime-hardening-design.md)

This is the branch-local execution checklist. Apply the common baseline, compatibility,
review, and completion gates from the workflow. Keep status and evidence here while the
stack is being implemented; consolidate the shared status table after restacking.

**Modify:** core/src/client.rs, core/js/prelude.js, packages/sdk/clients.d.ts,
core/src/cookies.rs, plugins/clientprefs/src/plugin.ts; inspect shim/src/s2script_mm.cpp's
client lifecycle bookkeeping and games/cs2/js/pawn.js consumers.
**Tests:** client.rs, cookies.rs, v8host.rs; create plugins/clientprefs/src/plugin.test.mjs.

**Boundary:** A connection token belongs to the host's client-liveness books, not SteamID alone.
Preserve the departing identity through its disconnect callback, then invalidate it. Account
for clients already present at runtime initialization and engine map-transition semantics.

- [ ] Add tests where A disconnects, B occupies the same slot, and a saved Client attempts
  kick/chat/command/voice operations. None may affect B; isValid must be false for the old handle.
- [ ] Add tests for the same SteamID reconnecting, late cookie query completion, a cached event
  queued before slot reuse but dispatched afterward, and disconnect-handler identity access.
- [ ] Mint a host connection generation on a new connection. Capture it when constructing Client
  and gate getters and actions against it. Document stale-access return values consistently
  with the existing API's safe-access conventions.
- [ ] Carry slot plus connection generation through the pending cookie notification queue and
  recheck at dispatch. Drop stale loads before cache mutation, not only before notification.
- [ ] Inspect generated/manual Player wrappers and retained Client uses; update affected callers
  and classify SDK/host API compatibility together. Do not hand-edit generated files.
- [ ] Wire the new plugin test into scripts/ci-js.sh. Run core, SDK, plugin typecheck, and live
  disconnect/reconnect tests; confirm normal connect, disconnect, and map-change behavior.

## Evidence required before completion

- [ ] Record the regression test and its failure on the parent implementation.
- [ ] Record implementation commits and passing focused checks.
- [ ] Record applicable full-gate and live-server results, with environment limitations stated.
- [ ] Review the diff against the parent and restack descendants using recorded old tips.
- [ ] Set status to complete only when this slice's required gates pass.
