# @s2script/sdk

The **s2script SDK** — the TypeScript types and the `s2s` CLI for building
[Source 2](https://developer.valvesoftware.com/wiki/Source_2) / Counter-Strike 2 server plugins.

s2script is a plugin framework for Source 2 games, loaded via
[Metamod:Source](https://www.sourcemm.net/). This package is what you develop against; the runtime
ships with the server addon.

## Quickstart

```bash
npx @s2script/sdk create my-plugin
cd my-plugin
npm run build          # → dist/<id>.s2sp
```

Copy the `.s2sp` into `addons/s2script/plugins/` on a running server and it loads immediately —
re-drop to hot-reload, delete to unload. No restart.

```ts
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("hello", (cmd) => {
    cmd.reply("hello from s2script");
  });
});
```

This package is **types-only** — the engine injects the implementation at load time, and
`s2s build` marks `@s2script/*` external rather than bundling it. Capabilities are imported as
per-capability subpaths (`@s2script/sdk/entity`, `@s2script/sdk/timers`, `@s2script/sdk/clients`, …);
game-specific schema types ship separately in
[`@s2script/cs2`](https://www.npmjs.com/package/@s2script/cs2).

## Docs

**[s2script.com/docs](https://s2script.com/docs)** — getting started, guides, and the full API
reference. Source and issues: [GitHub](https://github.com/GabeHirakawa/s2script).

## License

MIT OR Apache-2.0
