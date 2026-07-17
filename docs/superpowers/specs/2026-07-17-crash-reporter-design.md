# Crash Reporter — design spec

**Date:** 2026-07-17
**Status:** approved (brainstorming) → ready for planning
**Scope of THIS spec:** sub-project 1, the in-server **capture client**, plus the **incident
envelope** wire contract it shares with the backend. The central backend (sub-project 2) and the
CI symbol pipeline (sub-project 3) are sketched here for context but each gets its own spec.

---

## 1. Goal

Give s2script the equivalent of what **Accelerator** was to SourceMod — a crash handler that turns
an otherwise-silent server death into an actionable report — and go further by attributing crashes
using information only s2script has: *which JS plugin was running, in which dispatch, calling which
engine op, on which gamedata/CS2 build*. The captured data is uploaded to a **central service the
project hosts**, where server owners see their own crashes and the project sees fleet-wide patterns.

The differentiators over a generic crash reporter:

1. **The breadcrumb** — every report names the culprit plugin + dispatch + engine op, not just a
   native module+offset.
2. **The treadmill fingerprint** — every report carries the gamedata/schema/hl2sdk build fingerprint
   and the live CS2 build number, so a crash wave right after a CS2 update is instantly correlated to
   "stale gamedata," operationalizing the "maintenance treadmill is a first-class feature" guardrail.

## 2. Background / the gap today

s2script has **no native crash handler**. The FFI boundary wraps calls in `catch_unwind`
(`core/src/ffi.rs`), which recovers *Rust panics* and keeps the process alive — but a hard fault
(SIGSEGV/SIGABRT/SIGBUS/SIGFPE) in the shim, core, V8, or the detoured CS2 game code takes down the
whole `cs2` process with nothing captured. Recoverable panics that `catch_unwind` swallows are lost
silently. This spec closes both gaps and unifies them with fatal JS errors into one pipeline.

## 3. What counts as a crash (decided)

All three, converging on **one incident pipeline / one envelope**:

- **Hard native fault** — the Accelerator-equivalent. Minidump attachment.
- **Fatal JS error** — uncaught exception / unhandled rejection in a plugin context. JS-stack detail,
  no minidump.
- **Rust panic** — currently swallowed by `catch_unwind`; now *reported* (process still survives).

## 4. Architecture & decomposition

```
┌─────────────────────────── IN THE GAME SERVER ───────────────────────────┐
│  core/shim: breadcrumb tracker ──writes──► [signal-safe POD shared state] │
│  (every dispatch cheaply stamps: plugin, dispatch, engine-op, js loc)     │
│                                                    ▲ reads (async-safe)    │
│   ┌──────────────┐   ┌───────────────┐   ┌─────────┴──────────┐           │
│   │ native fault │   │ fatal JS error│   │  Rust panic hook   │           │
│   │  (Breakpad)  │   │  (V8 alive)   │   │ (core panic hook)  │           │
│   └──────┬───────┘   └───────┬───────┘   └─────────┬──────────┘           │
│          └──────────── one INCIDENT ENVELOPE ──────┘                       │
│                 (+ minidump attachment on native)                          │
│                     spool to disk → upload-on-next-boot (opt-in, API key)  │
└──────────────────────────────┼─────────────────────────────────────────────┘
                               │ HTTPS (envelope + optional minidump)
                               ▼
┌───────────────────── CENTRAL SERVICE (project-hosted) ───────────────────┐
│  ingest → symbolication (per-build symbol store) → grouping/dedup → DB    │
│  dashboard: per-operator crashes, fleet-wide patterns, treadmill alerts   │
└──────────────────────────────────────────────────────────────────────────┘
```

**Subsystems & build order (by risk, thinnest thread first):**

1. **Capture client** (this spec) — breadcrumb, three capture paths, envelope contract, spool +
   uploader. Ships inside the runtime, opt-in.
