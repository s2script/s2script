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
then embeds the same fresh token/revision in both test `.s2sp` archives. The receipt at
`build/engine-function-live/engine-function-build.json` hashes the entire staged
addon and is written only after both builds and repeated clean-source checks.
The installed official Metamod host is neither built nor replaced.

For an early Linux **compile-only** check using an existing compatible toolchain:

```sh
S2FN_SOURCE_REVISION=$(git rev-parse HEAD) \
S2FN_BUILD_TOKEN=$(python3 -c 'import secrets; print(secrets.token_hex(32))') \
bash tools/engine-function-probe/build-live.sh --probe-only
```

Host CI alone adds `--compile-only` after `--probe-only`: it reports the host
GLIBC requirement while retaining all link/export checks. That explicit mode
produces no deployable receipt. The default command and full bundle still require
GLIBC ≤ 2.31.

The coordinator's operator deploys the bundle's addon files and test VDF while the
server is stopped, preserving existing configs/data/plugins and the official host.
Start it once; verify both s2script and engine_function_probe with `meta list`.
The `@s2script/basecommands` plugin and both `@example/engine-function-acceptance`
and `@example/engine-function-witness` must be loaded. The driver uses
`sm plugins load/unload` to exercise script retirement.
The driver deliberately performs no deployment, native unload, or restart:

```sh
bash scripts/test-engine-function-live.sh --docker docker/docker-compose.yml --rcon scripts/rcon.py --port 27016
```

`--port` selects the RCON port for every driver command; omit it for the existing
default 27015. Values outside 1–65535 are rejected before contacting the server.

The driver checks mapped native inodes, consumer hashes, and both installed fixture
archive hashes against the source-bound bundle, asks the resident witness to create
and own exactly one new bot, runs controlled compatibility signatures,
novel mixed-scalar re-entry, recall and peer suppression, then unloads/reloads only
the fixture `.s2sp`. Five runtime bindings retire through both completion signals
before closure destruction; the independent peer remains resident. Raw RCON, server
logs, map evidence and a fail-closed verdict are saved under `.gate/engine-functions`.
A missing bot pawn or required observation is a failed/pending gate, never a pass.

Real CanAcquire uses the actual semantic resolver and deployed gamedata, with a
verified owned-bot receiver and current index/serial. During one expressly armed
`giveNamedItem`, its PRE makes one deliberate nested runtime Call with the unchanged
borrowed arguments; outer/nested non-skipped completions and peer returns are observed.
The independent idle witness owner records actual public `items.onCanAcquire` and
`items.onCanAcquirePost` deliveries while the stimulus owner is busy. Numeric
operation/invocation markers are scoped around the outer stimulus and deliberate
nested Call, independent of native/public callback ordering. The judge requires
one PRE and POST for each native invocation, matching generation, bot, item,
method and result; absent, stale, ambiguous or wrong markers fail closed. The
witness remains loaded across the stimulus owner's reload, then records its own
teardown. Neither archive exposes raw pointers or serials.
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

Return-phase storage has an additional full-callback/outbound-Call activity lease.
Both provider removal acknowledgements are necessary but not sufficient to collect
while a return callback or ffi_call result handling is active. The pinned UNIX64
libffi continuation reads only cached flags and caller-stack result bytes after
ClosureEntry; it never returns through the allocation or reads its CIF/phase again.
The real-provider regression pauses after provider unlock until both acknowledgements
arrive, and separately pauses after outbound ffi_call, before allowing reclamation.

The witness owns bot creation and cleanup across both stimulus generations. It
requires a settled normal quota and stable fully signed-on baseline clients, saves
the bot settings, and queues bare `bot_add_ct` with no profile/name argument.
Only one newly observed full-sign-on fakeclient with no network address can be
claimed. Its guarded Client handle and userId remain retained in the witness.
Before each acquisition A captures its own guarded handle, B revalidates ownership,
and A uses that captured handle, so slot reuse cannot redirect the operation.
Cleanup uses only the retained Client.kick, never a name selector or bot_kick.
It restores saved quota/settings in the same operation only when baseline identity
and the single owned client remain unambiguous (or creation produced no client).
Stale/ambiguous identities cause no client mutation and leave uncertain settings
for explicit operator recovery; the verdict cannot PASS. Witness teardown also
attempts this guarded cleanup if the driver failed after stimulus-owner unload.
Lifecycle control-flow regression: `node --test tools/engine-function-probe/test_fixture_lifecycle.mjs`.
