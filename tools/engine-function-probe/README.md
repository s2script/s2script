# Bounded engine function feasibility probe (pending acceptance)

This is Task 1's isolated proof. It exposes no SDK or production Rust API. The
adapter uses one runtime CIF and four closures for every permitted native vector.
The companion `plugin.cpp` contains independent typed peers for the engine-free
executable. `live_plugin.cpp` is a separate resident Metamod consumer; its Linux
compile and real-server acceptance remain pending.

On Linux x86_64, install CMake, a C++17 compiler, autoconf, automake, libtool and libltdl-dev,
initialize recursive submodules, then run:

```sh
bash scripts/test-engine-function-abi.sh --stock-provider
bash scripts/test-engine-function-v8-adapter.sh --spike --stock-provider
bash scripts/test-engine-function-live.sh --fixture-only --orders peer-first,s2-first
```

The provider target is test-only, EXCLUDE_FROM_ALL, and compiled directly from
unmodified pinned KHook/SafetyHook and the checked-in Zydis amalgamation using the
upstream AMBuilder source inventory. The production shim does not link it.
libffi 3.7.1 is built from pinned source into a private static PIC archive; no
system libffi or shared runtime lookup is used. Bootstrapping happens in a build
copy, leaving the git submodule clean. `--exclude-libs,ALL` hides archive symbols.

Without `--stock-provider`, the ABI script runs portable normalization/stack-bound
checks only. macOS arm64 is unsupported for runtime binding creation. Passing
portable tests never satisfies the provider, V8, peer, or live gates.

The V8 script selects exactly one ignored `v8host` child test and supplies an
absolute native bridge path. Normal core tests skip it. Two real host plugin
contexts receive host generation tokens and test-only package instances with the
same contract. A's private native enters the hooked live target under
`nest::with_outbound`; KHook re-enters real V8 through `CallbackScope` and executes
B's typed wrapper before A resumes. It repeats after unloading/reloading A while
B stays live. The harness does not claim that generic `fan_out_inner` currently
transports typed returns.

Actual runtime output must be reviewed before advancing beyond Task 1.

## Resident live fixture (pending Linux compile and live acceptance)

Build a clean committed checkout using `python3 tools/engine-function-probe/build.py`.
It invokes the existing Bullseye sniper build and separately builds this companion,
then embeds a fresh token/revision in the test `.s2sp`. The receipt at
`build/engine-function-live/engine-function-build.json` hashes the entire staged
addon and is written only after both builds and repeated clean-source checks.
The installed official Metamod host is neither built nor replaced.

For an early Linux **compile-only** check using an existing compatible toolchain:

```sh
S2FN_SOURCE_REVISION=$(git rev-parse HEAD) \
S2FN_BUILD_TOKEN=$(python3 -c 'import secrets; print(secrets.token_hex(32))') \
bash tools/engine-function-probe/build-live.sh --probe-only
```

The coordinator's operator deploys the bundle's addon files and test VDF while the
server is stopped, preserving existing configs/data/plugins and the official host.
Start it once; verify both s2script and engine_function_probe with `meta list`.
The `@s2script/basecommands` plugin must be loaded for `sm plugins load/unload`.
The driver deliberately performs no deployment, native unload, or restart:

```sh
bash scripts/test-engine-function-live.sh --docker docker/docker-compose.yml --rcon scripts/rcon.py
```

The driver checks the mapped native inodes and file hashes against the source-bound
bundle, creates one uniquely named bot, runs controlled compatibility signatures,
novel mixed-scalar re-entry, recall and peer suppression, then unloads/reloads only
the fixture `.s2sp`. Five runtime bindings retire through both completion signals
before closure destruction; the independent peer remains resident. Raw RCON, server
logs, map evidence and a fail-closed verdict are saved under `.gate/engine-functions`.
A missing bot pawn or required observation is a failed/pending gate, never a pass.

Real CanAcquire uses the actual semantic resolver and deployed gamedata, with a
verified owned-bot receiver and current index/serial. During one expressly armed
`giveNamedItem`, its PRE makes one deliberate nested runtime Call with the unchanged
borrowed arguments; outer/nested non-skipped completions and peer returns are observed.
Those are **not** direct counts inside the engine body. Exact original-body counters
are available in the controlled stock-provider targets. The fixture preserves outer
policy/results and never retains argument pointers for replay.

`ignite-scalar-compatibility` is a controlled scalar target only. The actual Ignite
ABI has a by-value Vector tail and is unsupported by the locked scalar-only contract;
its stale legacy example is not counted as an engine migration. No tail truncation
or target-specific ABI thunk is permitted. The separate real-V8 busy-owner gate is
still required; queued `Server.command` actions do not substitute for synchronous V8
re-entry evidence.

Judge rejection tests: `python3 tools/engine-function-probe/test_live.py`.