2. **Central backend** (spec #2) — ingest, symbolication, grouping, dashboard.
3. **Symbol pipeline** (spec #3, a CI step) — every release runs `dump_syms` on `s2script.so` /
   `libs2script_core.so` and ships symbols to the store, so the backend can symbolicate. Small but
   load-bearing: forget it and no native crash is ever symbolicated.

## 5. Where it lives (engine-generic)

The mechanism is runtime infrastructure in **`shim` + `core`**, armed during boot *before any plugin
loads* (so boot-time crashes are caught). It is **engine-generic** — catching a signal and dumping the
process is true on any Source 2 game (passes the CLAUDE.md litmus test) — so it must **not** import
`games/*`. The only game-specific fields (live CS2 build number, gamedata fingerprint specifics) are
supplied *into* the breadcrumb by the `@s2script/cs2` game package through an engine-generic setter,
never by core reaching into the game package.

It is **not** a JS plugin. An optional thin `@s2script/crash` SDK surface (plugins push extra
context / custom breadcrumb annotations) may follow later and would go through the permissions model;
it is out of scope for spec #1.

---

## 6. Sub-project 1 — the capture client

### 6.1 The breadcrumb (the crux)

A fixed-size, pre-allocated, lock-free **POD struct in `core` static memory** that the signal handler
reads with plain memory loads only — no allocation, no locking, no V8. Updated near-free on every
dispatch (a handful of word stores). Never grows, never reallocates.

Fields:

- **Identity / treadmill:** s2script version, apiVersion, gamedata fingerprint + generated-at,
  hl2sdk build, schema build, **live CS2 build number** (set by `@s2script/cs2`), map name.
- **Current context:** current plugin id (which V8 context, or `core`), current dispatch
  (`OnGameFrame:<phase>` / event name / engine-op name / entity-IO), best-effort JS `file:line`
  (stamped cheaply on JS entry), tick counter, uptime.
- **Ring buffer:** last 16 `(tick, plugin, dispatch)` entries — the sequence leading to the fault.
- **Plugin table:** fixed-size array of `{id, version}`, updated on plugin load/unload, so the handler
  can enumerate loaded plugins without allocating.

**Threading:** the breadcrumb is main-thread-focused (dispatch + engine + entity ops all run on the
game thread). A fault on a tokio worker thread is still fully captured by the minidump's all-thread
native stacks; the breadcrumb simply describes main-thread context in that case. Fields are written
only by the main thread, so no lock is required; the handler's reads tolerate a torn write (it is a
best-effort snapshot, and the minidump is the source of truth for the native stack).

**Handler output:** the signal handler writes the **raw POD bytes** of the breadcrumb to a `.s2meta`
sidecar file with a single `write()` — no JSON formatting inside the handler (not signal-safe). The
uploader, running later in normal context, reads the POD and renders the JSON envelope.

### 6.2 Capture path — native fault (Breakpad)

- Vendor **Google Breakpad** into the shim; arm a `ExceptionHandler` at boot, after core init.
- Install on a dedicated `sigaltstack` so stack-overflow faults are still catchable.
- **Chain to any pre-existing handler** (the game's own handler / core-dump path) — the handler must
  not swallow the crash or suppress core dumps. After writing artifacts, chain / re-raise so the
  process still dies as it otherwise would.
- On fault, in a minimal signal-safe callback: Breakpad writes the **minidump** (all thread stacks,
  registers, loaded-module list with build IDs) to the crash-spool dir; our callback `write()`s the
  `.s2meta` breadcrumb sidecar next to it.
- In-process minidump writing (as Accelerator did). Out-of-process (forked handler) is a documented
  future hardening, not in scope for spec #1.

### 6.3 Capture path — fatal JS error

- V8 is alive, so this is the rich path. Register V8 uncaught-exception and unhandled-promise-rejection
  callbacks, scoped to the plugin context. Produce an envelope (`kind: "js"`) with the full JS stack,
  culprit plugin id, source `file:line`, and message. No minidump.
- **Rate-limit / dedup by stack signature** so a per-frame thrower cannot spam the pipeline.
- Reporting is orthogonal to the existing degrade-per-descriptor unload policy — we report the error
  regardless of whether the plugin is unloaded; unload behavior is unchanged.

### 6.4 Capture path — Rust panic

- Install a `std::panic::set_hook` in core that captures panic message + backtrace + the current
  breadcrumb into the same envelope (`kind: "panic"`).
- The existing `catch_unwind` in `ffi.rs` still keeps the panic from crossing FFI (the process
  survives); the panic is now **reported instead of silently swallowed**.

### 6.5 The incident envelope (wire contract)

Versioned JSON, identical skeleton for all three kinds; only `detail` differs. This is the contract
between the capture client and the backend; it is frozen here (bump `schema_version` to evolve).

```jsonc
{
  "schema_version": 1,
  "incident_id": "<uuid, generated in normal context by the uploader>",
  "kind": "native | js | panic",
  "occurred_at": "<ISO-8601 crash time — stamped directly for js/panic (normal context); for native, reconstructed by the uploader from the spool file's mtime, since native uploads on next boot>",
  "s2script": { "version": "...", "api_version": "..." },
  "gamedata": { "fingerprint": "...", "generated_at": "...", "hl2sdk": "...",
                "schema_build": "...", "stale": false },
  "game":     { "name": "cs2", "build_number": 0, "map": "...", "players": 0, "uptime": 0 },
  "host":     { "server_id": "<stable, hashed, non-PII>", "os": "..." },
  "breadcrumb": { "plugin": "...", "dispatch": "...", "engine_op": "...",
                  "js_location": "file:line", "ring": [ { "tick": 0, "plugin": "...", "dispatch": "..." } ] },
  "plugins":  [ { "id": "...", "version": "..." } ],
  "detail": {
    // native: { "minidump_ref": "<spool filename>", "faulting_module": "<optional, best-effort>" }
    // js:     { "stack": "...", "message": "...", "file": "...", "line": 0 }
    // panic:  { "message": "...", "backtrace": "..." }
  }
}
```

- `server_id` is a stable, hashed, non-PII identifier so the backend can group by server.
- On the native kind the minidump is uploaded alongside the envelope (multipart) and referenced by
  `detail.minidump_ref`.

### 6.6 Spool + uploader (upload-on-next-boot)

- A hard crash cannot reliably upload from inside the dying process, so the handler only **writes
  files** (minidump + `.s2meta`) to a crash-spool directory.
- A **boot-time sweep** — plus a periodic sweep for the JS/panic kinds, whose process is still alive —
  renders each spooled incident into an envelope and uploads it via the existing tokio runtime +
  reqwest with retry/backoff, marking each done (delete or move to a `sent/` dir) on success. Crash-safe
  by construction: an incident survives a total process death and uploads on next boot. (Accelerator
  used the same next-start upload model.)

### 6.7 Config, opt-in, privacy

- Operator config file, e.g. `addons/s2script/configs/crashreporter.json`:
  - `enabled` (default **false** — opt-in),
  - `endpoint` (default the project's central service),
  - `api_key` (issued by the central service, ties a server to an operator),
  - `include_minidump` (allow envelope-only mode),
  - privacy toggles (e.g. scrub map/player counts).
- Runtime infra, so operator-configured, not plugin-permissioned.
- **Privacy / trust burden of a central service:** minidumps can contain process memory (IP strings,
  buffers). Default opt-in; document exactly what is collected; support envelope-only (no minidump).

---

## 7. Sub-project 2 — central backend (sketch; own spec)

Authenticated ingest (api key) → **symbolication** against a per-build symbol store → **grouping /
dedup** by signature (faulting module+function for native; JS-stack signature for js; panic signature
for panic) with occurrence + affected-server counts and first-seen build → **treadmill correlation**
("crashes spiking on CS2 build N; gamedata stale") → **dashboard**: per-operator view (their servers
only, scoped by api-key/org) and a project-wide fleet view. CS2 game frames stay module+offset (Valve
strips symbols) — still enough to answer "s2script vs game." Storage: minidumps in object storage,
envelopes in Postgres. Backend tech and the dashboard UI are pinned in spec #2 (the dashboard is where
a visual companion earns its keep).

## 8. Sub-project 3 — symbol pipeline (sketch; own spec)

A release CI step runs `dump_syms` over `s2script.so` and `libs2script_core.so` for every shipped
build and pushes the Breakpad symbol files to the backend's symbol store, keyed by build id, so native
minidumps are symbolicatable — including re-symbolicating months-old crashes.

## 9. Degrade-safety principles

- **The handler must never cause or worsen a crash.** Everything reachable from the signal handler is
  async-signal-safe, bounded, and chains to the previous handler so core dumps and the game's own
  crash handling still work.
- **Fail-off, never fail-loud.** If Breakpad init fails, or the spool dir is unwritable, or config is
  malformed, the runtime boots and runs normally with crash reporting simply disabled.
- Core must not import `games/*` (existing boundary gate applies).

## 10. Testing strategy

- **Deliberate-crash harness (dev-only):** a gated trigger to raise SIGSEGV / null-deref / abort /
  Rust panic / JS throw on command, exercising each capture path; assert a well-formed minidump +
  `.s2meta`, and a breadcrumb naming the right plugin + dispatch.
- **Signal-safety audit:** enumerate every call reachable from the handler; assert async-signal-safe
  only.
- **Cargo unit tests (core):** envelope serialization/round-trip, breadcrumb ring buffer, dedup
  signatures, boot-sweep spool logic, uploader retry/backoff.
- **Live gate (Docker CS2, per the project cadence):** trigger a real shim fault → confirm minidump +
  breadcrumb name the culprit → confirm upload-on-next-boot → symbolicate server-side and eyeball the
  "deeply relevant" report. Confirm normal boot is unregressed.

## 11. Proposed slices (thin vertical threads → PR stack)

Cut for atomic, independently reviewable PRs (final boundaries set in the plan):

1. **Breadcrumb POD + tracker** — the struct, cheap per-dispatch updates, ring buffer, plugin table,
   the `@s2script/cs2` setter for CS2 build number. Unit-tested; no capture yet.
2. **Rust panic path** — `set_hook` → envelope (kind=panic) written to spool. Smallest capture path,
   no C++ / Breakpad, proves the envelope + spool end to end.
3. **Spool + uploader + config** — boot/periodic sweep, opt-in config, retry upload (to a stub/mock
   endpoint in tests). Proves transport before the backend exists.
4. **Native fault path (Breakpad)** — vendor Breakpad into the shim, arm `ExceptionHandler` +
   `sigaltstack` + chaining, write minidump + `.s2meta`. The riskiest slice; live-gated.
5. **Fatal JS error path** — V8 uncaught/rejection callbacks → envelope (kind=js), dedup/rate-limit.
6. **Deliberate-crash harness + live gate** — the dev trigger and the end-to-end live validation.

Then spec #2 (backend) and spec #3 (symbol pipeline) as their own cycles.

## 12. Success criteria

- A deliberate native fault on the live CS2 dev server produces a minidump + a populated breadcrumb
  sidecar, uploaded on next boot to the (initially stubbed) endpoint.
- A Rust panic that `catch_unwind` would have swallowed produces a reported incident.
- A fatal JS error produces an incident naming the culprit plugin, file, and line.
- Every report carries the treadmill fingerprint (gamedata + CS2 build).
- Normal boot and the existing gate suite are unregressed; core does not import `games/*`.

## 13. Risks & open questions

- **Signal-handler ordering** with the CS2 dedicated server's own handler — Breakpad chains, but the
  ordering must be validated on the live gate (documented integration risk).
- **Breakpad in the sniper/bullseye build** — vendoring a chunky C++ dep into the constrained server
  build; verify it links under Steam Runtime 3 (glibc 2.31) with `--gc-sections`.
- **server_id derivation** — must be stable across restarts and non-PII; exact source TBD in the plan.
- **JS-error "fatal" definition** — confirm which V8 callbacks count and how dedup interacts with the
  degrade-per-descriptor policy; settle in the plan.
