# s2script

[![ci](https://github.com/GabeHirakawa/s2script/actions/workflows/ci.yml/badge.svg)](https://github.com/GabeHirakawa/s2script/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@s2script/sdk.svg)](https://www.npmjs.com/package/@s2script/sdk)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

**TypeScript plugins for Source 2 — one runtime, one contract.**

s2script is a plugin framework for Source 2 engine games, loaded via
[Metamod:Source](https://www.sourcemm.net/) — aiming to be what SourceMod is to Source 1: the
single runtime every server plugin loads into. You write TypeScript against one standard library;
the framework owns every engine touchpoint and multiplexes all plugins onto it.

```ts
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("hello", (cmd) => {
    cmd.reply("hello from s2script");
  });
});
```

## → [s2script.com](https://s2script.com)

| | |
|---|---|
| [Docs](https://s2script.com/docs) | Getting started, guides, and the full API reference |
| [Plugins](https://s2script.com/plugins) | The plugin catalog |
| [Download](https://s2script.com/download) | Server runtime releases |

Pre-1.0 and moving. **Linux x86-64 + CS2 only** — Windows is not supported.

## Contributing

```bash
git clone https://github.com/GabeHirakawa/s2script.git
cd s2script
git submodule update --init --recursive
make all
```

> ⚠️ A host build will **not** load on a real server — CS2 servers run under Steam Runtime 3
> (glibc 2.31). Deploy only what `scripts/build-sniper.sh` produces.

Build details, the gate suite, and the Docker live gate: **[`docs/BUILDING.md`](docs/BUILDING.md)**.
The design lives in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md); development history is in
[`docs/PROGRESS.md`](docs/PROGRESS.md).

Work ships as small, independently-reviewable PRs. Each one must pass the gate suite and be safe to
merge on its own.

## License

s2script is dual-licensed **`MIT OR Apache-2.0`** — take whichever you prefer. See
[`LICENSE`](LICENSE), and [`licenses/README.md`](licenses/README.md) for the map.

Two things worth knowing before you fork:

- **The Valve carve-out.** The grant covers s2script's own code. It does not cover the
  Valve Source 2 SDK — `third_party/hl2sdk` ships no license, and the built `s2script.so`
  embeds a few Valve translation units. Same posture as Metamod:Source, SourceMod,
  CounterStrikeSharp and CS2Fixes. Details in
  [`licenses/README.md`](licenses/README.md#the-valve-carve-out).
- **The release zip carries its notices.** `licenses/licenses.txt` is generated from the
  real linked sources (`./scripts/gen-licenses.sh`) and gated for freshness
  (`./scripts/check-licenses-generated.sh`), so it can't quietly go stale on a treadmill bump.
