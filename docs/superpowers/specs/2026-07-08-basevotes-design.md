# basevotes (voting) — Design

**Status:** Approved (brainstorm — combo UI, opt-in live tally, scope set), ready for the plan.
**Slice:** the SourceMod voting base plugin — `sm_vote` (custom) + `sm_votekick`, built on a new engine-generic `@s2script/votes` primitive (a chat ballot + result, with an optional live center-HTML tally).

## Goal

Give server admins votes: `sm_vote "Question" "Opt1" "Opt2" …` (a custom vote) and `sm_votekick <target>` (a built-in). A vote prints a chat ballot to every connected player, captures their picks (revote-able), and after a set time announces the result in chat and runs the action (for `votekick`, a passing Yes kicks the target). An **opt-in live tally** can additionally show running counts on the center screen.

## Motivation & context

Voting is a core SourceMod base plugin and fills the empty **Voting Commands** TopMenu category `adminmenu` stubbed. The design reuses the just-built primitives (`@s2script/menu`/`Chat`/`Clients`/`pickPlayer`/`TopMenu`) and adds one new engine-generic layer — `@s2script/votes`. The input is deliberately **chat-based**, not a WASD center menu: a vote can't freeze players mid-game, and a non-freezing WASD menu would conflict with movement (navigating = walking). Typing a number does not. The center HTML is used only for the **optional** live tally (a display, no input).

## Scope

**In scope:** `@s2script/votes` (the vote primitive + a tally-render seam); the CS2 center-tally renderer; `sm_vote` (custom) + `sm_votekick` (the built-in proof) in a new `basevotes` plugin; the TopMenu **Votekick** item (Voting Commands category).

**Deferred (named follow-ons):** `sm_voteban` / `sm_votemap` (mechanical once the primitive works); `funvotes`; a **cross-plugin global vote lock** (the MVP lock is per-context — one plugin); vote **cooldowns** and **min-turnout** thresholds (a simple majority for the MVP).

**Non-goal (will NOT be built):** the native CS2 F1/F2 vote panel (`CVoteController` RE). The chat-ballot + optional-center-tally combo is the intended, permanent vote UX for s2script — the native panel is explicitly not on the roadmap.

## Approach (decided)

