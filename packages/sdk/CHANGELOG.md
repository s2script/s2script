# @s2script/cli

## 0.5.0

### Minor Changes

- cb50b95: B1 (build ⊇ load): `s2s build` now DERIVES the manifest — `apiVersion` is stamped from the SDK's
  host-major constant (authored values ignored with a warning), the `publishes` name-set is derived
  from `ctx.publish` calls (drift is a build error; `"self"` auto-derives), dependency-usage
  advisories warn on declared-vs-used mismatches, and a `.s2script/types/<iface>/index.d.ts`
  verified contract copy gives a consumer REAL dependency types plus a `compiledAgainst` hash that
  the host verifies at load (contract drift now fails fast at load AND per-call).

  B2: new `@s2script/eslint-plugin` — `no-ctx-escape`, `no-floating-promise-in-factory`,
  `no-bigint-in-interface-payloads`, `no-await-in-raw-view` — pinned by the SDK, scaffolded by
  `s2s create` (`eslint.config.mjs`), and executed in-process by `s2s build` after the tsc gate
  against the gate's own `ts.Program`. Lint errors refuse the `.s2sp`.

- ddcb4c6: BREAKING (pre-1.0 minor): `EntityRef` is now `{index, id}` — `id` is a host-minted
  liveness id replacing the raw engine `serial` on the public surface. Liveness is
  decided by the host's books (listener-fed, cleared per map), never by entity memory;
  stale refs — including across a changelevel — deterministically resolve to
  `null`/`false`. The inter-plugin/handoff wire format is `{__s2ref: [index, id]}`;
  pre-E1 `{__entref__}` blobs revive as inert data. The `EntityRef` constructor is no
  longer part of the public typed surface — the framework mints every ref.
- 6cec7d0: L1 lifecycle v2: the plugin is a typed artifact. New `@s2script/sdk/plugin` subpath
  (`plugin()`, `PluginContext`, `Scope`, `PluginHooks`); every registration verb moves to `ctx`;
  `CommandContext`→`CommandInvocation` (param naming: `cmd`); usercmd `Cmd`→`UserCmdView`;
  apiVersion major is now 2.x. Old ambient registration verbs are deprecated and removed in-series.

### Patch Changes

- Updated dependencies [cb50b95]
  - @s2script/eslint-plugin@0.2.0

## 0.4.0

### Minor Changes

- bd40c35: config: new `s2s config gen <plugin.s2sp...> --out <dir>` command. It reads each staged `.s2sp`'s
  baked manifest and emits the operator's default config file — commented JSONC (defaults + a
  `// type — description` line per decl, sections nested) byte-compatible with the core's
  `generate_default_jsonc`, at a filename that matches the runtime's ConfigPath sanitizer exactly
  (`@s2script/funvotes` -> `_s2script_funvotes.json`). Plugin-scoped: it knows nothing about the
  framework templates, which the release script ships separately.
- 4db1f4f: config: sectioned config blocks + enriched validation. `s2script.config` entries may now nest into
  sections (any entry without a string-valued `type` key is a section, recursed), and decls gain
  `min`/`max` (int/float, mutually exclusive with `enum`), `enum` (string/int), `group`/`label`, and
  `sensitive` (masked in display, still written to the file). `validateConfigBlock` enforces all of
  these plus the ban on `.` in key names. The `@s2script/config` `Config` type widens to a recursive
  `Record<string, ConfigValue>` so nested sections type-check; this is an additive `.d.ts` widening
  (no apiVersion bump — plugins that used the flat scalar shape still type-check).

## 0.3.0

### Minor Changes

- 972103b: transmit: new builtin capability (`@s2script/sdk/transmit`) — per-client entity visibility filtering via a Source2 CheckTransmit post-hook. Declarative rules (`Transmit.setVisibleTo`/`reset`/`resetAll`/`stats`); multiple plugins AND-merge; zero JS in the per-snapshot hot path.
- c8639f2: UserMessage interception: `UserMessages.onPre(name, handler)` / `UserMessages.off(name)` with a
  block-scoped `UserMessageView` (typed scalar reads with dotted nested paths, read-only recipients,
  `debugString` fallback). Returning >= `HookResult.Handled` suppresses the send for every recipient.
  Fail-closed: an unresolvable name (or a degraded intercept descriptor) throws at subscribe time.
- bb2891c: Voice control: `Client.voiceMuted` (get/set — server-side mute of the client's outgoing voice for all
  receivers, enforced by a SetClientListening rewrite hook) and `Clients.onVoice(handler)` (throttled
  voice-transmission notification). Degrades to an inert no-op with a named reason if the voice
  descriptor fails validation.

## 0.2.0

### Minor Changes

- d858f38: Contract grammar: the host injects an interface's version, and an inconsistent manifest fails the plugin's load.

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
  consumer. So the host also reconciles, _after_ `onLoad` returns, every interface a
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

### Patch Changes

- 2ad151b: `s2s create` resolves non-`sdk` dependency versions live from the registry

  The scaffolder pinned `@s2script/cs2` to the CLI's own (`@s2script/sdk`) version, which
  is wrong once the two packages diverge — it emitted an unsatisfiable `^0.1.0` for a
  `0.5.0` package and `npm install` failed. `@s2script/sdk` still pins to the CLI version
  (the CLI _is_ that artifact); every other package is now resolved from the registry at
  scaffold time (`npm view`, respecting `.npmrc`), degrading to `latest` only when the
  registry is unreachable, npm is absent, or the package is unpublished. The in-monorepo
  `file:` path is unchanged.

## 0.2.0

### Minor Changes

- 1675ba9: Team change + writable narrow-int schema fields.

  - `@s2script/cs2`: `Player.changeTeam(team)` and `Player.spectate()` — move a player's controller between teams (Spectator=1/T=2/CT=3) via the sig-resolved `CCSPlayerController::ChangeTeam` (serial-gated, degrade-never-crash). Narrow-int schema fields (`int8`/`int16`/`uint8`/`uint16`/`uint32`) now generate setters — `player.desiredFOV`, `player.teamNum`, etc. are writable.
  - `@s2script/cli`: `gen-schema` emits setters for narrow-int atomic fields (the `EntityRef.writeInt8/16`/`writeUInt8/16/32` methods already existed; the WRITE/ATOMIC maps were stale). 64-bit fields stay read-only.

## 0.1.1

### Patch Changes

- 5fcc41f: Initial public npm release of the `@s2script/*` types packages and CLI (Changesets pipeline).
