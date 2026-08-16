# AGENTS.md

Project overview, conventions, and current status live in [`CLAUDE.md`](CLAUDE.md). Build/run
instructions live in [`README.md`](README.md) and the [`scripts/`](scripts) directory. This file
only records durable, non-obvious guidance for agents working in a cloud VM.

## Cursor Cloud specific instructions

**Scope:** the standard cloud dev loop here is the **TypeScript / npm** side — the `@s2script/*`
type packages and CLI (`packages/*`) and the plugins (`plugins/*`, `examples/*`). This is pure
Node.js; `npm install` (the update script) is the only dependency step. The native build (the Rust
V8 core in `core/`, the C++ Metamod shim in `shim/`, and the `third_party/*` git submodules) is a
separate, heavier toolchain that is **not** part of this loop — build it only when a task actually
touches `core/` or `shim/` (see [`docs/BUILDING.md`](docs/BUILDING.md)), or when driving the live
CS2 gate (below). If Docker is not present in the VM, the last gate in `scripts/ci-js.sh`
(`test-gate.sh`) fails on `docker: command not found` — that failure is benign for the pure npm loop.

**CLI + plugins (the main workflow):** the CLI is `@s2script/sdk` (the `s2s` bin), in
`packages/sdk` — there is no `packages/cli`.
- Build the CLI once: `cd packages/sdk && npm run build` → `packages/sdk/dist/cli.js` (a
  `node build.mjs` esbuild bundle, not `tsc`).
- SDK/CLI unit tests: `cd packages/sdk && npm test` (node `--test`, ~528 tests).
- Full JS gate (the CI job): `bash scripts/ci-js.sh` (or `CI=1 make ci-js` to add the `npm ci`
  lockfile guard). Everything passes except the Docker-only `test-gate.sh` at the end.
- Build all base plugins to `.s2sp`: `bash scripts/build-base-plugins.sh`.
- Typecheck gate for every plugin/example: `bash scripts/check-plugins-typecheck.sh`.
- New plugin: `node packages/sdk/dist/cli.js create <dir> --game cs2 --name <pkg>` then build it.
  `s2s build` externalizes `@s2script/*` (host-injected at runtime) and runs a strict `tsc`
  typecheck + eslint gate before emitting the archive. For a plugin scaffolded OUTSIDE the repo
  (e.g. under `/tmp`), `npm install` its `file:` deps with `--ignore-scripts` — the linked local
  `@s2script/sdk`'s `prepare` script otherwise trips `npm error ELOOP` — then `npm run build`.
  Inside the repo workspace, base plugins/examples resolve `@s2script/*` through the root
  `node_modules` symlinks, so a plain root `npm install` is enough.

**Live CS2 gate (running s2script on a real server in the VM).** This IS supported in the dev VM —
s2script is a Metamod plugin, so any behavior involving the engine (hooks, entities, commands,
players) can only be truly tested here. The environment is wired up in
[`.cursor/environment.json`](.cursor/environment.json): `install` →
[`scripts/cloud/install.sh`](scripts/cloud/install.sh) (idempotent: npm workspaces + the whole live
gate below), `start` → [`scripts/cloud/start.sh`](scripts/cloud/start.sh) (per-boot: start dockerd,
chown `cs2-data`, `compose up -d`), plus a `cs2-server` terminal tailing the server log. Both are
safe to run by hand. The pieces they automate (heavy, captured in the VM snapshot, NOT re-run per
boot):
Docker (no systemd — start the daemon by hand: `sudo dockerd &`, storage-driver `fuse-overlayfs`,
`iptables-legacy`); `git submodule update --init --recursive`; the sniper build for **loadable**
binaries (a host `cargo build` links `GLIBC_2.34+` and Metamod refuses it — use
`sudo docker run --rm -v "$PWD:/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`,
which packages `dist/addons/s2script` with `.so`s needing ≤ GLIBC_2.31); base plugins dropped in
(`bash scripts/build-base-plugins.sh` then `cp plugins/*/dist/*.s2sp dist/addons/s2script/plugins/`);
and Metamod:Source for CS2 in `docker/metamod/` (the `mmsource-*-linux.tar.gz` unpacks to
`addons/metamod/*` and must contain `bin/linuxsteamrt64/metamod.2.cs2.so`).

Non-obvious gotchas for the live gate:
- **`docker/cs2-data/` must be owned by uid 1000 BEFORE first boot.** The `joedwards32/cs2` image
  runs srcds as `steam` (uid 1000); a root-owned bind mount makes `entry.sh` die writing
  `post.sh`/`cfg/` (`./cs2.sh: No such file or directory`). `sudo chown -R 1000:1000 docker/cs2-data`.
- First boot pulls a **~71 GB** CS2 download into `docker/cs2-data/` (persists across restarts via
  the bind mount). `gameinfo.gi` self-heals on every boot via `docker/pre.sh`.
- Bring it up: `sudo docker compose -f docker/docker-compose.yml up -d`; watch
  `sudo docker logs -f s2script-cs2` for `[plugins] '@s2script/...' Active`.
- Drive it over RCON (`127.0.0.1:27015`, pw `s2script`): `python3 scripts/rcon.py "meta list"`
  (should list `s2script`), `"sm_slap <name> <dmg>"`, etc. `S2_DAMAGE_SELFTEST=1` (compose) fires a
  synthetic damage hook every few hundred frames as a built-in liveness proof.
- Bots on this headless LAN server get kicked unless you set `bot_quota_mode normal`,
  `bot_join_after_player 0`, `mp_limitteams 0` before `bot_add` / `bot_quota N`.
- After rebuilding `core`/`shim`, re-run the sniper build + `scripts/package-addon.sh`, then
  `sudo docker compose -f docker/docker-compose.yml restart cs2` (**restart**, not
  `--force-recreate`, which resets `gameinfo.gi`).

**If you do need the native build** (touching `core/` or `shim/`), two non-obvious gotchas:
- Rust: the committed `Cargo.lock` pins edition-2024 crates (e.g. `zeroize 1.9.0`) needing Rust
  ≥ 1.85. If `cargo build` fails with `feature 'edition2024' is required`, run
  `rustup toolchain install stable && rustup default stable`.
- Shim: needs `git submodule update --init --recursive`, and must be built with **g++, not the
  default clang** (clang auto-selects the GCC 14 dir, which lacks `libstdc++.so`, so a clang link
  fails with `cannot find -lstdc++`):
  `cmake -S shim -B build/shim -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++ && cmake --build build/shim -j`.
- Core tests: `cargo test -p s2script-core` (`.cargo/config.toml` already forces single-threaded).
