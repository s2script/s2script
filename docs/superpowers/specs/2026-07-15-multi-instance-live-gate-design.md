# Multi-instance live gate — one CS2 server per worktree, one shared install

**Date:** 2026-07-15
**Status:** design approved, plan pending
**Scope:** `docker/` + `scripts/` only. No core, shim, op, `packages/*`, or plugin change — therefore
no sniper rebuild and no changeset.

## The problem

Every slice ends at a live gate, and there is exactly one CS2 server. Parallel worktrees
(`s2script-sound`, `s2script-usercmd`, `s2script-weapon`, …) serialize behind it: a gate in one
worktree blocks every other. The obvious fix — a second server — is blocked by the install: CS2 is
~74 GB, and nobody wants N copies of it on disk or N steamcmd downloads.

Goal: any worktree can run `scripts/gate.sh up` and get its own CS2 server on its own port, sharing
the single existing install, in seconds, with the primary folder's server untouched.

## The decision: reflink clone per instance

`/home` is **btrfs with reflink support** (measured). A copy-on-write clone of the entire 74 GB
install takes **0.496s and consumes zero real disk** (`df` free space identical before and after; the
`du` 74 G is reflinked extents counted per-file). The install is only 3,251 files, which is why it is
instant.

So each instance gets a **full, independent, writable CS2 install** for free:

```
cp -a --reflink=always docker/cs2-data .gate/cs2-data
```

Nothing is shared between instances, so every concurrency hazard below simply cannot occur. The stock
image runs **unmodified**, and the primary folder keeps `docker/cs2-data`, port 27015, and container
`s2script-cs2` exactly as today. Teardown is `rm -rf`. An instance costs only the blocks it writes —
megabytes.

## Why not the alternatives

Evidence comes from the image's own `entry.sh`, extracted from `joedwards32/cs2` without running it
(`docker create` + `docker cp`; the image has `CMD ["bash","entry.sh"]` and no ENTRYPOINT).

### Rejected: share the one install read-write across containers

This is the intuitive "just mount the same volume" option. It is unsafe, specifically:

- **`entry.sh:30–48` runs `steamcmd +app_update 730` on every container boot, unconditionally.** There
  is no env var to skip it (only `STEAMAPPVALIDATE`, which toggles `validate`). Two containers booting
  run concurrent updates against one `steamapps/`.
- **`entry.sh:38–39` runs `rm -rf "${STEAMAPPDIR}/steamapps"` on retry ≥2.** One instance failing to
  boot deletes the install manifest out from under a *running* server.
- **`entry.sh:60–61,104–123` copies `server.cfg` from `/etc/server.cfg` every boot and seds it in
  place** with that instance's `CS2_SERVERNAME` / `CS2_RCONPW` / `TV_*`. N instances fight;
  last-booter-wins.
- **`entry.sh:140–147` rewrites every `gamemode_*.cfg` on every boot** (`TV_DELAY` defaults to `"0"`,
  which is non-empty, so the branch always fires). Confirmed on disk: all `gamemode_*.cfg` carry the
  last boot's mtime.
- **CS2 writes `backup_round*.txt` at the `game/csgo/` root during play**, plus `rpt/<date>/` — so
  instances interleave in the shared tree.

### Rejected: overlayfs (shared lowerdir + per-instance upperdir)

Works, and the host supports it (kernel has `overlay`; Docker is rootful). But overlayfs requires the
**lowerdir not change while overlays are mounted**, and the primary writes to `cs2-data` on every
boot. The primary would have to stop being a direct-mount server and become an overlay instance —
churning the workflow and runbook — and violating the rule is undefined behavior *in the game
install*, a poor foundation for a gate whose purpose is trusting the result. Reflink gets identical
disk economics with none of the constraint.

### Rejected: shared read-only + selective writable mounts

`entry.sh:51–53` **exits if steamcmd cannot write**, so a read-only install can never boot the stock
entrypoint. Every instance would need a maintained fork of `entry.sh` plus a whitelist of writable
paths that drifts with each game update. Most work, most fragile.

## Architecture

