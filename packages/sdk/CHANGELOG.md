# @s2script/cli

## 0.10.1

### Patch Changes

- 8d25254: Schema codegen: embedded structs, enums, `wrapEntity`, and every entity class.

  Catch-up changeset — this work merged in #26, #28 and #29 without one, so neither package was
  versioned or published and the new surface is unreachable from npm.

  `@s2script/cs2` grows from 62 to 415 interfaces and 793 to 2740 exposed fields:

  - **Embedded structs** as nested accessors, at any depth — `pawn.glow.glowing`,
    `pawn.collision.collisionGroup`, and struct-inside-struct via
    `collision.collisionAttribute.collisionGroup`.
  - **`wrapEntity(className, ref)`** — schema accessors on an entity you created. `createEntity`
    returns a bare `EntityRef`; this is how it gets fields. Keyed on a generated class map, so a wrong
    name is a compile error rather than an object with silently missing accessors.
  - **Enums** as unsigned integers of the width their binding declares. They were skipped wholesale
    because the category names the type but not its width; the width is now dumped from the live
    SchemaSystem. 533 fields catalog-wide. Notably `moveType`, `solidType` and `renderMode`.
  - **`Color` as a packed uint32** (R in the low byte), which also exposes `m_clrRender` as `render`.
    Previously skipped as "not a scalar", which is what kept `m_glowColorOverride` unreachable.
  - **367 entity classes** instead of 13 — everything deriving from `CEntityInstance` that has fields,
    plus `CCSGameRules`. Costs 3.6 ms once at boot.
  - **Transparent value wrappers flattened**: `pawn.deathTime` is a `number`, not `{ value }`.
    `GameTime_t`/`GameTick_t` were the majority of embedded fields on a pawn.

  Offsets still resolve by NAME at runtime, so none of this bakes in a layout constant.

  `@s2script/sdk` is a patch: only the codegen internals and its tests changed, no published `.d.ts`,
  but the shipped `dist/cli.js` differs.

## 0.10.0

### Minor Changes

- a64da95: Add cancellable, repeating timers — SourceMod `CreateTimer` / `KillTimer`.

  `after(ms, fn)` and `every(ms, fn)` return a `Timer` handle with `kill()` and `alive`. The existing
  `delay`/`nextTick`/`nextFrame` are Promises and cannot be cancelled: "cancelling" one means leaving
  it forever unresolved, which leaks the continuation.

  Every timer is ledgered against the creating plugin, so unload kills it whether or not `kill()` was
  called — a repeating timer can never outlive its plugin and fire into a dead context. A throwing
  callback is contained and a repeater keeps repeating; `kill()` is idempotent and safe from inside
  the callback itself; `every(0, …)` throws rather than starving the frame.

- a64da95: Add `Client.command` and `Client.fakeCommand` — SourceMod `ClientCommand` / `FakeClientCommand`.

  `command(cmd)` asks the client to run it in their own console; `fakeCommand(cmd)` has the SERVER
  execute it attributed to that player, so it works on bots. Both return `false` rather than silently
  no-opping when the slot is bad, the text is empty, or the engine interface is unavailable.

  `fakeCommand` dispatches through `ICvar::DispatchConCommand` with a `CCommandContext` carrying the
  slot — verified live by attribution, where a faked `say` prints as that player rather than Console.
  Engine commands execute; a command registered by an s2script _plugin_ is dispatched but its JS
  handler does not run, because the core holds the isolate borrow across all JS and the nested
  dispatch hits the documented re-entrancy skip. Use a cross-plugin interface to drive another
  plugin's behaviour.

- a64da95: Add `Server.onCvarChange` — SourceMod `HookConVarChange`.

  Watch one cvar by name, or `"*"` for every cvar; the handler receives the name plus the new and old
  values as strings. Returns `{ dispose() }`, and subscriptions are ledgered so unload drops them
  regardless.

  Notify-only, and the API says so: the engine's global change callback runs _after_ the value has
  been applied, so a handler cannot veto a change. A handler that throws is logged and contained; the
  remaining handlers still run. The engine only calls back on a real change, so a write of the same
  value does not fire and plugins need not de-duplicate.

- a64da95: Make curated `pawn.movementServices` fields writable — the `GetMaxSpeed` equivalent.

  All 53 generated accessors on the movement services were read-only. 14 now have setters:
  `maxspeed`, `stamina`, `surfaceFriction`, `fallVelocity`, the duck group, and the six move-input
  fields. Writability is opt-in per field via a `writable` allowlist in `nav-targets.json` rather than
  derived from the field's type — which byte a field lives at is regenerable layout, but whether
  writing it is safe is a reviewed behavioural decision, and the type-derived set would have exposed
  engine bookkeeping such as `m_nTraceCount`.

  A bad allowlist entry now fails codegen instead of silently emitting nothing: an unknown field name
  (what a CS2 rename produces) and a kind with no `EntityRef.write*Via` both throw, naming the field.

  These writes are not flagged for replication — the change-notifier addresses the root entity while a
  nav write changes a subobject — so a predicting client may see brief mismatch. Stated on the emitted
  `MovementServices` interface.

