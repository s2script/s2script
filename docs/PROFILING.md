# Profiling a server: `sm prof`

`sm prof` measures what s2script does on the game thread: each plugin, each callback, the
runtime itself and V8's garbage collection. It answers two questions:

1. **Was it s2script?** The server logged `UNEXPECTED LONG FRAME`, or players felt a hitch. Did
   s2script cause it, or did the engine, the map or the host?
2. **If so, which plugin, and which callback or function?**

It is the counterpart of SourceMod's `sm prof`. It runs from the server console or RCON only. A
player's `sm prof` prints the version banner, like any other `sm` command a player types.

## Commands

| Command | What it does |
|---|---|
| `sm prof start` | Start recording. Runs until `sm prof stop`. |
| `sm prof start 120` | Record for 120 seconds, then stop and print the report. |
| `sm prof start js` | Also sample JavaScript, to see the slow *function* and line in a plugin. Combine with a duration: `sm prof start 60 js`. |
| `sm prof stop` | Stop, print the report and write the files. |
| `sm prof status` | Show whether a session is running, how long it has run and how many frames it has seen. |
| `sm prof dump [rows]` | Print the last report again, with `rows` lines per table (default 15). |
| `sm prof clear` | Forget the last report. |

To catch an intermittent hitch, start a timed session, wait for the hitch, and read the report:

```
sm prof start 600
// ... wait until the server logs UNEXPECTED LONG FRAME ...
sm prof stop
```

## Reading the report

```
---- s2script profile: 10m00s from 12:01:00.000Z, 38400 frame(s) ----
Frame interval: avg 15.63ms  p50 15.62ms  p99 16.41ms  max 70.53ms  long (>= 50ms): 1
s2script used 2.41s of 10m00s wall time (0.4%); most in one frame: 1.92ms
Verdict: the longest frame (70.53ms) was NOT s2script — it ran 0.11ms of it. Look at the engine, the map, or the host (CPU steal, I/O).
Longest frames (engine frame-to-frame time) and s2script's share:
  frame   32317 at 12:09:24.953Z  interval   70.53ms  s2script   0.11ms (0.2%)  GameFrame 0.42ms  [@s2script/clientprefs 0.04ms]
  ...
By owner (self time — excludes callees, other plugins and GC):
        self  % wall     calls  owner
    912.40ms    0.2%    76800  @s2script/zones
    ...
Top callbacks by self time:
        self     calls       avg       max  owner :: what
    905.10ms     38400    0.02ms    0.31ms  @s2script/zones :: OnGameFrame (post)
    ...
```

- **Frame interval** is the time from one server frame to the next. At 64 tick it is about
  15.6 ms. A long frame is one whose interval is at least 50 ms.
- **s2script's share** of a frame is all the time s2script spent in that frame: plugin callbacks,
  the runtime's own work and garbage collection. If a 70 ms frame has 0.1 ms of s2script in it,
  s2script did not cause it. The **Verdict** line applies this to the longest frame.
- **GameFrame** is the server's own simulation in that frame, from s2script's pre hook to its post
  hook. A long interval with a short GameFrame means the time was spent outside the simulation:
  the engine between frames, or the process waiting for CPU.
- **Owners.** A plugin id is that plugin's code. `s2script (framework)` is the runtime's own work:
  dispatch, the async drain, the plugin loader. `v8 (garbage collection)` is V8's garbage collector,
  which all plugins share.
- **Self time** is a callback's time minus anything that ran inside it and was charged to someone
  else: another plugin's interface it called, or a garbage collection. Natives a plugin calls (ray
  traces, entity reads) count as the plugin's own time.
- **async continuation (await / .then)** is a plugin's code that ran after an `await` or in a
  `.then`. It is charged to the plugin that created the promise.
- **Clock times** (`12:09:24.953Z`) are UTC, to line a frame up with the server log.

## The files

When a session stops, s2script writes three files to `addons/s2script/data/profiles/`. A separate
thread writes them, so writing never stalls the server.

| File | Open it with |
|---|---|
| `prof-<time>.txt` | Any text editor. The report as printed (plus the JS table, with `js`). |
| `prof-<time>.trace.json` | <https://ui.perfetto.dev> (drag the file in) or `chrome://tracing`. A timeline of every span, nested as a flame chart, with tracks for frame interval and s2script time per frame, and markers on long frames. |
| `prof-<time>.cpuprofile` | Only with `js`. Chrome DevTools → Performance → *Load profile*, or <https://www.speedscope.app>. V8's sampling profile: the functions that ran, with file and line. |

Plugin functions in the `.cpuprofile` are listed under `s2s://<plugin id>/plugin.js`, so you can
tell which plugin a function belongs to. Stack traces in error messages use the same name.

`sm prof dump` also prints the hottest JS functions once the writer has finished (a moment after
`sm prof stop`).

## What it costs

- **Not profiling:** nothing measurable. Each instrumented point checks one flag.
- **Profiling:** two clock reads per callback, plus some bookkeeping. While a session runs, every
  garbage collection and every async continuation is also timed, which slows promise-heavy code a
  little.
- **`js`:** V8 interrupts the game thread every 250 µs to take a sample. Use it for a short
  session (a minute or two) once the span report has shown which plugin to look at.
- **Memory:** the timeline for the trace file keeps up to 1,000,000 spans (about 32 MB). After that
  the totals keep counting and the ten worst frames are still kept in full, but the trace stops
  growing. The report says when that happened.

## What it does not see

- Engine work outside s2script: the simulation itself, networking, the map. The frame interval
  shows that this time exists; s2script cannot break it down.
- The Metamod shim's own C++ work around a dispatch (hook trampolines, the entity listener). It is
  small, and anything it does in the core is counted.
- **Hibernation.** An empty server with `sv_hibernate_when_empty 1` can stop running frames, so a
  session may record few or none. The report says when it saw none. Set
  `sv_hibernate_when_empty 0` to profile an empty server.
