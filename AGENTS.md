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
touches `core/` or `shim/` (see `README.md` §"Reproduce from scratch"). There is also no live CS2
gate in the VM (no Docker); in-engine / "human-client" behavior stays deferred.

**CLI + plugins (the main workflow):**
- Build the CLI once: `cd packages/cli && npm run build` → `packages/cli/dist/cli.js`.
- CLI unit tests: `cd packages/cli && npm test` (node `--test`).
- Build all base plugins to `.s2sp`: `bash scripts/build-base-plugins.sh`.
- New plugin: `node packages/cli/dist/cli.js create <dir> --game cs2 --name <pkg>` then
  `node packages/cli/dist/cli.js build <dir>`. `s2script build` externalizes `@s2script/*`
  (host-injected at runtime) and runs a strict `tsc` typecheck gate before emitting the archive.
- Typecheck gate for every plugin/example: `bash scripts/check-plugins-typecheck.sh`.

**If you do need the native build** (touching `core/` or `shim/`), two non-obvious gotchas:
- Rust: the committed `Cargo.lock` pins edition-2024 crates (e.g. `zeroize 1.9.0`) needing Rust
  ≥ 1.85. If `cargo build` fails with `feature 'edition2024' is required`, run
  `rustup toolchain install stable && rustup default stable`.
- Shim: needs `git submodule update --init --recursive`, and must be built with **g++, not the
  default clang** (clang auto-selects the GCC 14 dir, which lacks `libstdc++.so`, so a clang link
  fails with `cannot find -lstdc++`):
  `cmake -S shim -B build/shim -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++ && cmake --build build/shim -j`.
- Core tests: `cargo test -p s2script-core` (`.cargo/config.toml` already forces single-threaded).
