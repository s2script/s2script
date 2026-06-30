# s2script

## Building the Rust core

```bash
cargo build --release          # builds libs2script_core.so (cdylib, V8 embedded)
cargo test -p s2script-core -- --test-threads=1
```

`--test-threads=1` is required: the V8 platform is process-global and
initialized exactly once, so parallel tests race that init.

The `v8` crate is pinned to **149.4.0** because its prebuilt binary was compiled
with `v8_monolithic_for_shared_library=true`, which is required to link V8 into a
`-shared` object (our `dlopen`'d Metamod plugin `.so`). The stock `v8 = 150.0.0`
prebuilt uses local-exec TLS and fails to link a cdylib with
`R_X86_64_TPOFF32 ... cannot be used with -shared`. To move to v150+, build from
source: `V8_FROM_SOURCE=1 GN_ARGS=v8_monolithic_for_shared_library=true cargo build`.

## Vendored SDKs (hl2sdk, Metamod:Source)

Two upstream SDKs are vendored as pinned git submodules under `third_party/`:

| Submodule | Remote | Branch | Pinned SHA |
|---|---|---|---|
| `third_party/hl2sdk` | https://github.com/alliedmodders/hl2sdk | `cs2` | `9ab16fa9fcdeeb30565dfdbf6fbb312356978a0b` |
| `third_party/metamod-source` | https://github.com/alliedmodders/metamod-source | `master` | `a5f4cca5824c0c5f13e8fa100dd15df164d2db22` |

Note: the upstream metamod-source repo has no `dev` branch; `master` is the active development branch.

### Fresh checkout

```bash
git submodule update --init --recursive
```

### Updating a submodule to a new upstream commit

```bash
git -C third_party/hl2sdk fetch
git -C third_party/hl2sdk checkout <newsha>
# then stage and commit the submodule pointer bump:
git add third_party/hl2sdk
git commit -m "chore: bump hl2sdk to <newsha>"
```

Same pattern applies for `third_party/metamod-source`.

### Patch workflow (hl2sdk)

hl2sdk occasionally lags Valve SDK updates, so we carry local patches ahead of upstream.

- Make changes directly in `third_party/hl2sdk`.
- Export the patch: `git -C third_party/hl2sdk diff HEAD > patches/hl2sdk/NNNN-description.patch`
  Note: use `diff HEAD` to capture both staged and unstaged changes; otherwise staged hunks may be silently dropped.
- On a fresh checkout, patches in `patches/hl2sdk/` are re-applied in order via `make apply-patches` (added when the first patch is needed).
- Each patch is reviewed and tracked in the update-day fire drill.
