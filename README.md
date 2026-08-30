# s2script

[![ci-native](https://github.com/s2script/s2script/actions/workflows/ci-native.yml/badge.svg)](https://github.com/s2script/s2script/actions/workflows/ci-native.yml)
[![ci-js](https://github.com/s2script/s2script/actions/workflows/ci-js.yml/badge.svg)](https://github.com/s2script/s2script/actions/workflows/ci-js.yml)
[![npm](https://img.shields.io/npm/v/@s2script/sdk.svg)](https://www.npmjs.com/package/@s2script/sdk)
[![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

**TypeScript plugins for Source 2 — one runtime, one contract.**

s2script is a plugin framework for Source 2 engine games, loaded via
[Metamod:Source](https://www.sourcemm.net/) — aiming to be what SourceMod is to Source 1: the
single runtime every server plugin loads into. You write TypeScript against one standard library;
the framework owns every engine touchpoint and multiplexes all plugins onto it.

```ts
import { command, hook } from "@s2script/sdk";

export function OnPluginStart(): void {
  command("hello", (cmd) => {
    cmd.reply("hello from s2script");
  });
  hook.client.onFullyConnected((client) => {
    client.chat("welcome");
  });
}
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
git clone https://github.com/s2script/s2script.git
cd s2script
git submodule update --init --recursive
make all
```

> ⚠️ A host build will **not** load on a real server — CS2 servers run under Steam Runtime 3
> (glibc 2.31). Deploy only what `scripts/build-sniper.sh` produces.

Build details, the gate suite, and the Docker live gate: **[`docs/BUILDING.md`](docs/BUILDING.md)**.
The design lives in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md); development history is in
[`docs/PROGRESS.md`](docs/PROGRESS.md).

Work ships as one branch and one PR per slice. Each PR must pass `make ci` and be safe to merge on
its own.

## Examples

Ten worked examples under [`examples/`](examples/), smallest first:

| Example | What it teaches |
|---|---|
| [`hello-plugin`](examples/hello-plugin) | The smallest complete plugin — a command, an event, and surviving a hot reload. **Start here.** |
| [`cookbook`](examples/cookbook) | One file per API under `src/recipes/` — HTTP, websockets, sockets, DB, cookies, menus, sounds, traces, usermessages, and more. Copy a recipe into your own plugin. |
| [`entity-playground`](examples/entity-playground) | Creating, configuring, and watching entities: keyvalue-configured spawns, entity I/O, lifecycle listeners, beams. |
| [`engine-call-demo`](examples/engine-call-demo) | Declaring and calling an engine function the framework doesn't already wrap — plugin-owned gamedata, `permissions`, and a descriptor that's *supposed* to fail, caught by name instead of misbehaving silently. |
| [`greeter-plugin`](examples/greeter-plugin) + [`greeter-consumer`](examples/greeter-consumer) | Two plugins talking over a typed, versioned interface — including an `EntityRef` that stays live across the boundary. |
| [`library-package`](examples/library-package) + [`library-consumer`](examples/library-consumer) | A **library** — build-time code bundled *into* a plugin's `.s2sp`, not loaded by the host itself — and a consumer that calls it, with the vendored copy `s2s add` would produce committed for an offline build. |
| [`workspace-library`](examples/workspace-library) | The other half of the library story: a library declared *inside* a workspace, resolved straight from its sibling source — no vendored copy, no `s2s add`, edits picked up on the next build. |
| [`monorepo`](examples/monorepo) | A workspace of several plugins that build and publish together — a producer, a consumer depending on it with **no hand-copied `.d.ts`**, and a shared library package bundled into both. |

Build any of them with `npx @s2script/sdk build examples/<name>`, then drop the resulting
`dist/*.s2sp` into `addons/s2script/plugins/` on a running server. `npx @s2script/sdk create
--library` scaffolds a new library package; a published one is added to a plugin with `npx
@s2script/sdk add <name>`, which vendors it — see `examples/library-consumer`'s README for why
libraries are vendored rather than npm-installed.

Dev tooling lives in [`tools/`](tools/) — `schema-dump` (regenerates gamedata
after a CS2 update), `s2bench` (op timing), and `crash-test`.

## Workspaces

A directory can hold **several plugins that build, publish, and version together** — a workspace,
marked by an `s2script.workspace.plugins` glob list in its root `package.json` next to npm's own
`workspaces` field. `s2s build`/`s2s deploy`/`s2s version` then operate on every plugin at once, in
dependency order, and a plugin can depend on a sibling's published interface with **no hand-copied
`.d.ts`** — npm already symlinks every workspace member, so the sibling's real contract resolves in
place. This repo's own [`plugins/`](plugins/) tree is one (`s2s build` at the repo root builds all
18 base plugins; `scripts/build-base-plugins.sh` is now a thin shim over it instead of its own bash
loop); see [`examples/monorepo`](examples/monorepo) for a from-scratch one, and
[`packages/sdk/README.md`](packages/sdk/README.md#workspaces) for the full CLI reference.

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
