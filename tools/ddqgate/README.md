# ddqgate — live-gate fixtures for the deferred-dispatch queue

Two throwaway plugins that drive the three checks in
`docs/superpowers/specs/2026-08-01-deferred-dispatch-queue-design.md` §5. Not shipped, not workspace
members, not part of the base-plugin suite — they exist so the gate is repeatable rather than a
one-off session.

`ddqgate-a` is the driver: it fires events from inside dispatches, owns the `ddq_*` server commands,
and calls the shim's gated selftest native. `ddqgate-b` is the **second plugin context** and it is
what the gate actually reads — the deferred dispatch has to cross a context boundary to prove
anything, because before this slice a re-entrant dispatch was dropped and `b` simply never ran.

## The one thing to understand first

**Only `ddq_selftest` proves the game-event path.** Everything else in these fixtures was tried on a
live server and does not re-enter:

- **`Events.fire()` from inside a handler** — CS2 does not route a JS-fired event back through our
  listener while core holds the borrow. It arrives synchronously and never defers.
- **`pawn.slay()` from inside a dispatch** (`CommitSuicide` fires `player_death` inline) — the
  `player_death` *does* land a frame later, but from the **engine's own** next-frame delivery, not
  our drain. The tell: the drain runs at the top of `Hook_GameFramePre`, *before* the frame dispatch
  that increments a plugin's counter, so a drained delivery reads the **old** counter value. The
  observed one read the new value.

The genuine trigger is an engine call that fires an event synchronously *inside* the borrow — which
is exactly what A5b's `Respawn`/`TerminateRound` will be, and exactly why the shim still hand-rolls
`s_pendingRespawn`/`s_pendingTerminate` drains for those two today. Until A5b lands there is no
natural trigger, so the shim ships a **synthetic, env-gated** one, in the same spirit as
`S2_DAMAGE_SELFTEST` for the damage detour (combat is un-generatable on a bots-only server; a
re-entrant game event is un-generatable on any server before A5b).

`ddq_fire` / `ddq_slay` / `ddq_throw` are kept because they are the honest negative controls — they
show the paths that *look* like they should defer and don't.

## `S2_DEFER_SELFTEST` — the gate, and why it is off

Set `S2_DEFER_SELFTEST` (to anything) in the **server process environment**. It arms two things:

1. core installs the `__s2_defer_selftest` native on every plugin context. Without the variable the
   property does not exist on the global at all — this is registration-gating, not call-gating, so
   there is no reachable path to the synthetic re-entrancy in a production process. Pinned by
   `defer_selftest_native_exists_only_when_the_env_var_is_set` (core/src/ffi.rs) and by
   `scripts/check-defer-selftest-gate.sh`.
2. the shim's op stops refusing, and the drain's replay + free start printing a transcript.

**Do not arm it in production.** It dispatches a real catalog event name (`player_changename`) with
fake field values to every subscribed plugin.

## Building them

`s2s build` walks up to the nearest workspace root and builds the **whole workspace**, ignoring the
directory you are standing in. `tools/` is not in the `workspaces` globs, so building these in place
silently rebuilds the base plugins instead. Stage them outside the repo with a symlinked
`node_modules` and pass the dir explicitly:

```bash
STAGE=$(mktemp -d)
cp -r tools/ddqgate/ddqgate-a tools/ddqgate/ddqgate-b "$STAGE/"
ln -s "$PWD/node_modules" "$STAGE/node_modules"
for p in ddqgate-a ddqgate-b; do
  node node_modules/@s2script/sdk/dist/cli.js build "$STAGE/$p" --packages-dir "$PWD/packages"
done
# → $STAGE/<p>/dist/_ddqgate_<x>.s2sp
```

## Running the gate

The queue's drain lives in `Hook_GameFramePre`, which is installed **lazily** when a plugin
subscribes to `OnGameFrame`. Both fixtures call `ctx.server.onGameFrame`, so deploying them is what
installs it — a server with neither would never drain.

Deploy **only** these two into `plugins/` (hold the rest aside so the counters are unambiguous), arm
the variable, then cold-restart:

```bash
D=<install>/addons/s2script
mkdir -p "$D/.held" && mv "$D/plugins"/*.s2sp "$D/.held/"
cp "$STAGE"/*/dist/*.s2sp "$D/plugins/"

# Arm it. docker/docker-compose.yml passes the process environment through, so either add
#   S2_DEFER_SELFTEST: "1"
# under the cs2 service's `environment:` and recreate, or export it before `docker start`.
docker stop s2script-cs2 && docker start s2script-cs2   # NOT restart — keeps the stale .so
```

Then drive it:

