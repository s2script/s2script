# Cross-framework CS2 performance harness — design

**Date:** 2026-07-14
**Status:** Design (Phase 1 = s2script + CounterStrikeSharp head-to-head)
**Author:** brainstormed with the user; plan to be Fable-authored, executed by Opus/Sonnet subagents.

## Goal

A **framework-pluggable performance harness** that measures and compares the runtime cost of
CS2 scripting frameworks — s2script, CounterStrikeSharp (CSSharp), ModSharp, SwiftlyCS2 — on the
capability surface they all share, under one uniform, reproducible yardstick.

The harness serves three goals at once (user-selected "all three, phased"):

1. **Positioning** — credible, publishable head-to-head numbers.
2. **Engineering** — find s2script's own bottlenecks and guard against regressions.
3. **Architecture validation** — quantify what the charter's V8-per-plugin-context + ledger +
   safety model costs vs. shared-runtime frameworks.

## Scope

### Decomposition (phased)

This is delivered as a sequence of sub-projects, each its own spec → plan → implementation cycle:

- **Sub-project 1 (THIS effort):** the framework-agnostic harness (load driver, metric collectors,
  report generator) + s2script fully instrumented + the complete workload catalog implemented in
  s2script + **CounterStrikeSharp** installed and ported → a real **2-way head-to-head** report.
- **Sub-project 2 (deferred):** add **ModSharp** (port the catalog, add its runner config).
- **Sub-project 3 (deferred):** add **SwiftlyCS2** (port the catalog, add its runner config).

The catalog and harness are designed as a **framework-neutral contract** from day one so
sub-projects 2 & 3 are "install + port + register a runner," not a harness redesign.

### In scope (Phase 1)

- A new top-level `bench/` directory: the Python orchestrator, report generator, workload manifest,
  results store, and the CSSharp C# benchmark plugins.
- s2script benchmark plugins under `examples/bench-*`.
- CounterStrikeSharp installed as a **swappable** second Metamod addon in the CS2 Docker.
- Six benchmark scenarios (B0–B5) measured for baseline, s2script, and CSSharp.
- A committed markdown report + raw per-run JSON.

### Out of scope (Phase 1)

- Any core/shim change or sniper rebuild. Phase 1 is **harness + plugins only** (see "No core
  change" below).
- ModSharp and SwiftlyCS2 ports (sub-projects 2 & 3).
- s2script-only capabilities with no counterpart in the other frameworks (async `fetch`,
  WebSocket, DB, cookies, zones interface) — nothing fair to compare against, so excluded from the
  catalog.

## Key design decisions

### Sequential, never concurrent

Frameworks are benchmarked **one at a time, each alone in its own server session**. We never load
two frameworks into one server. Every metric is diffed against a **clean no-framework baseline**
(vanilla CS2 + Metamod, no scripting addon). This:

- avoids Metamod/hook interference between frameworks,
- gives each framework the full server (fairest),
- makes "framework tax" a first-class, directly-measured quantity (loaded − baseline).

The harness swaps the *active* addon between runs (enable/disable the framework's Metamod VDF +
addon dir) and restarts the container.

### Hybrid measurement

- **External / server-level (uniform yardstick)** for tick overhead, memory, startup: measure the
  real server from outside — the same tool for every framework, zero trust in per-framework timing
  code.
- **In-plugin loop-and-time microbenchmarks** for boundary-call and dispatch latency: the only way
  to isolate a single sub-microsecond operation. Fairness comes from implementing the **identical**
  loop/op in each language.

### In-plugin transport = a `[BENCH] {json}` log-line contract

Each framework's benchmark plugin prints a single structured line per result:

```
[BENCH] {"scenario":"B2","metric":"boundary_schema_read","ns_per_op":48.3,"iters":1000000,"framework":"s2script"}
```

The harness scrapes these from `docker logs`. Chosen over a shared results file because printing to
the server console is the **most uniform** channel across V8 / .NET / C++-or-Lua runtimes — no
per-framework file-path coupling. The JSON shape is the cross-framework contract.

### No core change / no sniper rebuild (Phase 1)

- External metrics need no in-process cooperation.
- In-plugin microbenchmarks are **loop-amortized over 1e6 iterations** using each framework's own
  best available timer. For s2script, V8 `Date.now()` (ms resolution) over a ≥100 ms total loop is
  <0.1% error — precise enough. A monotonic-ns native (`@s2script/timers` op) is an **optional,
  deferred** precision upgrade, added later only if the numbers demand it.

## Architecture

### Data flow

```
bench/run.py  ──(1) swap active addon + set bot_quota──▶  CS2 Docker container
     │                                                          │
     │◀─(2) rcon: wait map-live, warmup, drive scenario ────────┤  (reuses scripts/rcon.py)
     │                                                          │
     │──(3) sample /proc/<cs2-pid>: VmRSS + utime/stime jiffies─┤  external, uniform yardstick
     │──(3) rcon `stats`: achieved server FPS / tickrate ───────┤
     │                                                          │
     │◀─(4) scrape docker logs for `[BENCH] {json}` lines ──────┤  in-plugin microbench transport
     ▼
bench/results/<framework>-<scenario>-<run>.json
     │
     ▼
bench/report.py ──▶ bench/results/REPORT.md   (per-scenario tables, baseline-diffed comparison)
```

### Components (each independently testable)

1. **`bench/rcon` helper** — thin reuse of `scripts/rcon.py`'s Source RCON client (map-live wait,
   `stats`, command drive, `bot_quota`).
