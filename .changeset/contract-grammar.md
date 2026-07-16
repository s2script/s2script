---
"@s2script/sdk": minor
---

Contract grammar: the host injects an interface's version, and an inconsistent manifest fails the plugin's load.

`publishInterface(name, impl)` drops its version parameter and is now generic —
`<T extends object>`, not `Record<string, Function>` — so a producer binds its
implementation to its own contract type (`const impl: Zones = {...}`) and `tsc` proves
the shape matches; a `Record` parameter would reject every interface-typed contract
outright (no implicit index signature). The host reads the version from the plugin's
manifest `publishes` map and refuses to register a name the manifest does not declare.
A plugin may no longer type a version string anywhere.

Refusing an undeclared publish isn't enough on its own to make a bad manifest "fail the
load": a typo'd `publishInterface` name (manifest says `@x/greeter`, code publishes
`@x/greetr`) gets its stray publish refused, but the plugin still ran with `@x/greeter`
silently unpublished — surfacing later as `InterfaceUnavailable` in some other plugin's
consumer. So the host also reconciles, *after* `onLoad` returns, every interface a
plugin's manifest declares against what it actually ended up owning; a mismatch tears
the plugin down (WARN + unload) rather than leaving it running half-honoured. This is
per-descriptor degradation — only that plugin is refused — and it also catches the loser
of a two-live-producer race, since a rejected second publisher never owns the name it
declared.

`s2script build` derives `publishes` as `{interface: {version, typesSha256}}` from the
authored `"self"` sugar (a map-form range is refused for now — resolving it needs the
registry, out of scope for this slice) and embeds a hash-verified copy of the contract
in the `.s2sp`. The typecheck gate's ambient-stub filter now checks whether a module
actually resolves on disk rather than pattern-matching `@s2script/*` by name, so a
plugin-published interface (e.g. `@s2script/zones`, which no longer has an npm package
behind it) still typechecks; a plugin's own `src/*.d.ts` is compiled as part of its
gate too, instead of only ever being picked up by the editor.

`@s2script/zones` is no longer published to npm — its contract now ships with the zones
plugin itself, at `plugins/zones/api.d.ts`. The already-published `@s2script/zones@0.2.0`
npm package is not unpublished by this change; deprecating it (`npm deprecate
@s2script/zones@"<=0.2.0" "..."`) needs npm auth and is a maintainer action, not run here.