```bash
python3 scripts/rcon.py "ddq_selftest"          # THE check — read the RCON reply AND the log
python3 scripts/rcon.py "ddq_report"
python3 scripts/rcon.py "ddq_report_b"
docker logs s2script-cs2 --since 2m | grep -E 'defer self-test|deferred-dispatch|DDQ-'
```

## The transcript, in order

One `ddq_selftest` should produce exactly this sequence. Each line is a distinct stage, so a failure
is attributable to one of them rather than to "it didn't work":

```
[DDQ-A]    ddq_selftest #1 at frame=<F> — calling the gated native
[s2script] defer self-test #1: SYNTHETIC dispatch of 'player_changename' (...) from inside a JS native at frame <N> — NOT PRODUCTION DATA
[s2script] defer self-test #1: core reported DEFERRED at frame <N> — queueing
[s2script] deferred-dispatch: DuplicateEvent round-trip verified on the live binary (event 'player_changename') — game-event deferral confirmed
[s2script] defer self-test #1: QUEUED (depth 1) — expect a DRAIN line at frame <N+1> and a FREE line right after it
[DDQ-A]    ddq_selftest #1: native returned 1 — DEFERRED+QUEUED (expect DDQ-B one frame later)
[s2script] defer self-test: DRAIN replaying game_event 'player_changename' at frame <N+1>
[DDQ-B]    selftest #1: userid=1 oldname="ddq-selftest" newname="selftest-1@frame-<N>" deferredAtShimFrame=<N> nowJsFrame=<F> — payload OK
[s2script] defer self-test: FREE duplicate 0x... at frame <N+1>
```

The **round-trip** line is the one that has never appeared on a live server before this: the boot
log has always said `DuplicateEvent = vtable slot 10 … armed (pending the first-duplication
round-trip)` and stopped there, because nothing ever duplicated an event. It appears once per
process, on the first duplication.

What each stage failing means:

| Missing / wrong line | What it means |
| --- | --- |
| `native ABSENT` from DDQ-A | the process was not started with `S2_DEFER_SELFTEST` set |
| `REFUSED — S2_DEFER_SELFTEST is not set` | core's native installed but the shim disagrees — impossible if `check-defer-selftest-gate.sh` passes; suspect a stale `.so` |
| `NOT DEFERRED (dispatch returned 0)` | nothing is subscribed to `player_changename` (is `ddqgate-b` loaded?) or the isolate was free |
| `NOT DEFERRED (dispatch returned <other>)` | the borrow premise broke — the run proves nothing, do not read past it |
| `round-trip FAILED (...)` | `DuplicateEvent`'s vtable slot moved. Game-event deferral degrades by name; scalar deferral keeps working. See `docs/re-strategy.md` |
| `QUEUE REFUSED the entry` | duplication was unavailable or the queue was full — the reason is on the line above |
| no `DRAIN` line | the drain never ran: no `OnGameFrame` subscriber, or a flush (map start) landed in between |
| `DRAIN` but no DDQ-B line | the replay reached the shim but not the subscriber — a core-side fan-out failure |
| DDQ-B line with `ZEROED-PAYLOAD` | the duplicate did not carry the fields; the replay ran against a dead `s_currentEvent` |
| `DRAIN` with no `FREE` after it | the duplicate leaked — the exactly-once free is broken |

**`deferredAtShimFrame` vs `nowJsFrame` is the timing evidence, and it reads backwards.** The shim's
own two frame numbers (`N` at defer, `N+1` at drain) are the direct statement of the one-frame
delivery the spec promises (§4). The *JS* counter deliberately does **not** advance between them:
the drain runs at the top of `Hook_GameFramePre`, before the frame dispatch that increments it, so a
**drained** delivery reads the same `frame` value that was current at defer time. Delta 0 on the JS
side is the drain's signature — an engine-delivered next-frame event would read the incremented one.
That asymmetry is exactly how the `pawn.slay()` attempt was ruled out.

## The negative controls

```bash
python3 scripts/rcon.py "ddq_fire"     # Events.fire from a handler — arrives synchronously, no defer
python3 scripts/rcon.py "ddq_slay"     # CommitSuicide from a handler — engine-delivered, not drained
python3 scripts/rcon.py "ddq_throw"    # 20-event burst; DDQ-B throws on each
```

`ddq_throw` covers spec §5 check 3 (a throwing handler must still free its duplicate) *only while
the selftest path is what queued them* — on today's build these arrive synchronously, so the throw
isolation is exercised but the free path is not. The free path's throwing case is pinned offline
instead, against the shipped code, by `shim/tests/defer_queue_test.cpp` (`scripts/test-defer-queue.sh`).

Restore the held plugins and unset `S2_DEFER_SELFTEST` afterwards.