**A combo UI: chat ballot for input, optional center HTML for the live tally.** The vote splits by the charter litmus exactly like the menu did:
- **Vote logic** (options, per-player tally, revote, duration/lifecycle, pass/fail) + **chat ballot** (`Chat.toAll`/`Chat.onMessage`) + **player enumeration** (`Clients.all()`) — engine-generic → `@s2script/votes`.
- **The live center-tally HTML** (`show_survival_respawn_status`, re-sent each tick) — CS2-specific → a renderer registered through a **`registerTallyRenderer` seam** (mirrors `@s2script/menu`'s `registerRenderer`), invoked only when `showLiveTally` is set.

Not pursued (a permanent non-goal, user-confirmed) — the **native CS2 vote panel** (F1/F2 via `CVoteController`): a substantial engine-RE spike, and the chat-ballot + center-tally combo is the intended vote UX for s2script for good. It is not a future direction.

## Architecture

One-way deps (game → core). `@s2script/votes` is engine-generic; the CS2 tally renderer + `basevotes` are the game layer.

### `@s2script/votes` (engine-generic, core prelude)

The vote model + lifecycle. A single active vote per context (a lock).

- **`Vote.start(config) → boolean`** — begins a vote; returns `false` if one is already active. `config`:
  - `question: string`, `options: string[]` (2+), `duration: number` (seconds).
  - `showLiveTally?: boolean` (default **false** — the SM way: no live display, just the chat result after `duration`).
  - `onEnd: (result: VoteResult) => void`.
- **`Vote.isActive(): boolean`**, **`Vote.cancel(): void`** (abort — clears, no `onEnd`).
- `VoteResult = { winner: number | null, counts: number[], total: number }` — `winner` = the option index with the most votes; a tie or zero votes → `null` (the caller treats `null` as "failed / no decision").

**Flow (in `@s2script/votes`):**
1. `start`: set the active lock; `Chat.toAll` the ballot (`[Vote] <question> — type 1=<opt0>, 2=<opt1>, …`); subscribe `Chat.onMessage`; subscribe `Clients.onDisconnect`; arm a `duration` timer; if `showLiveTally`, call the registered tally renderer (below).
2. **Capture:** on a `Chat.onMessage` from slot `S` whose trimmed text is a digit `1..options.length`, record `votes[S] = digit - 1` (a `Map<slot, index>`; **re-typing replaces** → revote) and return `HookResult.Handled` (swallow it). Non-vote chat passes through. If `showLiveTally`, refresh the tally on each cast.
3. **Countdown (only if `showLiveTally`):** a `1s`-interval (via `delay` re-arm) updates `secondsLeft` and re-calls the tally renderer so the center HUD shows the ticking time + counts.
4. **Disconnect:** `Clients.onDisconnect` → delete that slot's vote.
5. **End** (timer fires, or every connected non-bot has voted): compute `counts`/`winner`; `Chat.toAll` the result (`[Vote] Passed: <opt> (<pct>%)` or `[Vote] No votes / tie — failed`); clear the tally (`renderer.clear` per voter); release the lock; call `onEnd(result)`.

**Tally seam:** `Vote.registerTallyRenderer(renderer)` where `renderer = { show(slot, tally), clear(slot) }` and `tally = { question: string, options: [{ label: string, count: number }], total: number, secondsLeft: number }`. `@s2script/votes` calls `show(slot, tally)` for each connected voter on each cast + once per second; `clear(slot)` on end. If no renderer is registered (a non-CS2 game, or none), `showLiveTally` degrades to chat-only with a one-time warn.

Types-only package `packages/votes/{package.json,index.d.ts}`.

### CS2 center-tally renderer (`games/cs2/js/pawn.js`)

Registers a tally renderer through the seam. `show(slot, tally)` stores the current tally for `slot`; a lazy `OnGameFrame` poll (armed while ≥ 1 tally is active) re-sends the center HTML each tick via the same `Events.fireToClient(slot, "show_survival_respawn_status", { loc_token })` path the menu center renderer uses — formatted as a title (`fontSize-m`) + one row per option (`fontSize-sm`: `<label> — <count>`) + a `secondsLeft` footer (`fontSize-s`). `clear(slot)` drops the tally + sends the blank space to wipe the HUD (the menu's clear trick). **No `freezePlayer`** — a vote never freezes. Reuses the menu renderer's per-tick re-send discipline; no new native.

### `basevotes` plugin (`plugins/basevotes`, CS2)

Declares two config values (`@s2script/config`): `vote_duration` (int, default 20) and `show_live_tally` (bool, default true — the admin can turn the live center tally OFF for the pure SM chat-only vote, satisfying "allow the option to show results or not"). `onLoad` registers:
- **`sm_vote "Question" "Opt1" "Opt2" …`** (`ADMFLAG.VOTE`) — parse the quoted question + options; `Vote.start({ question, options, duration: config vote_duration, showLiveTally: config show_live_tally, onEnd: r => Chat.toAll(...) })`. Refuse if `Vote.isActive()`.
- **`sm_votekick <target>`** (`ADMFLAG.VOTE`) — resolve the target via `Player.target` (single, reject ambiguous); `Vote.start({ question: "Kick <name>?", options: ["Yes","No"], duration, showLiveTally, onEnd: r => { if (r.winner === 0 && r.counts[0] > r.total / 2) target-kick } })` — re-resolve the target by userId at end (the pick-time slot may be stale — the adminmenu-Ban lesson).
- A TopMenu **Votekick** item (Voting Commands, `ADMFLAG.VOTE`) → `pickPlayer` → start the votekick vote.

## Testing & live gate

- **Core unit tests** (`@s2script/votes`, in-isolate against fake `Chat`/`Clients`/`timers` + a record tally-renderer): a cast records a vote; a re-cast replaces it (revote); `counts`/`winner` computed correctly; a tie and zero-votes both yield `winner: null`; a 2nd `start` while active returns `false`; a `Clients.onDisconnect` drops that slot's vote; `showLiveTally:false` never calls the renderer; `showLiveTally:true` calls `show`/`clear`.
- **Live gate (human + bots):** `sm_vote "Test?" Yes No` → the chat ballot prints to all; typing `1` then `2` (revote) records + flips; at `duration` the result announces in chat. With `showLiveTally`, the **center tally shows live** and counts up as the human votes, ticking down. `sm_votekick <bot>` → a Yes majority kicks the bot. `sm_admin → Voting Commands → Votekick` chains in. RestartCount=0, no crash.
- **Gates:** core-boundary (`@s2script/votes` engine-generic — no game names, only chat strings/slots/the tally seam), name-leak, typecheck, full `cargo test`. One sniper rebuild (the `@s2script/votes` prelude module).

## Boundary & safety summary

`@s2script/votes` is engine-generic (a vote is chat strings + slot-keyed tallies + a render seam — Source2-generic). The center-tally renderer (`show_survival_respawn_status`/`loc_token`), `basevotes`, and the `ADMFLAG.VOTE` gating are the CS2/game layer. The vote composes already-ledgered subs (`Chat.onMessage`, `OnGameFrame`, `delay` timers, `Clients.onDisconnect`), so teardown is free — no new ledger resource. Both boundary gates stay green.
