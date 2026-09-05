# Mixed-workload live soak

This test fixture drives the final 60-minute gate on the isolated hardening server. It does not
change production source, install artifacts, restart a server, or expose a helper port by itself.
The operator runs it only after Tasks 6–10 have been integrated, rebuilt with the Bullseye sniper
toolchain, installed on `s2script-cs2-hardening`, and restarted under the existing runbook.

Before warm-up, a separate bounded pressure phase submits 256 `threadSleep(750)` operations from
one owner. Under the shipping per-owner job limit it requires at least one named `AsyncQueueFull`,
zero other errors, exact `success + rejected = settled = 256`, an identical native live-gauge
snapshot after quiescence, and an exact match between named rejections and the native
`jobs.rejected` delta. The post-pressure rejection counters become the baseline, so the ordinary
mixed workload must introduce no further admission failures.

The workload then has one warm-up burst, a stable post-warmup baseline, then 57 one-minute measured
phases over the default 3,600-second run. Each phase is bounded to 35 seconds plus 10 seconds of
eventual-idle sampling. A burst runs these components concurrently:

- twelve timer completions: one-shots, immediate cancellations, `delay`, `nextFrame`, and worker
  sleep;
- two 4 KiB HTTP responses streamed in eight delayed chunks by the private helper;
- two 2 KiB framed TCP writes read 128 bytes at a time by the private helper, followed by exact
  byte-count ACKs;
- two SQLite connections to `mixed_live.sqlite`: one takes an exclusive transaction, the other
  must receive `busy`/`locked`, then the fixture rolls back, retries, and reads back `recovered`;
- sixteen `point_worldtext` entities with `SetTransmit` registration, explicit unhook, and removal;
- one atomic edit of the fixture's materialized config and one reload of
  `@s2script/clientprefs`;
- one bounded bot disconnect/replacement attempt. A same-slot candidate must remain the same live
  connection for 250 ms before the fixture exercises stale reads and stale chat/print/command/voice/kick
  actions, then it verifies the replacement identity and voice state after another 100 ms. A
  provisional quota candidate that disappears during settling remains pending and cannot count as
  proof or failure. Old handles survive across cycles until actual stable same-slot reuse,
  with a hard cap of 64 handles and no expiry timers. The collector's `bot_quota 2` owns replacement;
  the fixture never issues a competing `bot_add`, and at most one verification timer exists.
  Exceeding the cap is a named `SLOT failed ... reason=pending-cap`
  isolation failure; expiry or skipped attempts never count as proof.

Bots are slot-generation actors only. Their presence is not a signed-on viewer count and does not
prove that a CheckTransmit callback ran. The hook workload proves registration/index teardown;
human viewer traffic is a separate live gate.

The engine may choose many different free slots before wrapping (the prior live proof needed
16 controlled bot cycles). The collector permits `slotPending` from 0 through 64 during each
phase, while still requiring at least one successful real same-slot stale-action probe for PASS.
A successful verification clears all retained old handles and slot timers. Run-start
`sm_mixed_slotreset` clears old proof counters as well as handles/timers, so a prior pilot cannot
satisfy a new run. End/exception `sm_mixed_slotcleanup` clears handles/timers while preserving
this run's counters. The collector requires its empty-state ACK, allows quiescence, then makes
the final native-gauge comparison. A short pilot that ends before any same-slot reuse fails
that proof requirement; it does not manufacture success from cleanup.

## Build after integration

From `/home/ghirakawa/s2script-hardening` on Nebula, after the integrated checkout is present:

```sh
cd packages/sdk && npm run build && cd ../..
node packages/sdk/dist/cli.js build \
  tools/hardening-live/mixed-live
```

The archive is:

```text
tools/hardening-live/mixed-live/dist/_fixture_mixed-live.s2sp
```

Copy it into the integrated `dist/addons/s2script/plugins/` before the final test install
and restart of `s2script-cs2-hardening`. Keep the existing client-live and memory-live fixtures
installed. With 14 base plugins plus those three fixtures, the expected running count is 17. If the
installed set differs, record it and pass the exact count with `--expected-plugins`. In particular,
leaving the prior cookie-live fixture installed makes the current total 18; either remove only that
completed scratch fixture before the final restart or pass `--expected-plugins 18`.

After startup, confirm the runtime materialized:

```text
dist/addons/s2script/configs/_fixture_mixed-live.json
```

The collector refuses to start if that file is absent. It saves the exact original bytes, uses
mode-preserving atomic replacements during the gate, and restores the original content on every
normal or exceptional exit. Before the first workload edit it verifies the original generation is
already applied through the fixture's `STATUS config=` value. During normal cleanup it requires that value
to return to the original generation and waits for loader retirement before the final hard
deadline. The `finally` path retries the exact-byte restoration if the normal proof does not finish.
Measured generations use compact JSON in a fixed 17-byte capacity;
shorter values are padded with trailing JSON whitespace so generations 9, 10, and 57 have identical
byte and capacity accounting. The report records the original retained loader plateau separately
because an operator's original config can have a different length. A value that exceeds that
capacity fails explicitly instead of being truncated; at most generation 99 is supported.

## Private helper boundary

By default the collector creates its own container named `s2script-mixed-slow-peer` from
`python:3.12-alpine`. It joins only `s2script-cs2-hardening_default`, mounts this fixture directory
read-only, and publishes no host ports. It verifies the empty public-port map and exact network
membership before touching the workload. It refuses to replace a pre-existing container with that
name and stops only the helper it started. No `-p`, host networking, or production container is
used.

Pre-pull the helper image outside the timed gate if needed:

```sh
docker pull python:3.12-alpine
```

## No-server checks

`sm_mixed_stats` uses a bounded multipart reply because Source console/RCON truncates long command
replies. The fixture serializes the native ASCII JSON from `__s2_async_stats()` exactly once, caps
the snapshot at 65,536 bytes, and emits at most 64 replies with at most 1,024 bytes of JSON data each:

```text
[mixed-live] STATS_PART snapshot=<id> part=<index>/<count> bytes=<total> data=<json-fragment>
```

The collector accepts exactly one snapshot ID, consistent count and byte declarations, and every
part from 1 through `count` exactly once. It rejects malformed, missing, duplicate, mixed,
oversized, or length-mismatched parts before JSON parsing, then applies the unchanged frozen native
stats schema and limit checks to the reassembled object.

Run these locally and again on the integrated checkout:

```sh
python3 tools/hardening-live/mixed-live/test-mixed-soak.py
python3 -m py_compile \
  tools/hardening-live/mixed-live/mixed-soak.py \
  tools/hardening-live/mixed-live/slow-peer.py
node packages/sdk/dist/cli.js build \
  tools/hardening-live/mixed-live
```

The Python suite uses real loopback HTTP/TCP servers and a real timed subprocess. It also runs
`test-slot-protocol.cjs`, which bundles the actual fixture with the local esbuild dependency and
executes it against a deterministic client-generation model: 16 different free slots at one-minute
intervals, then actual slot reuse and stale actions, plus count-cap, cleanup and fresh-run reset
checks. Its deterministic executable-main model includes transient loader work, malformed loader
metrics, over-cap/high-water samples, a retained lease leak, a timed-out stats read, and runtime
config-restoration failure. It detects an
incomplete native schema, malformed component counts, resource/queue/stage/cache growth, admission
rejection deltas, incomplete or incorrectly classified pressure settlement, wrong reload counts,
missed same-slot reuse, output that bypasses a timeout,
oversized helper input, and wrong slow-peer payload/ACK behavior. It never connects to CS2.

## Pilot, then the 60-minute gate

Do not overlap another gate on RCON 27016. A short pilot exercises the same flow but is evidence
only for harness compatibility:

```sh
python3 tools/hardening-live/mixed-live/mixed-soak.py \
  --root /home/ghirakawa/s2script-hardening \
  --source <integrated-commit> \
  --duration 300 --warmup 120 --cycle-period 60 --cycle-timeout 35 --quiescence 10
```

Run the acceptance soak with the defaults:

```sh
python3 tools/hardening-live/mixed-live/mixed-soak.py \
  --root /home/ghirakawa/s2script-hardening \
  --source <integrated-commit>
```

