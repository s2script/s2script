# panorama-svelte (prototype)

Author a `custom_hud_layout` as Svelte components and build it to the XML, CSS and manifest the
workshop addon ships. It sits one level below `tools/` because the plugin typecheck gate treats
every `tools/*/package.json` as a plugin. Not wired into CI or the addon yet: this is a feasibility prototype.

```bash
npm ci
npm run build   # example/Hud.svelte -> out/hud.xml, out/hud.css, out/hud.manifest.json
npm test        # 10 tests: the example builds, 8 kinds of invalid layout fail
npm run check   # svelte-check over the kit and the example
```

## How it works

CustomHud has no client scripting, so Svelte never runs in the game. Svelte 5.36 added an
experimental custom renderer API (`svelte/renderer`, `experimental.customRenderer`): a component
can render into any tree, not only the DOM. `src/renderer.mjs` is such a tree for CustomHud.
`src/build.mjs` compiles the components, mounts the entry once in Node, and writes the tree out.

`{#each}` expands a pool at build time, components split a layout into files, props carry the
numbers the runtime must agree with, and `<style>` blocks become the stylesheet (scoped classes
work: Panorama matches `.a.svelte-hash` like any compound selector).

## What cannot be built

| Mistake | Caught by |
|---|---|
| A tag other than Panel / Label / Image / Button | renderer, build error |
| An attribute off the tag's whitelist (`textureheight`, `style`, `scaling`) | renderer, build error; type error on kit components |
| An event handler (`onclick`) | renderer, build error |
| An id on the outer panel, a duplicate id, a Button with no id | serialiser |
| A dialog variable not named after its panel id | serialiser |
| More than 1024 ids in one layout | serialiser |
| Loose text outside a Label, children under a Label or Image | serialiser |
| CSS the client ignores (`display`, `flex`, `calc()`, `rgba()`, `:not()`, `@media`) | `src/css.mjs` |

The whitelist is one table, `src/rules.mjs`.

## What it does not solve

- **A stale whitelist.** The table still comes from `client.dll` and still has to be re-read after
  a CS2 update. A wrong table builds wrong layouts with full confidence.
- **The runtime.** `games/cs2/js/gamekit*.js` still drives ids by hand. The manifest here lists ids,
  dialog variables and buttons; pools and meters are not ported.
- **Editor typing of raw elements.** `<hud-image textureheight>` written directly is not a type
  error (svelte-check is lenient on hyphenated elements); the build still rejects it. The kit
  components (`src/kit/`) are strictly typed.
- **CSS tooling.** The editor's CSS service does not know `flow-children` and friends, so CSS
  diagnostics are switched off in `npm run check`; `src/css.mjs` is a deny-list, not Panorama's
  full property list.
- **API stability.** Custom renderers are experimental and may change; the Svelte version is pinned.
