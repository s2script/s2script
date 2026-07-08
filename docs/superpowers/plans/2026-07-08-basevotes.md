# basevotes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship SourceMod-style voting — a new engine-generic `@s2script/votes` primitive (chat ballot + revote + result, with an optional live center-HTML tally via a render seam) and a `basevotes` plugin (`sm_vote` + `sm_votekick` + a TopMenu Voting item).

**Architecture:** `@s2script/votes` is an engine-generic prelude module (the vote model + lifecycle + chat ballot/capture over `@s2script/chat`/`@s2script/clients`/`@s2script/timers`) with a `registerTallyRenderer` seam. The CS2 layer registers a center-tally renderer (`show_survival_respawn_status` HTML, re-sent each tick, reusing the menu renderer's path). `basevotes` (CS2) drives it.

**Tech Stack:** Rust (prelude JS in `core/src/v8host.rs`), JavaScript (`@s2script/votes` prelude module + `games/cs2/js/pawn.js` renderer), TypeScript (`.d.ts` + `basevotes` via `s2script build`).

## Global Constraints

- **Charter boundary — core engine-generic; deps game→core.** `@s2script/votes` carries NO game identifiers (ballot/result strings + slot-keyed tallies + the render seam are Source2-generic). CS2 facts (`show_survival_respawn_status`/`loc_token`, `Player`, `ADMFLAG`) live in `pawn.js` + `basevotes`. `scripts/check-core-boundary.sh` + `scripts/test-boundary-nameleak.sh` stay green.
- **One vote per context** (a lock); a 2nd `start` while active returns `false`.
- **Subs are lazy-once** — `Chat.onMessage`/`Clients.onDisconnect` install on the FIRST `Vote.start` (a flag), never per-vote (else handlers accumulate). Return `HookResult.Handled` (`2`) to swallow a captured vote digit.
- **`showLiveTally` default false** (SM chat-only); the tally seam is invoked only when true. `winner` = max-count option index; **tie or zero votes → `null`**.
- **Chat votes are single-digit** — up to 9 options (`1`..`9`).
- **Naming:** PascalCase types (`Vote`), camelCase methods (`start`, `isActive`).
- **Test running:** core tests serial (`cd core && cargo test`); in-isolate prelude tests use `load_plugin_js`/`eval_in_context_string` in `v8host.rs` `frame_tests`. Full spec: `docs/superpowers/specs/2026-07-08-basevotes-design.md`.

---

### Task 1: `@s2script/votes` core (vote model + lifecycle + chat ballot/capture + tally seam)

The engine-generic vote primitive in the prelude.

**Files:**
- Modify: `core/src/v8host.rs` — the `@s2script/votes` module in `INJECTED_STD_PRELUDE` (after the `__s2pkg_clients` assignment, since it composes chat/clients/timers) + `globalThis.__s2pkg_votes`; in-isolate tests in `frame_tests`.
- Create: `packages/votes/package.json`, `packages/votes/index.d.ts`.

**Interfaces:**
- Consumes (prelude globals defined earlier in the string): `globalThis.__s2pkg_chat.Chat` (`toAll(msg)`, `onMessage(fn)`), `globalThis.__s2pkg_clients.Clients` (`all()`, `onDisconnect(fn)`), `globalThis.__s2pkg_timers.delay(ms)` (→ thenable), `globalThis.HookResult.Handled`.
- Produces: `globalThis.__s2pkg_votes = { Vote }` where `Vote = { start(config)→boolean, isActive()→boolean, cancel(), registerTallyRenderer(renderer) }`. `config = { question, options: string[], duration, showLiveTally?, onEnd(result) }`; `result = { winner: number|null, counts: number[], total: number }`; renderer = `{ show(slot, tally), clear(slot) }`, `tally = { question, options: [{label,count}], total, secondsLeft }`.

- [ ] **Step 1: Write the failing in-isolate tests**

Add to `frame_tests` in `core/src/v8host.rs`. The tests override the deps (deferred subs let the override happen before the first `start`):

```rust
#[test]
fn votes_cast_revote_tally_and_winner() {
    init(dummy_logger()).unwrap();
    let out = eval_std("vt1", r#"
        var sent = [], chatHandler = null, delayed = [];
        globalThis.__s2pkg_chat.Chat.toAll = function (m) { sent.push(m); };
        globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
        globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
        globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false},{slot:9,isBot:true}]; };
        globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
        var res = null;
        var ok = globalThis.__s2pkg_votes.Vote.start({ question:"Q", options:["A","B"], duration:2, onEnd:function(r){ res = r; } });
        var handled = chatHandler(0, "1");   // slot0 -> A
        chatHandler(1, "2");                 // slot1 -> B
        chatHandler(0, "2");                 // slot0 REVOTE -> B
        while (delayed.length) delayed.shift()();   // drain the countdown -> end
        JSON.stringify({ ok:ok, handled:handled, counts:res.counts, total:res.total, winner:res.winner });
    "#);
    // slot0 revoted to B, slot1 B -> A:0 B:2, winner index 1
    assert_eq!(out, r#"{"ok":true,"handled":2,"counts":[0,2],"total":2,"winner":1}"#);
    shutdown();
}

#[test]
fn votes_tie_and_zero_are_null_winner_and_lock() {
    init(dummy_logger()).unwrap();
    let out = eval_std("vt2", r#"
        var chatHandler = null, delayed = [];
        globalThis.__s2pkg_chat.Chat.toAll = function () {};
        globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
        globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
        globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false},{slot:1,isBot:false}]; };
        globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
        var V = globalThis.__s2pkg_votes.Vote, res = null;
        V.start({ question:"Q", options:["A","B"], duration:1, onEnd:function(r){ res = r; } });
        var second = V.start({ question:"Q2", options:["A","B"], duration:1, onEnd:function(){} });  // locked out
        var activeMid = V.isActive();
        chatHandler(0, "1"); chatHandler(1, "2");   // 1-1 tie
        while (delayed.length) delayed.shift()();
        JSON.stringify({ second:second, activeMid:activeMid, winner:res.winner, activeEnd:V.isActive() });
    "#);
    assert_eq!(out, r#"{"second":false,"activeMid":true,"winner":null,"activeEnd":false}"#);
    shutdown();
}

#[test]
fn votes_live_tally_renderer_show_and_clear() {
    init(dummy_logger()).unwrap();
    let out = eval_std("vt3", r#"
        var chatHandler = null, delayed = [], shows = [], clears = [];
        globalThis.__s2pkg_chat.Chat.toAll = function () {};
        globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
        globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
        globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false}]; };
        globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
        var V = globalThis.__s2pkg_votes.Vote;
        V.registerTallyRenderer({ show:function(slot,t){ shows.push(slot + ":" + t.options[0].count); }, clear:function(slot){ clears.push(slot); } });
        V.start({ question:"Q", options:["A","B"], duration:1, showLiveTally:true, onEnd:function(){} });
        chatHandler(0, "1");   // A:1
        while (delayed.length) delayed.shift()();
        JSON.stringify({ shows: shows.length > 0 && shows[shows.length-1] === "0:1", cleared: clears.indexOf(0) !== -1 });
    "#);
    assert_eq!(out, r#"{"shows":true,"cleared":true}"#);
    shutdown();
}

#[test]
fn votes_no_live_tally_never_calls_renderer() {
    init(dummy_logger()).unwrap();
    let out = eval_std("vt4", r#"
        var chatHandler = null, delayed = [], calls = 0;
        globalThis.__s2pkg_chat.Chat.toAll = function () {};
        globalThis.__s2pkg_chat.Chat.onMessage = function (fn) { chatHandler = fn; };
        globalThis.__s2pkg_clients.Clients.onDisconnect = function () {};
        globalThis.__s2pkg_clients.Clients.all = function () { return [{slot:0,isBot:false}]; };
        globalThis.__s2pkg_timers.delay = function () { return { then: function (cb) { delayed.push(cb); } }; };
        var V = globalThis.__s2pkg_votes.Vote;
        V.registerTallyRenderer({ show:function(){ calls++; }, clear:function(){ calls++; } });
        V.start({ question:"Q", options:["A","B"], duration:1, onEnd:function(){} });   // showLiveTally omitted -> false
        chatHandler(0, "1");
        while (delayed.length) delayed.shift()();
        String(calls);
    "#);
    assert_eq!(out, "0");
    shutdown();
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cd core && cargo test votes_`
Expected: FAIL — `__s2pkg_votes` is undefined.

- [ ] **Step 3: Implement the votes module**

In `core/src/v8host.rs`, `INJECTED_STD_PRELUDE`, add BEFORE the `globalThis.__s2pkg_* = ...` assignment block (needs `__s2pkg_chat`/`clients`/`timers` only at call-time, but place it near the other module code):

```javascript
  // --- @s2script/votes: chat-ballot voting (revote) + an optional live center tally (a render seam). ---
  var __s2_vote_state = null;             // the single active vote, or null (the per-context lock)
  var __s2_vote_tallyRenderer = null;     // { show(slot, tally), clear(slot) } — CS2 registers it
  var __s2_vote_subInstalled = false;     // lazy-once guard: install onMessage/onDisconnect on first start
  var VOTE_HANDLED = (globalThis.HookResult && globalThis.HookResult.Handled) || 2;

  function __s2_vote_eligibleSlots() {
    var out = [], all = globalThis.__s2pkg_clients.Clients.all();
    for (var i = 0; i < all.length; i++) if (!all[i].isBot) out.push(all[i].slot);
    return out;
  }
  function __s2_vote_counts(st) {
    var counts = [], total = 0;
    for (var i = 0; i < st.options.length; i++) counts.push(0);
    st.votes.forEach(function (idx) { if (idx >= 0 && idx < counts.length) { counts[idx]++; total++; } });
    return { counts: counts, total: total };
  }
  function __s2_vote_showTally(st) {
    if (!st.showLiveTally || !__s2_vote_tallyRenderer) return;
    var c = __s2_vote_counts(st);
    var opts = st.options.map(function (label, i) { return { label: label, count: c.counts[i] }; });
    var tally = { question: st.question, options: opts, total: c.total, secondsLeft: st.secondsLeft };
    var slots = __s2_vote_eligibleSlots();
    for (var i = 0; i < slots.length; i++) { try { __s2_vote_tallyRenderer.show(slots[i], tally); } catch (e) {} }
  }
  function __s2_vote_clearTally(st) {
    if (!st.showLiveTally || !__s2_vote_tallyRenderer) return;
    var slots = __s2_vote_eligibleSlots();
    for (var i = 0; i < slots.length; i++) { try { __s2_vote_tallyRenderer.clear(slots[i]); } catch (e) {} }
  }
  function __s2_vote_castFromChat(slot, text) {
    var st = __s2_vote_state; if (!st) return 0;                    // no active vote -> pass through
    var t = ("" + text).trim();
    if (!/^[0-9]$/.test(t)) return 0;
    var d = parseInt(t, 10);
    if (d < 1 || d > st.options.length) return 0;                  // out of range -> pass through
    st.votes.set(slot, d - 1);                                     // revote replaces
    __s2_vote_showTally(st);
    if (st.votes.size >= __s2_vote_eligibleSlots().length) __s2_vote_end();   // all voted -> end early
    return VOTE_HANDLED;
  }
  function __s2_vote_ensureSubs() {
    if (__s2_vote_subInstalled) return; __s2_vote_subInstalled = true;
    globalThis.__s2pkg_chat.Chat.onMessage(function (slot, text) { return __s2_vote_castFromChat(slot, text); });
    globalThis.__s2pkg_clients.Clients.onDisconnect(function (c) { var st = __s2_vote_state; if (st) st.votes.delete(c.slot); });
  }
  function __s2_vote_tick(st) {
    if (__s2_vote_state !== st) return;                            // ended/cancelled
    if (st.secondsLeft <= 0) { __s2_vote_end(); return; }
    st.secondsLeft--;
    __s2_vote_showTally(st);
    globalThis.__s2pkg_timers.delay(1000).then(function () { __s2_vote_tick(st); });
  }
  function __s2_vote_end() {
    var st = __s2_vote_state; if (!st) return;
    __s2_vote_state = null;                                        // release the lock BEFORE onEnd (so onEnd can start a new vote)
    __s2_vote_clearTally(st);
    var c = __s2_vote_counts(st), winner = null, best = -1, tie = false;
    for (var i = 0; i < c.counts.length; i++) {
      if (c.counts[i] > best) { best = c.counts[i]; winner = i; tie = false; }
      else if (c.counts[i] === best) { tie = true; }
    }
    if (c.total === 0 || tie) winner = null;
    var result = { winner: winner, counts: c.counts, total: c.total };
    if (winner !== null) globalThis.__s2pkg_chat.Chat.toAll("[Vote] Passed: " + st.options[winner] + " (" + Math.round(c.counts[winner] / c.total * 100) + "%)");
    else globalThis.__s2pkg_chat.Chat.toAll("[Vote] Failed — no majority.");
    try { st.onEnd(result); } catch (e) { globalThis.console && console.log("[votes] onEnd threw: " + e); }
  }
  var Vote = {
    start: function (config) {
      if (__s2_vote_state) return false;                          // one vote at a time
      if (!config || !config.question || !config.options || config.options.length < 2) return false;
      __s2_vote_ensureSubs();
      var dur = (config.duration | 0) || 20;
      var st = { question: String(config.question), options: config.options.map(String), votes: new Map(),
                 showLiveTally: !!config.showLiveTally, secondsLeft: dur,
                 onEnd: (typeof config.onEnd === "function") ? config.onEnd : function () {} };
      __s2_vote_state = st;
      globalThis.__s2pkg_chat.Chat.toAll("[Vote] " + st.question + " — " + st.options.map(function (o, i) { return (i + 1) + "=" + o; }).join(", "));
      __s2_vote_showTally(st);
      __s2_vote_tick(st);                                         // starts the countdown + end
      return true;
    },
    isActive: function () { return !!__s2_vote_state; },
    cancel: function () { var st = __s2_vote_state; if (!st) return; __s2_vote_state = null; __s2_vote_clearTally(st); },
    registerTallyRenderer: function (r) { __s2_vote_tallyRenderer = r; },
  };
```

Add to the `globalThis.__s2pkg_* = ...` block (beside `__s2pkg_menu`):
```javascript
  globalThis.__s2pkg_votes = { Vote: Vote };
```

- [ ] **Step 4: Run the tests**

Run: `cd core && cargo test votes_`
Expected: PASS (4). Then `cd core && cargo test` (full suite green) + `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.

- [ ] **Step 5: `.d.ts` + commit**

`packages/votes/package.json`:
```json
{ "name": "@s2script/votes", "version": "0.1.0", "types": "index.d.ts" }
```
`packages/votes/index.d.ts`:
```typescript
/** @s2script/votes — chat-ballot voting with revote + an optional live center tally. NO runtime code (injected at load). */
export interface VoteResult { readonly winner: number | null; readonly counts: number[]; readonly total: number; }
export interface VoteConfig {
  question: string;
  /** 2..9 options (chat votes are single-digit). */
  options: string[];
  /** seconds. */
  duration: number;
  /** show a live center tally (default false — the SM chat-only way). */
  showLiveTally?: boolean;
  onEnd: (result: VoteResult) => void;
}
export interface VoteTally { question: string; options: { label: string; count: number }[]; total: number; secondsLeft: number; }
export interface VoteTallyRenderer { show(slot: number, tally: VoteTally): void; clear(slot: number): void; }
export declare const Vote: {
  /** Start a vote (chat ballot to all connected players). Returns false if one is already active. */
  start(config: VoteConfig): boolean;
  isActive(): boolean;
  /** Abort the active vote (no onEnd). */
  cancel(): void;
  /** Register the live-tally renderer (the CS2 center-HTML renderer). */
  registerTallyRenderer(renderer: VoteTallyRenderer): void;
};
```

```bash
git add core/src/v8host.rs packages/votes/package.json packages/votes/index.d.ts
git commit -m "feat(votes): @s2script/votes — chat-ballot voting + revote + tally seam (engine-generic)"
```

---

### Task 2: CS2 center-tally renderer

Register a tally renderer that shows the live center HTML.

**Files:**
- Modify: `games/cs2/js/pawn.js` — register a tally renderer with `@s2script/votes`.

**Interfaces:**
- Consumes: `globalThis.__s2pkg_votes.Vote.registerTallyRenderer`; `globalThis.__s2pkg_frame.OnGameFrame`; `Events.fireToClient`; `getUserId`/`escapeHtml`/`MENU_TTL` (already in `pawn.js`).
- Produces: a registered tally renderer (no new public API).

- [ ] **Step 1: Implement the renderer**

In `pawn.js`, near the menu center renderer (which already defines `Events`/`OnGameFrame`/`getUserId`/`escapeHtml`/`MENU_TTL`), add a vote-tally renderer block:
```javascript
// --- CS2 vote-tally renderer: the live center HTML for @s2script/votes (NON-freezing; no input). ---
(function () {
  if (!globalThis.__s2pkg_votes) return;
  var Events = globalThis.__s2pkg_events.Events, OnGameFrame = globalThis.__s2pkg_frame.OnGameFrame;
  var tallies = {};       // slot -> current tally
  var pollSub = null;
  function renderTallyHtml(t) {
    var html = "<font class='fontSize-m' color='#ffd700'>" + escapeHtml(t.question) + "</font>";
    for (var i = 0; i < t.options.length; i++) {
      var o = t.options[i];
      html += "<br><font class='fontSize-sm' color='#cccccc'>" + (i + 1) + ". " + escapeHtml(o.label) + " — " + o.count + "</font>";
    }
    html += "<br><font class='fontSize-s' color='#8a8a8a'>" + t.total + " voted &nbsp; " + t.secondsLeft + "s</font>";
    return html;
  }
  function ensurePoll() {
    if (pollSub) return;
    pollSub = OnGameFrame.subscribe(function () {
      for (var slot in tallies) {
        var sl = slot | 0;
        Events.fireToClient(sl, "show_survival_respawn_status", { loc_token: renderTallyHtml(tallies[slot]), duration: 1, userid: __s2_client_userid(sl) });
      }
    });
  }
  function stopIfIdle() { for (var k in tallies) { if (tallies[k]) return; } if (pollSub) { pollSub.dispose(); pollSub = null; } }
  globalThis.__s2pkg_votes.Vote.registerTallyRenderer({
    show: function (slot, tally) { tallies[slot] = tally; ensurePoll(); },
    clear: function (slot) {
      delete tallies[slot]; stopIfIdle();
      Events.fireToClient(slot, "show_survival_respawn_status", { loc_token: " ", duration: 1, userid: __s2_client_userid(slot) });   // wipe
    },
  });
})();
```
(Confirm `__s2_client_userid`, `escapeHtml`, `MENU_TTL`, `OnGameFrame.subscribe(...).dispose()` are the exact names used by the menu center renderer above it; match them.)

- [ ] **Step 2: Regenerate + gates + commit**

Run: `bash scripts/package-addon.sh` then `bash scripts/check-core-boundary.sh && bash scripts/test-boundary-nameleak.sh`.
```bash
git add games/cs2/js/pawn.js
git commit -m "feat(votes/cs2): center-tally renderer (live vote HTML, non-freezing)"
```

---

### Task 3: `basevotes` plugin

**Files:**
- Create: `plugins/basevotes/package.json` (with `s2script.config`), `plugins/basevotes/tsconfig.json`, `plugins/basevotes/src/plugin.ts` (mirror `plugins/basecommands/` structure + `plugins/antiflood/package.json` for the config block).

**Interfaces:**
- Consumes: `@s2script/votes` (`Vote`), `@s2script/commands` (`Commands`), `@s2script/admin` (`ADMFLAG`), `@s2script/chat` (`Chat`), `@s2script/config` (`config`), `@s2script/cs2` (`Player`, `pickPlayer`), `@s2script/topmenu` (`TopMenu`).
- Produces: `sm_vote`, `sm_votekick`, a Voting Commands TopMenu item.

- [ ] **Step 1: package.json + config**

`plugins/basevotes/package.json`:
```json
{
  "name": "@s2script/basevotes",
  "version": "0.1.0",
  "main": "src/plugin.ts",
  "s2script": {
    "apiVersion": "1.x",
    "config": {
      "vote_duration": { "type": "int", "default": 20, "description": "Seconds a vote stays open." },
      "show_live_tally": { "type": "bool", "default": true, "description": "Show the live center-screen tally during a vote (false = chat-only, the SM way)." }
    }
  }
}
```
`plugins/basevotes/tsconfig.json`: copy `plugins/basecommands/tsconfig.json`.

- [ ] **Step 2: the plugin**

`plugins/basevotes/src/plugin.ts`:
```typescript
import { Vote } from "@s2script/votes";
import { Commands } from "@s2script/commands";
import { ADMFLAG } from "@s2script/admin";
import { Chat } from "@s2script/chat";
import { config } from "@s2script/config";
import { Player, pickPlayer } from "@s2script/cs2";
import { TopMenu } from "@s2script/topmenu";

// Parse a command arg string into quoted (or bare) tokens: sm_vote "Kick Rex?" Yes No
function parseTokens(s: string): string[] {
  const out: string[] = [];
  const re = /"([^"]*)"|(\S+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(s)) !== null) out.push(m[1] !== undefined ? m[1] : m[2]);
  return out;
}

