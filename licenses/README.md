# Licensing

s2script is **dual-licensed `MIT OR Apache-2.0`** — take whichever you prefer.
The root [`LICENSE`](../LICENSE) is the authoritative statement; this file is the map.

| File | What it is |
|---|---|
| [`MIT.txt`](MIT.txt) | First-party MIT terms. |
| [`Apache-2.0.txt`](Apache-2.0.txt) | First-party Apache-2.0 terms. |
| [`NOTICE`](NOTICE) | The Apache-2.0 §4(d) NOTICE. Ships with the binaries. |
| [`licenses.txt`](licenses.txt) | **Generated.** Every third-party notice the shipped binaries owe. Ships in the release zip. Do not hand-edit — see below. |

## Why dual MIT/Apache-2.0

- **Apache-2.0 §3 is an express patent grant from every contributor**, and it
  terminates for anyone who sues claiming the work infringes. MIT is silent on
  patents — there is an argument that "use, copy, … and/or sell" implies a
  license, but it is untested. For a project that takes outside contributions,
  that is the one protection MIT cannot offer: a contributor cannot hand over
  code today and assert a patent over it later. It is also why organizations
  with legal review tend to clear Apache-2.0 faster.
- **It matches the dependency graph.** 154 of the ~294 crates s2script links are
  already `MIT OR Apache-2.0`; matching them means downstream never has to
  reconcile anything.
- **The `OR MIT` half keeps the copy-paste surface frictionless.** `packages/*`
  types, `examples/`, and the base plugins are the templates every third-party
  plugin starts from; Apache's "state your changes" clause is friction there.

Because the two are offered with `OR`, nobody is ever *forced* to comply with
Apache-2.0. Someone who wants the simplest possible terms reads the SPDX string
and takes MIT — an experience identical to MIT-only. The only party affected is
the one who actively wants the patent grant, and they get it at no cost to
anyone else.

> **Not a reason:** Apache-2.0 §6's trademark disclaimer. It is a clarification,
> not extra protection — MIT does not grant trademark rights either. Neither
> license does, because trademark rights come from trademark law, not from a
> copyright license. The name is reserved in [`NOTICE`](NOTICE) under both.

Deliberately **not** GPL: SourceMod is GPLv3 only because it carries a
hand-written Valve-SDK linking exception, and copyleft would put a permanent
cloud over "is my plugin a derivative work?" — which is fatal to the registry.

## The Valve carve-out

`third_party/hl2sdk` (`alliedmodders/hl2sdk`, branch `cs2`) ships **no LICENSE
file**, and its sources carry `Copyright © 1996-2005, Valve Corporation, All
rights reserved`. Valve's SDK terms are not an open-source license.

s2script does not redistribute Valve source — hl2sdk is a git submodule, so a
clone of this repo contains none of it. But the **built** `s2script.so` embeds
Valve SDK object code, because `shim/CMakeLists.txt` compiles these in:

| Translation unit | Why |
|---|---|
| `entity2/entitykeyvalues.cpp` | `CEntityKeyValues` for EKV-configured entity spawns |
| `tier1/keyvalues3.cpp` | KV3 backing store for the above |
| `public/tier0/memoverride.cpp` | routes `operator new`/`delete` to the game allocator |
| `lib/linux64/release/libprotobuf.a` | protobuf 3.21.8 reflection for `SayText2` |

Consequently the MIT/Apache grant covers **s2script's own code only**. It does
not, and cannot, relicense the Valve-derived portions of a built binary. This is
the same posture as every other framework in this ecosystem — Metamod:Source,
SourceMod, CounterStrikeSharp, and CS2Fixes all ship binaries built against
hl2sdk on the same footing.

*(Not legal advice. If you plan to sell a fork, get your own counsel.)*

## `licenses.txt` is generated, and gated

Hand-maintaining it is not viable: Breakpad's own LICENSE is 1304 lines of
aggregated sub-licenses, the `v8` crate vendors 16 third-party trees (abseil,
ICU, simdutf, …), and the crate graph moves every `cargo update`. A stale
notice file is a compliance bug that looks like a green build.

So it follows the same regenerate-and-gate discipline as the rest of the repo's
derived data (`check-schema-generated.sh`, `check-events-generated.sh`, …):

```bash
./scripts/gen-licenses.sh          # rebuild licenses/licenses.txt from the real sources
./scripts/check-licenses-generated.sh   # gate: regenerate + `git diff --exit-code`
```

The generator reads actual upstream `LICENSE` files — from the initialized
submodules under `third_party/` and from the crate sources in the cargo
registry — so it can never drift from what is really linked. It fails closed:
a missing submodule or an unresolvable crate license aborts the run rather than
silently emitting an incomplete notice file.

Requires `git submodule update --init --recursive` and a populated cargo
registry (`cargo fetch`).