```
docker/cs2-data/                 74 G install — primary's, unchanged; doubles as clone source
docker/metamod/                  MM:S 1403 — primary's; also a clone source
docker/pre.sh                    NEW — boot hook, auto-patches gameinfo (see below)
docker/docker-compose.gate.yml   NEW — today's compose with env-driven paths
.gate/                           NEW, gitignored, per-worktree
  gate.env                         claimed port + instance name (survives up/down)
  cs2-data/                        reflink clone — this instance's private install
  metamod/                         reflink clone (MM writes metamod-fatal.log → not shared :ro)
scripts/gate.sh                  NEW — up / down / destroy
```

### Locating the primary

`docker/cs2-data` and `docker/metamod` are gitignored, so they exist **only in the primary folder** —
a worktree has no copy to clone from. `gate.sh` finds the primary as the repo's **main worktree**:

```sh
PRIMARY="$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")"
```

`--git-common-dir` resolves to the primary's `.git` from any linked worktree, so its parent is the
primary folder. The same fact identifies the primary: in the main worktree `--git-dir` and
`--git-common-dir` are equal; in a linked worktree they differ.

### `gate.sh up`

1. Refuse if `--git-dir` == `--git-common-dir` (i.e. run from the primary) — that's
   `docker compose up` as usual, container `s2script-cs2` on 27015.
2. Derive the instance name from the worktree dir: `s2script-sound` → `s2script-cs2-sound`.
3. Claim the first free port in **27016–27030** — skipping 27015 (primary) and Steam's 27036/27060 —
   under an `flock`, scanning `ss` plus existing containers. Record it in `.gate/gate.env` so it is
   stable across up/down; on a later `up`, re-verify it is still free and re-claim if not.
4. Reflink-clone `cs2-data` + `metamod` from the primary if `.gate/` is empty. **Refuse if
   `cs2-data/steamapps/downloading` or `temp/` is non-empty** (a mid-update clone would capture a
   half-updated tree).
5. Verify `${GATE_S2SCRIPT_DIR}` exists with `configs/` + `data/` present and uid-1000-writable; warn
   if `${GATE_METAMOD_DIR}` has no `bin/` (the silent-failure case).
6. `docker compose -f docker/docker-compose.gate.yml -p <instance> up -d`.
7. Print the port and a paste-ready rcon line.

`gate.sh down` stops the container, keeps `.gate/`. `gate.sh destroy` also removes the clone.

### Automatic gameinfo patching

`entry.sh` **sources `pre.sh` at line 153 — after the steamcmd update (line 45), before srcds launches
(line 203)**. That is exactly the right hook. `entry.sh` has no `set -e`, and the stock `pre.sh` is a
noop echo, so:

```bash
#!/bin/bash
# entry.sh sources this after steamcmd update, before srcds launches.
bash /patch-gameinfo.sh || echo "[s2script] WARN: gameinfo patch failed"
```

mounted through the nested-path trick already used for `configs/`:

```yaml
- ./pre.sh:/home/steam/cs2-dedicated/pre.sh:ro
```

Invoking `patch-gameinfo.sh` as a subprocess rather than sourcing keeps its `exit 1` from killing the
boot. `entry.sh:64–69` only copies its own `pre.sh` if absent, so the mount wins.

**This lands on the primary's compose too.** A CS2 update wipes `gameinfo.gi`, and re-patching it by
hand has been the treadmill's one remaining manual step. It now self-heals on every boot.

### Flexible addon mounting

No new mount points and no override files — the two existing sources become env-driven:

```yaml
- ${GATE_S2SCRIPT_DIR}:/…/csgo/addons/s2script:ro
- ${GATE_S2SCRIPT_DIR}/configs:/…/csgo/addons/s2script/configs
- ${GATE_S2SCRIPT_DIR}/data:/…/csgo/addons/s2script/data
- ${GATE_METAMOD_DIR}:/…/csgo/addons/metamod
```

| Invocation | `GATE_S2SCRIPT_DIR` | `GATE_METAMOD_DIR` |
|---|---|---|
| `gate.sh up` (default) | `<worktree>/dist/addons/s2script` | `<worktree>/.gate/metamod` (clone) |
| `gate.sh up --addons <dir>` | `<dir>/s2script` | `<dir>/metamod` |
| `gate.sh up --s2script <dir>` | `<dir>` | `<worktree>/.gate/metamod` (clone) |