2. **`bench/proc.py`** — given the cs2 PID (via `docker top s2script-cs2` / `pidof`), samples
   `/proc/<pid>/status` VmRSS and `/proc/<pid>/stat` utime+stime jiffies at a fixed interval;
   returns a time series → CPU-seconds and peak/mean RSS.
3. **`bench/logscrape.py`** — tails `docker logs` for `[BENCH] {json}` lines, parses, validates
   against the contract schema.
4. **`bench/runner.py`** — the per-framework runner config: how to enable/disable this framework's
   addon, its "ready" log signature, its plugin-deploy path. s2script and CSSharp runners in
   Phase 1; ModSharp/Swiftly runners are added by later sub-projects.
5. **`bench/run.py`** — the orchestrator: for a (framework, scenario, run-index) it restarts the
   container with the right addon active, waits ready + map-live, warms up (discards first N s),
   drives the scenario, samples externally for a fixed duration, scrapes in-plugin lines, writes
   one results JSON.
6. **`bench/report.py`** — aggregates all results JSON → `REPORT.md`: median-of-K per scenario,
   baseline-subtracted, s2script vs CSSharp tables + run metadata (CS2 build, host, run count,
   timestamp).

### Workload catalog (framework-neutral)

Only the surface **all four frameworks share**: frame hooks, event hooks, command hooks, schema
read/write, player enumeration, chat output.

| ID | Scenario | Metric type | Isolates |
|----|----------|-------------|----------|
| **B0** | Framework loaded, one plugin that only subscribes an empty frame handler | External | Idle "in-the-loop" tick tax + base RSS (framework − baseline) |
| **B1** | Frame handler reads K schema fields per player each tick; sweep work level {0,1×,4×,16×} with a fixed `bot_quota` (e.g. 10) | External cost curve | Per-frame boundary-call cost under load; regression slope = per-op cost, intercept − baseline = framework tax |
| **B2** | Tight loop of 1e6 iterations: schema-field read, native call, entity lookup (three sub-metrics) | In-plugin, loop-amortized | Raw interop latency, ns/op |
| **B3** | An event handler subscribed; drive a high-frequency event and count dispatches over a fixed window (+ in-plugin per-dispatch timing where the framework can fire events) | In-plugin + external | Dispatch cost + throughput |
| **B4** | rcon-driven command bursts (K commands); measure round-trip latency distribution | External | Command dispatch path |
| **B5** | Load K equivalent no-op plugins | External | RSS delta per plugin, cold plugin-load time (+ hot-reload, s2script-only, reported but not compared) |

