# funvotes (fun/admin votes) — Design

**Status:** Approved (brainstorm — set = alltalk/ff/gravity/slay decided), ready for the plan.
**Slice:** the SourceMod `funvotes` plugin — admin-triggered Yes/No votes that toggle a cvar or apply a player action on pass. `@s2script/votes`' third consumer (after basevotes + rockthevote). Ships opt-in (`disabled/`).

## Goal

Admins start a quick Yes/No vote; on a Yes majority the effect applies: `sm_votealltalk`/`sm_voteff` toggle a cvar, `sm_votegravity <value>` sets `sv_gravity`, `sm_voteslay <target>` kills a player. Reuses `@s2script/votes` (the vote), `@s2script/server` (`getCvar`/`setCvar`), and `@s2script/cs2` (`Player.target`/`pawn.slay`). No new engine primitive — JS-only, no sniper.

## Scope

**In scope:** the `funvotes` plugin (`disabled/funvotes/`, CS2) — `sm_votealltalk`, `sm_voteff`, `sm_votegravity <value>`, `sm_voteslay <target>`, all `registerAdmin(ADMFLAG.VOTE)`; a shared Yes/No vote helper; `funvote_duration`/`funvote_show_tally` config.

**Deferred (do NOT build):** `sm_voteburn` (needs the deferred ignite primitive); `sm_votegravity` with no arg → a preset-value multi-choice menu (MVP takes an explicit value); a player-initiated fun vote (SM's are admin-only by default); a cross-plugin global vote lock (per-context for now).

## Approach (decided)

- **Every command is a Yes/No vote** via `Vote.start({ question, options: ["Yes","No"], duration, showLiveTally, onEnd })`. On end, `VoteResult.winner` is the index of the most-voted option; **pass iff `winner === 0` (Yes)** — a `No` win or a tie/no-votes fails.
- **Cvar votes read the current value to frame the question + compute the new one.** `sv_alltalk`/`mp_friendlyfire` toggle (`"1"`↔`"0"`); `sv_gravity` sets an explicit numeric arg.
- **`sm_voteslay` re-resolves the target at vote end** (`Player.fromUserId`, captured at start) so a player who left mid-vote is skipped, never a stale/reused slot (the same safety `sm_kick`/adminmenu use).
- **One vote per context.** Each command refuses (`Vote.isActive()` → "a vote is already running") — funvotes has its own context lock, independent of basevotes/rockthevote.
- **Admin-only.** All four are `registerAdmin(ADMFLAG.VOTE)` (SM parity; a console/rcon caller is root).

## Architecture

Entirely CS2/game-layer (`disabled/funvotes/`). One file.

### The commands

- **`sm_votealltalk`** — `cur = Server.getCvar("sv_alltalk")`; `on = cur === "1" || cur === "true"`; question `"<Enable|Disable> AllTalk?"`; on pass `Server.setCvar("sv_alltalk", on ? "0" : "1")`.
- **`sm_voteff`** — same against `mp_friendlyfire`.
- **`sm_votegravity <value>`** — `value = ctx.arg(0)`; reject if not `^[0-9]+(\.[0-9]+)?$` (usage: "sm_votegravity <number>"); question `"Set gravity to <value>?"`; on pass `Server.setCvar("sv_gravity", value)`. (The numeric validation is the injection guard — `setCvar` builds a console string.)
- **`sm_voteslay <target>`** — `targets = Player.target(ctx.arg(0), ctx.callerSlot)`; require exactly 1 (0 → "No matching players"; >1 → "Multiple players match — be specific"); capture `userId = targets[0].userId` + `name = targets[0].playerName`; question `"Slay <name>?"`; on pass re-resolve `p = Player.fromUserId(userId)`; if `p && p.pawn` → `p.pawn.slay()` else "target left".

### The vote helper

`startYesNo(ctx, question, onPass)`: if `Vote.isActive()` → `ctx.reply("A vote is already running.")` + return; else `Vote.start({ question, options: ["Yes","No"], duration: config.getInt("funvote_duration"), showLiveTally: config.getBool("funvote_show_tally"), onEnd: (r) => { if (r.winner === 0) { Chat.toAll("[Vote] Passed: " + question); onPass(); } else { Chat.toAll("[Vote] Failed: " + question); } } })` + `ctx.reply("Vote started.")`.

### Config

- `funvote_duration` (int, default 20) — vote length in seconds.
- `funvote_show_tally` (bool, default true) — the live center tally.

## Testing & gate

- **Live gate (bots-provable for the vote START + no-crash; the pass→apply is human-deferred):** deploy to `disabled/` → doesn't load; enable → loads. `sm_votealltalk` (rcon = root) → a Yes/No vote starts (the ballot + the question reflecting the current cvar), no crash; the vote ties (no bot votes) → "Failed" → the cvar unchanged. `sm_votegravity abc` → usage rejected (no vote). `sm_votegravity 200` → vote starts. `sm_voteslay <bot>` → resolves the bot → vote starts. A 2nd `sm_votealltalk` while one runs → "A vote is already running." `RestartCount=0`, no crash. (Set `funvote_show_tally=false` for the gate — the bot `fireToClient` tally path is untested.)
- **Deferred (human-client):** a real Yes majority → the cvar/slay actually applies (a composition of proven primitives — `Vote.onEnd`/basevotes + `Server.setCvar`/6.7-cvar-gate + `pawn.slay`/playercommands-gate).
- **Gates:** core-boundary, name-leak, `scripts/check-plugins-typecheck.sh`, full `cargo test` (unchanged). No sniper.

## Boundary & safety summary

Entirely CS2/game-layer (`disabled/funvotes/`) — `Server.getCvar`/`setCvar`, `Player.target`/`fromUserId`/`pawn.slay`, `@s2script/votes`. No core/shim change, no op, no sniper. The `sv_gravity` value is validated numeric before it reaches `setCvar` (the console splits on `;`); the toggle values are literal `"0"`/`"1"`. The vote lock is per-context. Both boundary gates stay green.
