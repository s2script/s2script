# Bounded engine function feasibility probe (pending acceptance)

This is Task 1's isolated proof. It exposes no SDK or production Rust API. The
adapter uses one runtime CIF and four closures for every permitted native vector.
The companion `plugin.cpp` currently contains the independent typed peer fixture
for the engine-free test executable. It is **not yet a deployable Metamod plugin**.

On Linux x86_64, install CMake, a C++17 compiler, autoconf, automake and libtool,
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

Actual runtime output must be reviewed before advancing beyond Task 1. The
CS2/Docker mode intentionally exits 2 until the deployable probe, internal
compatibility fixtures, source-bound bundle, and all live observations exist.
Do not deploy the standalone provider or treat this intentional refusal as green.