**Parity rule:** each scenario has a single, written, framework-neutral spec ("read the player
pawn's health field", "subscribe an empty per-tick handler", etc.). Every framework's plugin must
implement exactly that — no extra work, no shortcuts. The C# / future ports are held to the same
spec. Where a framework lacks a capability (e.g. can't fire a synthetic event for B3's in-plugin
half), the harness records "N/A" for that sub-metric rather than fabricating a comparison.

## Metric collection detail

- **CPU cost:** `/proc/<pid>/stat` utime+stime jiffies sampled over the run → CPU-seconds consumed
  → CPU per wall-second. The framework's CPU tax = (loaded − baseline) under identical load.
- **Server tick health:** rcon `stats` reports achieved server FPS (server frames/sec ≈ realized
  tickrate); under CPU pressure it dips below the target tick. Sampled at 1 Hz, reported as
  mean/min.
- **Memory:** `/proc/<pid>/status` VmRSS sampled; peak and steady-state; framework RSS =
  (loaded − baseline).
- **Startup:** wall time from container restart to the framework's "ready" log signature, and to
  first map-live.
- **In-plugin:** ns/op = (loop wall time) / iters, loop ≥ 1e6, each framework's own timer.

## Fairness controls

- **Same CS2 build** = the container's current build is the shared ground; all frameworks run
  against it in one benchmarking session. Record the build id in the report.
- **Pinned framework versions:** the CSSharp release matched to the container's CS2 build; recorded.
- Fixed map, fixed `bot_quota`, fixed tickrate/`fps_max`.
- **Warmup discard** (first N s) + **median of K ≥ 5 runs** per (framework, scenario).
- `taskset` CPU-pin the container if the host allows; otherwise record that it was not pinned.
- Stop the `mysql`/`postgres` sidecars during runs (idle, but removes noise).
- Baseline re-measured in the same session (not a stale number).

## CounterStrikeSharp onboarding (Phase 1)

- Added to the Docker as a second, **swappable** Metamod addon (its own VDF + addon dir, enabled
  only for CSSharp runs). Chosen first: most mature, cleanest .NET install, most likely to have a
  build matching the container's CS2.
- **Risk (recorded):** a framework may not have a build compatible with the container's current CS2
  build. The runner's "ready" check fails **loud** — the harness reports the framework as
  unavailable for that session rather than silently producing garbage.
- The catalog ported to C# plugins that emit the same `[BENCH] {json}` contract and obey the same
  per-scenario parity spec.

## Deliverable

- `bench/results/REPORT.md` — per-scenario tables, s2script vs CSSharp, baseline-diffed, with run
  metadata.
- Raw per-run JSON committed under `bench/results/` for reproducibility.
- A short `bench/README.md` — how to run the harness, add a framework runner, and read the report.

## Testing

- **Harness units** (Python): `proc.py` parsing against fixture `/proc` snapshots; `logscrape.py`
  against fixture log lines; `report.py` against fixture results JSON → expected markdown. Hermetic,
  no container needed.
- **Contract validation:** a schema check that every `[BENCH]` line matches the JSON contract.
- **Live gate:** the real 2-way run on the CS2 Docker (baseline + s2script + CSSharp) producing the
  committed REPORT.md. This is the acceptance test for Phase 1.

## Orchestration (per user instruction)

- **Fable authors the implementation plan** (spawned as a subagent after this spec is approved —
  the established "Fable-architected" pattern).
- **Opus/Sonnet execute** via subagent-driven-development: the Python harness and s2script plugins
  on Sonnet; the fairness-sensitive measurement/report logic and the CSSharp C# port on Opus; an
  adversarial review per task.

## Risks & open questions

- **Framework build compatibility** with the container's CS2 (mitigated: loud "ready" check).
- **CPU-tax signal-to-noise** for very cheap ops in B1's external curve — mitigated by the
  work-level sweep (slope isolates per-op cost from the noisy intercept) and median-of-K.
- **`stats` FPS semantics** across CS2 updates — validated live during the gate; if unreliable,
  fall back to CPU-jiffies-per-wall-second as the primary tick-health signal.
- **CSSharp event-firing parity** for B3's in-plugin half — if CSSharp can't fire a synthetic
  event, that sub-metric is "N/A" and B3 relies on the external throughput half.

## Follow-ups (do NOT build ahead)

- Sub-projects 2 & 3: ModSharp and SwiftlyCS2 runners + C++/Lua/C# ports.
- Optional monotonic-ns native for sharper in-plugin timing.
- A charted/artifact report view.
- Concurrency-scaling scenarios (many plugins × many players) beyond B1/B5.