- a64da95: Publish standard interface contracts for econ/skins and workshop — types only, no implementation.

  New subpaths: `@s2script/sdk/contracts/workshop` (`WorkshopService`, engine-generic) and
  `@s2script/cs2/econ` (`EconService`, `WeaponSkin`, `Loadout`, CS2-specific). A community plugin
  implements one and publishes it; consumers depend on the agreed shape via `ctx.tryUse` instead of a
  different ad-hoc interface per plugin.

  The framework ships no implementation deliberately: applying skins means driving CS2's economy item
  model and workshop means Steam's UGC services, neither of which is a Source 2 engine touchpoint.

  Every 64-bit value in these contracts — workshop published-file IDs, SteamIDs — is typed as a
  decimal **string**, because a `BigInt` throws crossing the plugin boundary and silently drops the
  whole payload. Everything Steam-facing is `Promise`-returning so an implementation cannot block the
  game frame.

  Also fixes game-package subpath resolution in the plugin typecheck: `@s2script/*` mapped to
  `<pkg>/index.d.ts`, which cannot express a subpath, so `@s2script/cs2/econ` was `TS2307`.

- 291e017: Add `@s2script/sdk/voice` — per-(receiver, sender) voice hearability.

  `Voice.setAudibleTo(sender, receivers)` restricts who can hear a speaker; rules from multiple plugins
  AND-merge so one plugin can only narrow another's, never widen it. Layered under `Client.voiceMuted`,
  which still wins, so admin moderation always beats a gameplay rule.

  Declarative rather than a callback: the engine's listen matrix is re-asserted continuously and the
  underlying hook fires per pair, so a per-pair JS callback would run up to 64x64 times per refresh.
  `Voice.stats()?.rewrites` is the effect counter (stats is nullable — null means the running shim predates the capability) — a rule that never rewrites is not taking effect.

## 0.9.0

### Minor Changes

- 6a423a1: Add plugin-shippable gamedata and declared engine calls (`@s2script/sdk/unsafe`).

  A plugin can now declare, in its own regenerable gamedata, an engine function the framework does not
  natively wrap, and call it from TypeScript with generated types.

  `s2s build` gains three behaviours when `s2script.gamedata` is set: it validates the gamedata, writes
  `.s2script/gamedata.d.ts` (which augments `EngineCalls`, so arity and argument types are enforced by
  the existing typecheck gate), and packs `gamedata.json` into the `.s2sp`. Declaring a `calls` section
  requires `s2script.permissions: ["engine:calls"]`, which is recorded in the manifest — and is
  necessary but not sufficient, since an operator must also allow-list the plugin id.

  New export subpath `@s2script/sdk/unsafe`, exposing `Engine.call(name)` (a plain callable, or `null`
  when the descriptor failed a load-time gate) and `Engine.status(name)` (the named reason).

  Validation is deliberately strict in two places that are easy to get wrong: a `vtable` target must
  carry a `validate.prologue` (a bare borrowed index is never trusted, because a wrong-but-in-range slot
  silently misbehaves rather than crashing), and call names must be plain identifiers, since they are
  interpolated into the generated `.d.ts` and a crafted name could otherwise inject an index signature
  that defeats the type gate entirely.

- 6de4606: Add `s2s install` — download plugins and their dependencies onto a server.

  Reads an `s2script-plugins.json` manifest (and/or names on the command line),
  resolves each plugin's full non-`@s2script/*` dependency tree from the registry,
  verifies every `.s2sp` by sha256, and writes them into the server's plugins
  directory. Needs no credentials (registry reads are public), so it drops cleanly
  into a Dockerfile:

      COPY s2script-plugins.json .
      RUN s2s install --dir /cs2/game/csgo/addons/s2script/plugins

  `@s2script/*` base plugins are skipped (they ship with the runtime). Unreviewed
  plugins install with a warning; `--reviewed-only` blocks them. `--dry-run` prints
  the resolved plan without downloading.

## 0.8.0

### Minor Changes