function startKickVote(userId: number, name: string): void {
  Vote.start({
    question: "Kick " + name + "?",
    options: ["Yes", "No"],
    duration: config.getInt("vote_duration"),
    showLiveTally: config.getBool("show_live_tally"),
    onEnd: (r) => {
      if (r.winner === 0 && r.counts[0] > r.total / 2) {
        const cur = Player.fromUserId(userId);   // re-resolve at end (pick-time slot may be stale)
        if (cur) cur.kick("Vote kicked");
        Chat.toAll("[Vote] " + name + " was vote-kicked.");
      } else {
        Chat.toAll("[Vote] Kick " + name + " failed.");
      }
    },
  });
}

export function onLoad(): void {
  Commands.registerAdmin("sm_vote", ADMFLAG.VOTE, (ctx) => {
    const toks = parseTokens(ctx.argString);
    if (toks.length < 3) { ctx.reply('Usage: sm_vote "Question" "Opt1" "Opt2" ...'); return; }
    const question = toks[0], options = toks.slice(1, 10);   // up to 9 options (single-digit chat)
    if (!Vote.start({ question, options, duration: config.getInt("vote_duration"), showLiveTally: config.getBool("show_live_tally"),
                      onEnd: (r) => { Chat.toAll(r.winner === null ? "[Vote] No decision." : "[Vote] Result: " + options[r.winner]); } })) {
      ctx.reply("[SM] A vote is already in progress.");
    }
  });

  Commands.registerAdmin("sm_votekick", ADMFLAG.VOTE, (ctx) => {
    const targetStr = ctx.arg(0);
    if (!targetStr) { ctx.reply("Usage: sm_votekick <target>"); return; }
    const targets = Player.target(targetStr, ctx.callerSlot);
    if (targets.length === 0) { ctx.reply("[SM] No matching players."); return; }
    if (targets.length > 1) { ctx.reply("[SM] Ambiguous target."); return; }
    const p = targets[0];
    if (Vote.isActive()) { ctx.reply("[SM] A vote is already in progress."); return; }
    startKickVote(p.userId, p.playerName ?? "player");
  });

  TopMenu.addItem("Voting Commands", { id: "basevotes:votekick", name: "Vote Kick", flags: ADMFLAG.VOTE,
    onSelect: adminSlot => pickPlayer(adminSlot, t => startKickVote(t.userId, t.playerName ?? "player")) });

  console.log("[basevotes] onLoad — sm_vote/sm_votekick registered");
}

