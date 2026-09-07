# Runtime hardening and performance scope

## Objective

Resolve all ten findings from the September 4 codebase review: stale client identities,
append-only resource ledgers, stale subscription indexes, incomplete socket cleanup, lost
cookie writes, unbounded async work, linear hook lookup, linear timer scheduling, synchronous
file polling, and oversized host modules.

This document records the scope and proposed implementation constraints. The companion
[workflow](../plans/2026-09-04-runtime-hardening.md) orders the work into ten independently
reviewable slices. Creating these documents does not execute, publish, or schedule the work.

## Evidence and limits

- The review ran all 565 SDK tests successfully.
- Isolated checks reproduced retained timer ledger entries, stale subscription ID mappings,
  retained empty channels, stale Client objects targeting replacement occupants, delayed
  cookie notifications targeting reused slots, and cookie loss after a rejected write.
- Socket termination and unbounded queues were identified by source inspection.
- Hook lookup, timer scheduling, and file polling are optimization candidates identified by
  source inspection. No live CS2 speedup has been measured.
- The full native suite and live CS2 gate remain required during implementation.

## Required behavior

1. A Client captured for one connection must never act on its slot's next occupant. Deferred
   notifications carry and validate connection identity, including reconnects by the same SteamID.
2. Ledgers describe active resources. Completion, explicit disposal, and unload release each
   resource exactly once. Reverse acquisition order remains the unload order.
3. Subscription indexes contain only live subscriptions and necessary channel descriptors.
4. Each socket has one terminal transition. Write failure, peer failure, cancellation, timeout,
   and plugin unload cannot strand workers, handlers, or pending connection promises.
5. Accepted cookie changes survive transient database errors in an owned pending-write queue.
   A failed older write cannot overwrite a newer accepted value during retry. This is not a
   promise of crash durability before a database commit.
6. Async admission and buffering have finite count and byte limits. Frame polling has finite
   batches and fair progress across sources. The game thread never blocks for queue capacity.
   Terminal signals remain deliverable under saturation. Arbitrary JS callback execution time
   is outside the guarantee of a bounded poll batch.
7. Hook lookup scales with subscribers for the addressed entity and hook kind while preserving
   registration order, liveness checks, result folding, and reentrant subscription behavior.
8. Timer scheduling preserves current nextTick/nextFrame semantics, equal-deadline ordering,
   self-cancellation, repeating behavior, and owner-generation checks.
9. Filesystem discovery, reading, and archive parsing run without engine or V8 access on a
   worker. Publication, lifecycle transitions, config application, and notifications stay on
   the game thread. Superseded work cannot replace newer state.
10. Host extraction preserves behavior, ABI order, module dependencies, borrow discipline,
    and reset/teardown ordering. Moving code alone is not a performance claim.

## Global constraints

- The core owns every engine touchpoint.
- Core is engine-generic; games are packages. Dependencies point one way: game → core, never core → game.
- Never expose a raw pointer or raw cross-plugin reference across time.
- The ledger is the teardown authority.
- Degrade per-descriptor, never crash globally.
- A slice is one branch and one PR.
- Keep native ABI, JS wrappers, SDK declarations, generated artifacts, and affected consumers
  together when a contract changes. Classify compatibility and include required version bumps
  and changesets in that slice.
- Gates belong in scripts/ci-js.sh or scripts/ci-native.sh; CI workflow YAML invokes those scripts.
- Preserve the pinned V8 dependency and existing runtime/toolchain requirements.
- Use Linux/sniper binaries for live CS2 validation; a macOS build is not a live-server gate.

## Approach

Use sequential, narrowly scoped fixes with deterministic regression tests, then measured
optimizations, then mechanical extraction. A combined rewrite would make regressions difficult
to localize. Extracting the host first would obscure the faulty paths before their behavior is
pinned. Execution is sequential by default; parallel agents are not required by this workflow.

## Completion

Every finding maps to a completed slice, each slice has regression evidence and applicable
gates, and the integrated build passes a Linux live-server soak. Performance results compare
the same workloads and hardware, report raw measurements, and distinguish native RSS from
V8 heap. No speedup is claimed from asymptotic analysis alone.
