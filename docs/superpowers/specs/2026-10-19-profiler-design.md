# `sm prof` — runtime profiler (design)

## Why

An idle CS2 server logged `UNEXPECTED LONG FRAME DETECTED: 70.53ms elapsed, 0.10ms sim time`.
Nobody could say whether s2script caused it. SourceMod answered this with `sm prof`; s2script had no
equivalent. Operators need a command that rules s2script in or out of a hitch and, when it is
s2script, names the plugin and the callback (and with JS sampling, the function and line).

## Requirements

1. One console command family, server console / RCON only: `sm prof start [seconds] [js] | stop |
   status | dump [rows] | clear`.
2. Attribution per owner (plugin id, the framework, V8 GC) and per callback, with self time so
   nested work (interface calls into another plugin, GC inside a handler) is charged to whoever ran.
3. Per-frame accounting: for each engine frame, its interval and s2script's share, so "was it us"
   has a direct answer for the worst frames.
4. Results to read: a console report, plus files for real viewers (Chrome trace JSON for Perfetto /
   chrome://tracing, `.cpuprofile` for DevTools / speedscope).
5. Free when off. Bounded memory when on. Never stall the game thread to write results.
6. Engine-generic: no game names, nothing in a game package.

## Design

**Spans** (`core/src/profiler/mod.rs`). A RAII `Span` guard opened at every place the core runs
work: each C-ABI dispatch entry (`ffi.rs`), each per-plugin JS entry (the two `dispatch.rs`
fan-outs, frame handlers, SDKHooks, timers, interface calls and forwards, TopMenu, Engine.function
hooks, plugin load / config reload / unload), the frame's own phases (async drain, socket callbacks,
loader) and the microtask checkpoint. Off, a span is one thread-local bool load. On, it pushes onto
a stack; closing computes duration and self time, updates `(owner, name)` totals, and records an
event. Guards carry their session and depth, so a guard that outlives a session is inert and a hook
that leaves spans open is unwound by the next close below it.

**V8 hooks** (`v8host.rs`, installed only while recording):
- GC prologue/epilogue callbacks → spans owned by `v8 (garbage collection)`.
- A promise hook (`Before`/`After`) → each async continuation charged to the plugin whose context
  created the promise (read from the context's `PluginId` slot). All continuations otherwise run in
  one shared microtask checkpoint and could not be attributed. rusty_v8's setter cannot remove a
  hook, so the core declares V8's `SetPromiseHook` binding with a nullable signature to remove it.

**Frames.** The profiler keeps the `GameFrame` hook installed (`refresh_detour`) and cuts windows at
each PRE. A window's length is the engine's frame interval; depth-0 spans closed inside it are
s2script's share. The ten longest frames and the ten with the most s2script time are kept with their
top owners and, if they missed the timeline, their full span list.

**JS sampling** (`profiler/js.rs`). rusty_v8 binds the inspector but not `v8::CpuProfiler`, so `js`
creates a `V8Inspector` + session and drives the protocol's `Profiler` domain (250 µs interval).
The inspector exists only during the session. Plugin bundles are now compiled with a script name,
`s2s://<id>/plugin.js`, so samples (and stack traces) name the plugin.

**Control.** `start`/`stop` apply at once when V8 is free (the console command path); otherwise they
wait for the top of the next frame. A timed session stops itself at the end of a frame.

**Output** (`profiler/report.rs`). The console report: frame interval stats, s2script's share of
wall time, a verdict on the longest frame, worst frames, owners, top callbacks. On stop, a thread
writes `data/profiles/prof-<utc>.{txt,trace.json,cpuprofile}`. The `.cpuprofile` is extracted from
the protocol response on that thread too. `sm prof dump` re-prints the last report, with the JS
hot-function table once the writer has produced it.

## Limits

- The shim's own C++ work outside core calls is not measured; engine time is visible only as frame
  interval minus s2script.
- The timeline is capped (1,000,000 spans); totals and worst frames are not.
- Promise hooks and JS sampling add overhead while a session runs; neither exists otherwise.

## Not in this slice

A plugin-facing API for custom spans (`profiler.mark()`), per-native timing, and memory/heap
profiling (`HeapProfiler.takeHeapSnapshot` is reachable the same way) are possible later; nothing
here needs them.