export function onUnload(): void { console.log("[basevotes] onUnload"); }
```

- [ ] **Step 3: Build (typecheck) + commit**

Run: `node packages/cli/dist/cli.js build plugins/basevotes` (clean `.s2sp`).
```bash
git add plugins/basevotes
git commit -m "feat(basevotes): sm_vote + sm_votekick + Voting Commands TopMenu item"
```

---

### Task 4: Sniper build + live gate

**Files:** none.

- [ ] **Step 1: Sniper build (Task 1 core prelude)**

Run:
```bash
docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh
```
Expect exit 0, no `error:`, GLIBC `libs2script_core` ≤ 2.30 / `s2script.so` ≤ 2.14.

- [ ] **Step 2: Deploy (plugins/ only)**

```bash
mkdir -p dist/addons/s2script/plugins dist/addons/s2script/configs dist/addons/s2script/data
find plugins -path '*/dist/*.s2sp' -exec cp {} dist/addons/s2script/plugins/ \;
docker compose -f docker/docker-compose.yml restart cs2
```

- [ ] **Step 3: Live gate (human + bots)**

Poll `docker logs s2script-cs2 --since 3m` for `GAMEDATA VALIDATION: 12 ok` + `[basevotes] onLoad`. Then (human joined): `sm_vote "Test?" Yes No` → the chat ballot prints to all; typing `1` then `2` (revote) records + flips; the center tally shows live + ticks down; at 0 the result announces in chat. `sm_votekick <bot>` → a Yes majority kicks the bot. `sm_admin → Voting Commands → Vote Kick` → pick a bot → the vote starts. Confirm `RestartCount=0`, no crash.

- [ ] **Step 4: Commit any live-gate fixes** as `fix(basevotes): <what> (live gate)` with the session trailer.

---

## Self-Review

**Spec coverage:**
- `@s2script/votes` (vote logic + revote + lifecycle + one-vote lock + tally seam) → Task 1 ✅
- `showLiveTally` opt-in (default false; seam only when true) → Task 1 (tests vt3/vt4) ✅
- Chat ballot + capture (Handled) + revote + disconnect-drop → Task 1 ✅
- CS2 center-tally renderer (live HTML, non-freezing, re-sent each tick) → Task 2 ✅
- `basevotes`: `sm_vote` + `sm_votekick` + config (`vote_duration`/`show_live_tally`) + TopMenu Vote Kick item → Task 3 ✅
- Re-resolve the kick target by userId at end → Task 3 (`startKickVote` via `Player.fromUserId`) ✅
- Teardown free (composed subs) → Task 1 (lazy-once subs, no new ledger resource) ✅
- Live gate (human + bots) → Task 4 ✅
- Boundary + one sniper → Tasks 1–3 run gates, Task 4 snipers ✅
- Deferred (voteban/votemap/funvotes, global lock, cooldowns/turnout) + the F1/F2 non-goal → not built ✅

**Placeholder scan:** no TBD/etc. `parseTokens` is complete (a quoted/bare tokenizer). Task 2 notes "confirm the exact names (`__s2_client_userid`/`escapeHtml`/`OnGameFrame.subscribe().dispose()`) match the menu renderer above it" — a concrete verification against sibling code, not a vague TODO.

**Type consistency:** `Vote.start(config)→boolean`, `VoteResult {winner,counts,total}`, `VoteTally {question,options:[{label,count}],total,secondsLeft}`, and `registerTallyRenderer({show,clear})` match across Task 1 (module + `.d.ts` + tests), Task 2 (renderer), Task 3 (caller). `config.getInt("vote_duration")`/`getBool("show_live_tally")` match the package.json config keys. `Player.fromUserId`/`pickPlayer`/`Player.target` are the existing `@s2script/cs2` signatures.