Every RCON call is a separate invocation of the repository's proven `scripts/rcon.py`, capped at 15
seconds and 256 KiB of output. Each cycle's calls are further capped by its remaining 35-second
budget, and no measured cycle starts at or after the soak deadline. The whole run gets only a
90-second final cleanup and collection allowance. Loader idle sampling caps each stats subprocess
to the remaining sampling deadline as well as the global deadline, and a response returned after
its deadline is retained as evidence but cannot satisfy the proof. When the collector owns the slow
peer, it reserves the last 10 seconds exclusively for one bounded `docker stop` attempt and stops
ordinary calls at the reserve boundary. An exceptional external-call overrun still gets one fresh,
bounded cleanup-only attempt; the run is `INCOMPLETE` and reports its cleanup overrun rather than
silently orphaning the helper. Before config edits, helper startup, or RCON, the collector
requires `--source` to equal the checkout's full `git rev-parse HEAD`. A timeout is `INCOMPLETE`; a
completed protocol mismatch is `FAIL`.

`PASS` requires all 57 measured cycles, one ACK and one subsequent `Active` line for every reload
(including warm-up), zero component errors, at least one verified same-slot replacement, an
acknowledged empty slot-probe cleanup, exactly 17
running plugins at the end, no new native rejection counters, and exact return to the baseline for:

- `items` and `bytes` in jobs, completion, sockets, SQLite, pools, inbound, outbound, and timers;
- worker/HTTP/DB/WS/net queue counts;
- staged timer/WS/net/cookie/HTTP/DB counts;
- cookie cache accounts, entries, and bytes.

The parser also requires the complete frozen `loader` schema, positive effective limits, and
non-negative integer gauges, rejections, and high-water values. It rejects any current charge or
high-water above its effective cap. Shared loader obligations are checked once against both item
caps; queued, in-flight, and result rows locate those obligations and are not summed. Config paths
use the shared union cap, and baseline/proposal high waters are not summed across time. Effective
loader limits must remain identical, and every high-water field must be nondecreasing across all
valid samples for the continuously running worker lifecycle.

Quiescence is eventual rather than a single sleep and snapshot. A sample is idle only when worker
obligations, queued/in-flight/results, pending controls, proposals, main pending rows,
active/ready/waiting/applying rows, and retained leases are all zero. `applying` locates the lease
temporarily popped into a re-entrant lifecycle application. Periodic watcher work may therefore be
visible in an intermediate saved sample. The watcher control charge and committed config baseline
persist; after warm-up their controls, config union, and baseline gauges must return to the same
measured plateau after every fixed-size generation. Once a prior quiescent snapshot exists, a sample
is accepted only when these loader conditions and every existing non-loader leak gauge both match;
named pressure rejection deltas remain a separate check. The original-to-warm transition allows the
config and baseline byte gauges to change only by the exact difference between the original payload
length and the fixed 17-byte generation payload; all other persistent loader gauges remain exact.
Final cleanup restores the original config,
proves the original generation was applied, reaches idle, and returns those retained gauges to the
separately captured original plateau. `loader.running` must remain true, loader rejection counters
must not increase, and every captured loader high-water remains within its reported limit.

The parser requires the current `__s2_async_stats()` shape, including the `frame` key, `lastNs`,
`maxNs`, `timerExamined`, and the current `staged.http`/`staged.db` additions. `frame` may be null
before the runtime has produced its first instrumented frame; when present, it must contain valid
`items`, `bytes`, and `polls` counters. The collector records per-cycle rejection, drain-time,
timer-examined, and whole-container RSS values, plus nearest-rank p50/p95/p99/max summaries.
`frame`, `lastNs`, `maxNs`, and rejection counts are cumulative or instantaneous measurements
rather than leak gauges; admission deltas are a separate workload failure. RSS is the sum of all
container process RSS and is recorded for context only. Allocator retention alone is not evidence
of a leak or a memory bound.

Artifacts go under `.gate/mixed-soak/mixed-soak-<UTC>/`: manifest, every bounded RCON/log output in
a monotonically numbered exclusive file,
native JSON snapshots, per-cycle comparisons, RSS snapshots, and the final report. The report must
be published with the integrated commit, Nebula machine identity, map and actual `status` player
rows, exact workload values, cycle count, ACK/Active counts, errors, final plugin state, and the
measurement summary. The harness makes no claim about human continuity, authenticated same-account
reconnects, rendered chat/console delivery, CheckTransmit callback traffic, or core-only RSS.

Original materialized configs are JSONC. The collector uses the repository’s shared string-aware comment stripper only for reading the generation; it retains and restores the original UTF-8 bytes exactly. Both generated-comment configs and plain JSON are covered by tests.