- afce5a2: Chat messages now render their first color byte without a hand-written leading space. `Chat.toSlot` / `Chat.toAll` (and every chat-bound `ctx.reply`) auto-prefix each line with an invisible zero-width space (U+200B), so a Source 2 chat box no longer swallows a color control byte that sits at index 0. Write `Chat.toSlot(slot, ChatColors.Green + "hi")` and the green lands. The prefix is idempotent — a line you already lead with a space or a ZWSP is passed through unchanged — and chat-only, so console / rcon replies stay byte-clean.
- Deploy over `application/octet-stream`, report registry errors properly, and log in through the browser.

  **Breaking:** `s2s deploy` now posts a single zip (`manifest.json` + `plugin.s2sp` + optional
  `types.tgz`) as `application/octet-stream` instead of `multipart/form-data`. It requires a registry
  running the matching server change and cannot deploy to an older one.

  Multipart was the reason deploys failed: SvelteKit's CSRF guard rejects form-content-type POSTs
  whose `Origin` does not match, and Node's `fetch` sends no `Origin` at all, so every deploy was
  refused before the server ever read the token.

  Errors are now surfaced instead of swallowed. The client previously read only `body.error`, but the
  registry emits `{message, status}` from SvelteKit's `error()` helper, `{error, code}` from explicit
  JSON responses, and plain text from the CSRF guard — so most failures showed up as a bare
  `deploy failed (403)`. All four client methods now handle every shape, plus HTML error pages, empty
  and malformed bodies, network failures, and timeouts, and say what to do next.

  `s2s login` opens the browser and uses device authorization rather than asking for a pasted token.
  Pasting still works, and `S2SCRIPT_TOKEN` is unchanged for CI.

  The registry URL now defaults to `www` and saved apex URLs are normalized, because an apex redirect
  downgrades POST to GET and strips the `Authorization` header.

## 0.7.0

### Minor Changes

- 74d45bd: The `s2s` CLI is now interactive: arrow-key prompts for `create` and `login`, a no-arg command menu (`s2s` in a terminal), and spinners on `build`/`deploy`/`add`, with consistent styling. Every non-interactive path (flags, `-y`, `--ci`, no TTY, CI) behaves exactly as before — same output and exit codes; `build` still prints the plain `.s2sp` path to stdout. Powered by `@clack/prompts`, bundled into the CLI, so there is no new runtime dependency.

### Patch Changes

- d6949a1: Replace the `adm-zip` dependency with `fflate` for all `.s2sp` zip read/write in the `s2s` CLI (`build`, `config gen`, `deploy`). `adm-zip <0.6.0` carries a high-severity advisory (GHSA-xcpc-8h2w-3j85, crafted-ZIP 4 GiB allocation) that kept surfacing in `npm audit`; `fflate` has no known advisories, ships its own types, and is small enough to bundle into `dist/cli.js` — so it is no longer a runtime dependency at all (the CLI now installs zero third-party zip code). Archive output is unchanged (standard DEFLATE members read by core's `read_s2sp`); verified against the base-plugin build and an independent `unzip` reader.

  Also refresh the `s2s login` prompt for the registry's new auth flow: it now points at the full `<registry>/account/tokens` URL and notes you sign in (or create an account) first, and fixes a stale `s2script login` reference.

## 0.6.0

### Minor Changes

- 24864c0: **Behaviour change:** `CommandInvocation.reply` now answers in the channel the caller used, matching
  SourceMod's `ReplyToCommand`. A player who types `sm_help` at their developer console gets the reply
  in that console instead of having it spammed into chat; `!help` still answers in chat, and the server
  console/rcon still answers on the server console (now with control bytes stripped). Control bytes are
  stripped because a chat colour is a control byte and renders as garbage in a console. No plugin change
  is needed — every existing `reply` / `replyT` call site is routed correctly automatically.

  Alongside it, `CommandInvocation` gains:

  - a readonly `replySource` (`"server" | "console" | "chat"`) recording how the command was invoked,
    plus the exported `ReplySource` type;
  - `replyToChat` and `replyToConsole` — explicit targets that force the answer into a specific channel
    regardless of how the command was invoked (SM `PrintToChat` / `PrintToConsole`).

  `Commands.dispatch` takes an optional trailing `replySource` so a plugin re-dispatching a command on
  a player's behalf can say which channel the answer belongs in; omitted, it falls back to the caller's
  slot (SM `FakeClientCommand` parity). `Commands.handleChatTrigger` always dispatches as `"chat"`,
  including for the silent `/` trigger.

  Also fixes a latent hazard on the same surface: the context's reply methods no longer depend on their
  receiver, so a plugin that hands `cmd.reply` to a helper as a bare function reference keeps working
  instead of throwing a silently-swallowed `TypeError`.

## 0.5.1

### Patch Changes

- c9f0293: Rich TSDoc across the author-facing `@s2script/sdk` capability stubs: every exported symbol and member now carries a doc comment — a summary line, `@param`/`@returns`/`@throws` where they add signal, `{@link}` cross-references, and an `@example` (drawn from real plugin/example usage) on each major entry point — so hovering any SDK symbol in an editor gives complete intellisense. Types are unchanged; this is a comments-only pass verified against every base plugin and example.

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
