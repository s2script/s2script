# a4gate — live-gate fixtures for the A4 dispatch chain

Two throwaway plugins that drive the four checks in
`docs/superpowers/plans/2026-07-31-a4-live-gate.md`. Not shipped, not a workspace member, not part of
the base-plugin suite — they exist so the gate is repeatable rather than a one-off session.

`a4gate-a` is the driver (both damage handlers, the entity/event/usercmd/output subscriptions, and the
`a4_relay` / `a4_report` server commands). `a4gate-b` is the second plugin context: it observes the same
channels — which is what makes the damage-collapse result a statement about *cross-plugin* composition
and not just two handlers in one closure — and it is the hot-reload subject.

## Building them

`s2s build` walks up to the nearest workspace root and builds the **whole workspace**, ignoring the
directory you are standing in. `tools/` is not in the `workspaces` globs, so building these in place
silently rebuilds the 18 base plugins instead. Stage them outside the repo with a symlinked
`node_modules` and pass the dir explicitly:

```bash
STAGE=$(mktemp -d)
cp -r tools/a4gate/a4gate-a tools/a4gate/a4gate-b "$STAGE/"
ln -s "$PWD/node_modules" "$STAGE/node_modules"
for p in a4gate-a a4gate-b; do
  node node_modules/@s2script/sdk/dist/cli.js build "$STAGE/$p" --packages-dir "$PWD/packages"
done
# → $STAGE/<p>/dist/_a4gate_<x>.s2sp
```

## Running the gate

Deploy **only** these two into `plugins/` (hold the rest aside so the counters are unambiguous), then
cold-restart so the frame counter resets — the synthetic damage self-test fires at frames 300/900/1800
of the process and nowhere else:

```bash
D=<install>/addons/s2script
mkdir -p "$D/.held" && mv "$D/plugins"/*.s2sp "$D/.held/"
cp "$STAGE"/*/dist/*.s2sp "$D/plugins/"
docker stop s2script-cs2 && docker start s2script-cs2   # NOT restart — keeps the stale .so
```

Then drive it:

```bash
python3 scripts/rcon.py "bot_add"; python3 scripts/rcon.py "mp_restartgame 1"   # spawns + entity churn
python3 scripts/rcon.py "a4_relay"                                              # delayed output test
python3 scripts/rcon.py "a4_report"      # A's counters — read the RCON REPLY, not the container log
python3 scripts/rcon.py "a4_report_b"
touch "$D/plugins/_a4gate_b.s2sp"                                               # check 3, the reload
docker logs s2script-cs2 --since 5m | grep A4GATE
```

Two things that cost time the first run:

- **A server command's `console.log` goes to the rcon reply, not the container log.** `a4_report`
  looks like it did nothing if you `>/dev/null` the rcon output. Anything the handler triggers
  *asynchronously* (the delayed relay output) does land in the container log.
- **An entity created from JS does not fire `onCreate`,** and an input fired from JS does not reach
  `onOutput` — the listener re-enters while core still holds the isolate borrow and is
  `try_borrow_mut`-skipped. `a4_relay` therefore uses `acceptInput(..., delay)` so the engine's own
  I/O queue fires it on a later frame, outside the borrow. Engine-created entities (round restart)
  drive the `onCreate` counters.

Restore the held plugins afterwards.