`--addons` accepts any `addons/`-shaped tree (an extracted release zip, another worktree's
`dist/addons`); `--s2script` accepts a bare s2script folder. **`gate.sh` resolves every path to an
absolute one** before writing `.gate/gate.env`, so compose interpolation never depends on the
compose file's location or the caller's cwd. The worktree's `dist/` is already per-worktree for
free — `package-addon.sh` builds relative to cwd.

### Ports

Each instance sets `CS2_PORT` to its claimed host port and maps it **1:1** (`27017:27017/udp` +
`/tcp`) rather than remapping onto a fixed internal 27015. Source 2 serves game traffic, A2S queries,
and RCON on that one port number, and 1:1 avoids any question of the server advertising an internal
port that differs from the reachable one — which matters for A2S self-queries and human `connect
ip:port`.

## Failure modes

| Mode | Handling |
|---|---|
| Clone from a mutating primary | Only `backup_round*.txt` / `rpt/` are written during play — harmless if torn in 0.5s. Mid-update is the real window; `gate.sh` refuses on non-empty `steamapps/downloading` or `temp/`. |
| Port race between worktrees | `flock` around the claim; port pinned in `.gate/gate.env`; re-verified on `up`. Docker refuses a taken bind, so worst case is a clear error. |
| Run from the primary folder | Refused with a message. |
| `configs/`/`data/` missing → Docker creates them root-owned | `package-addon.sh` already `mkdir -p`s them; `gate.sh` verifies and warns. |
| Metamod dir without `bin/` | Warn (silent-failure case). |

**Accepted risk:** if steamcmd fails twice on an instance boot, `entry.sh` does `rm -rf steamapps` and
re-downloads ~74 GB into that clone's *real* disk. It is contained to the one instance (the shared
base is untouched — precisely why we clone) and is the same behavior the primary has today, but with
~10 instances the blast radius multiplies. It is slow enough to notice; the response is
`gate.sh destroy`. Eliminating it requires forking `entry.sh`, which this design explicitly rejects.

**Known minor:** the net-sockets demo self-queries a hardcoded `127.0.0.1:27015` and would hit the
wrong server from inside an instance. Demo-only; not fixed here.

## Update day

Unchanged, minus one manual step. The primary updates `cs2-data` on boot as today; instances clone
from a current base. An instance's own boot also runs steamcmd, so a stale clone self-updates —
costing that clone real disk for the delta. **The rule is therefore destroy and re-clone (0.5s)
rather than update in place.** Order: update the primary, then recreate instances.
`gameinfo.gi` re-patching is now automatic (`pre.sh`). Signature regen and gamedata are unaffected —
they are files in the worktree.

## Supporting changes

- `scripts/rcon.py` gains `--port` (it currently hardcodes `PORT = 27015`), defaulting to 27015 so
  every existing call site keeps working.
- `.gitignore` gains `.gate/`.

## Verification

1. `gate.sh up` in a worktree → distinct port, plugins load, `rcon.py --port <N> "meta list"`
   responds.
2. `df` free space unchanged after the clone (proves CoW sharing).
3. The primary keeps ticking untouched throughout, `RestartCount=0`.
4. Un-patch a clone's `gameinfo.gi` deliberately, reboot the instance, confirm `pre.sh` self-heals it
   and Metamod loads.
5. Two instances up simultaneously on different ports, both responding to rcon independently.
6. `gate.sh destroy` removes the clone; `df` returns to baseline.

## Out of scope (deliberate)

- **DB sidecars per instance.** mysql/postgres stay ordinary services in the primary's compose; DB
  gates use the primary as today. Almost no gate needs SQL.
- **Forking `entry.sh`** to skip steamcmd (see accepted risk).
- **GOTV/`TV_PORT`** — `TV_ENABLE` defaults to `0` and nothing sets it; no 27020 mapping needed.
- **`CS2_RCON_PORT`** — the image's `simpleproxy` sugar is unnecessary when host ports are mapped 1:1.
- Fixing the net-sockets demo's hardcoded self-query port.
