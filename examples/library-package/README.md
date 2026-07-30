# library-package

`@example/base64` is a **library** — build-time TypeScript a plugin bundles into its own
`.s2sp`, not something the host loads on its own. `s2script.kind: "library"` is what tells
`s2s build` to emit a `dist/_example_base64.s2lib` here instead of a `.s2sp`. It has no
`Buffer`/`atob`/`btoa` in its codec, on purpose: bare V8 (what a plugin actually runs in) has
none of those, so this is what real library code has to look like.

A published library is added to a consuming plugin with `s2s add @example/base64`, which
vendors it to `.s2script/libs/@example/base64/` — see `examples/library-consumer` for the other
half of this example.
