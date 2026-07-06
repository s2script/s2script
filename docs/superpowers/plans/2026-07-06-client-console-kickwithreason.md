# Sub-project 2 — console-print + `Client.kickWithReason` + `Client.ip` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Goal:** Add `Client.print(msg)` (console), `Client.ip`, and `Client.kickWithReason(reason, delaySeconds?)` to `@s2script/clients`.

**Architecture:** Two ABI-appended engine ops — `client_console_print` (→ `IVEngineServer2::ClientPrintf`, proven 6.1b) and `client_address` (→ `GetPlayerNetInfo`→`GetAddress`) — then pure-JS `Client.print`/`ip`/`kickWithReason` in the prelude. Builds on sub-project 1.

**Tech Stack:** Rust (core), C++ (shim), embedded JS (prelude), TypeScript (`.d.ts`).

## Global Constraints

- **Core stays engine-generic** (`slot`/string ops; `IVEngineServer2` is shim-only). Both `check-core-boundary.sh` invocations green.
- **ABI is positional.** Append the two ops AFTER `pawn_commit_suicide` in the SAME order in: the C header struct (`shim/include/s2script_core.h:143`), the Rust mirror (`core/src/v8host.rs:169`), the shim `ops.` assignment (`s2script_mm.cpp:1520`), AND the two test op-structs (`core/src/v8host.rs:5780`, `:5906` — every field listed, add the two as `None`). Order: `client_console_print` then `client_address`.
- **Degrade-never-crash + bot-safe.** `client_console_print` guards `GetPlayerNetInfo(slot) != null` (a print to a null-netchannel fake client segfaults — the exact guard already at `s2script_mm.cpp:606`). `client_address` returns `""` when netinfo is null. Natives no-op / return `""` without the op.
- Commit messages end with `Claude-Session: https://claude.ai/code/session_01G8RQTQTdmeczdE6oihp8Pn` (no backticks in `-m`; `-F -`). ed25519 signing configured.

---

## Task 1: The two engine ops (`client_console_print` + `client_address`)

**Files:**
- Modify: `shim/include/s2script_core.h` — 2 typedefs + 2 struct fields.
- Modify: `core/src/v8host.rs` — 2 Fn types + 2 mirror fields; 2 natives + registration; the 2 test op-structs; cargo tests.
- Modify: `shim/src/s2script_mm.cpp` — 2 shim impls + 2 `ops.` assignments.

**Interfaces produced (Task 2 consumes):** `__s2_client_console_print(slot: number, msg: string): void`; `__s2_client_address(slot: number): string` (`""` for bot/absent).

### C header (`shim/include/s2script_core.h`)

- [ ] **Step 1 — typedefs** beside `s2_pawn_commit_suicide_fn` (`:93`):
  ```c
  // Slice sub-project-2: print one line to a client's developer console (IVEngineServer2::ClientPrintf).
  typedef void (*s2_client_console_print_fn)(int slot, const char* msg);
  // Client IP address ("IP:port"; "" for a bot/no netchannel). Valid until the next call.
  typedef const char* (*s2_client_address_fn)(int slot);
  ```
- [ ] **Step 2 — struct fields** appended AFTER `s2_pawn_commit_suicide_fn pawn_commit_suicide;` (`:143`), before `} S2EngineOps;`:
  ```c
      /* Console print + client address (ban-reason sub-project 2) — APPENDED after pawn_commit_suicide; order is the ABI. */
      s2_client_console_print_fn client_console_print;
      s2_client_address_fn       client_address;
  ```

### Rust mirror + natives (`core/src/v8host.rs`)

- [ ] **Step 3 — Fn type aliases** (beside `PawnCommitSuicideFn`; grep it): `type ClientConsolePrintFn = extern "C" fn(i32, *const c_char);` and `type ClientAddressFn = extern "C" fn(i32) -> *const c_char;` (match the existing alias style).
- [ ] **Step 4 — mirror struct fields** appended AFTER `pub pawn_commit_suicide: Option<PawnCommitSuicideFn>,` (`:169`):
  ```rust
      // --- ban-reason sub-project 2 (APPENDED after pawn_commit_suicide; order is the ABI; do not reorder) ---
      pub client_console_print: Option<ClientConsolePrintFn>,
      pub client_address:       Option<ClientAddressFn>,
  ```
- [ ] **Step 5 — `__s2_client_console_print(slot, msg)` native** — mirror `s2_client_print` (the SayText2 native; grep `fn s2_client_print`): read `slot` (i32) + `msg` (string → CString); `let f = ops.client_console_print?; f(slot, cmsg.as_ptr());`. Degrades to no-op without the op. Register `set_native(scope, global_obj, "__s2_client_console_print", s2_client_console_print);` beside `__s2_client_print` (`:3051`).
- [ ] **Step 6 — `__s2_client_address(slot) -> string` native** — mirror `s2_client_name` (`:2734`, the C-string copy): `let f = ops.client_address?; let p = f(slot); if p.is_null() { rv = "" } else { rv = CStr::from_ptr(p).to_string_lossy() }`. Returns `""` without the op or on null. Register beside `__s2_client_name` / `__s2_client_steamid`.
- [ ] **Step 7 — the two test op-structs.** In the two `S2EngineOps { ... }` literals used by tests (`:5780`, `:5906` — they list every field, mostly `None`), append `client_console_print: None,` and `client_address: None,` in ABI order (after `pawn_commit_suicide: None,`). WITHOUT these the tests won't compile (missing struct fields).
- [ ] **Step 8 — cargo tests.** (a) `__s2_client_console_print(0, "x")` is a no-op with no engine (doesn't throw); (b) `__s2_client_address(0)` returns `""` with no engine. Add to the existing natives-degrade test block (grep for the `client_steamid`/`client_kick` degrade tests). Run `cargo test -p s2script-core -- --test-threads=1` → expect green.

### Shim impls (`shim/src/s2script_mm.cpp`)

- [ ] **Step 9 — `s2_client_console_print`** (beside `s2_client_print`): 
  ```cpp
  static void s2_client_console_print(int slot, const char* msg) {
      if (!s_pEngine || slot < 0 || slot >= 64) return;
      if (!s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot))) return;   // bot / no netchannel — skip (would segfault)
      s_pEngine->ClientPrintf(CPlayerSlot(slot), msg ? msg : "");
  }
  ```
- [ ] **Step 10 — `s2_client_address`** (mirror `s2_client_steamid` at `:643`, static-string):
  ```cpp
  static std::string s_addressBuf;
  static const char* s2_client_address(int slot) {
      s_addressBuf = "";
      if (s_pEngine && slot >= 0 && slot < 64) {
          INetChannelInfo* nci = s_pEngine->GetPlayerNetInfo(CPlayerSlot(slot));
          if (nci) { const char* a = nci->GetAddress(); if (a) s_addressBuf = a; }
      }
      return s_addressBuf.c_str();
  }
  ```
  (Confirm `INetChannelInfo` is included — it's the return type of `GetPlayerNetInfo`; if not visible, include `<inetchannelinfo.h>`.)
- [ ] **Step 11 — `ops.` assignments** appended AFTER `ops.pawn_commit_suicide = &s2_pawn_commit_suicide;` (`:1520`):
  ```cpp
  ops.client_console_print = &s2_client_console_print;   // ban-reason sub-project 2 (order MUST match S2EngineOps)
  ops.client_address       = &s2_client_address;
  ```

- [ ] **Step 12 — sniper build.** `docker run --rm -v "$(pwd):/repo" -w /repo -v s2script-cargo:/usr/local/cargo/registry rust:bullseye bash /repo/scripts/build-sniper.sh`. Expect clean shim + core compile/link. Paste the success tail.
- [ ] **Step 13 — boundary gate** `bash scripts/check-core-boundary.sh` → green.
- [ ] **Step 14 — commit Task 1.**
  ```
  feat(clients): client_console_print + client_address engine ops

  client_console_print = IVEngineServer2::ClientPrintf (bot-skip guarded, 6.1b);
  client_address = GetPlayerNetInfo->GetAddress (static-string, "" for bots).
  ABI-appended after pawn_commit_suicide (header + Rust mirror + shim + 2 test
  structs). Natives __s2_client_console_print / __s2_client_address. Engine-generic.
  ```

---

## Task 2: JS — `Client.print` / `Client.ip` / `Client.kickWithReason` + `.d.ts` + demo

**Files:**
- Modify: `core/src/v8host.rs` — the `@s2script/clients` prelude (the `Client` prototype + module closure; grep `__s2pkg_clients`).
- Modify: `packages/clients/index.d.ts`.
- Modify: `examples/clients-demo/src/plugin.ts`.

**Interfaces consumed:** `__s2_client_console_print`, `__s2_client_address` (Task 1); the sub-project-1 `Client`/`Clients`/`__s2_client_on`; `globalThis.__s2pkg_timers.delay`.

- [ ] **Step 1 — `Client.prototype.print` + the `ip` getter** in the prelude (beside `Client.prototype.chat` / the `steamId` getter):
  ```js
  Client.prototype.print = function (msg) { __s2_client_console_print(this.slot, String(msg) + "\n"); };
  Object.defineProperty(Client.prototype, "ip", { get: function () {
    var a = __s2_client_address(this.slot); if (!a) return ""; var i = a.indexOf(":"); return i < 0 ? a : a.slice(0, i);
  } });
  ```
- [ ] **Step 2 — the `kickWithReason` machinery** in the module closure (verbatim from the design):
  ```js
  var __s2_pendingKicks = {};
  var __s2_kickWired = false;
  function __s2_wireKickOnActive() {
    if (__s2_kickWired) return; __s2_kickWired = true;
    __s2_client_on("active", function (c) {
      var p = __s2_pendingKicks[c.slot]; if (!p) return;
      delete __s2_pendingKicks[c.slot];
      c.chat(p.reason); c.print(p.reason);
      globalThis.__s2pkg_timers.delay(p.delay * 1000).then(function () {
        var cc = __s2_clients.fromSlot(c.slot); if (cc) cc.kick(p.reason);
      });
    });
  }
  Client.prototype.kickWithReason = function (reason, delaySeconds) {
    __s2_wireKickOnActive();
    __s2_pendingKicks[this.slot] = { reason: String(reason), delay: (delaySeconds == null ? 5 : delaySeconds) };
  };
  ```
  (Place these so `Client`, `__s2_client_on`, and `__s2_clients` are all in scope — inside the same closure that already defines them. `globalThis.__s2pkg_timers.delay` is referenced at CALL time, so timer-prelude ordering is irrelevant.)
- [ ] **Step 3 — cargo test** (prelude surface): in a context, `typeof __s2pkg_clients.Client.prototype.print === "function"`, `typeof __s2pkg_clients.Client.prototype.kickWithReason === "function"`, and `new __s2pkg_clients.Client(0).ip === ""` (no engine → `__s2_client_address` → `""` → getter `""`). Also assert the `:port` strip logic if reachable in-isolate (it's pure JS on the native's return; with no engine the return is `""`, so a unit-level check of the strip can be a tiny separate JS eval: set a fake `globalThis.__s2_client_address = () => "1.2.3.4:27005"` in the test context, then `new Client(0).ip === "1.2.3.4"`). Run `cargo test -p s2script-core -- --test-threads=1` → green.
- [ ] **Step 4 — `.d.ts`** (`packages/clients/index.d.ts`, on the `Client` class): add
  ```ts
    /** Print one line to this client's developer console (skipped for bots). */
    print(message: string): void;
    /** This client's IP address (":port" stripped); "" for a bot. */
    readonly ip: string;
    /** Show `reason` (chat + console) once the client is in-game, then kick after `delaySeconds` (default 5). Intended to be called from a Clients.onConnect handler. */
    kickWithReason(reason: string, delaySeconds?: number): void;
  ```
- [ ] **Step 5 — extend `clients-demo`** (`examples/clients-demo/src/plugin.ts`): in `onConnect`, also log `ip=${c.ip}`; add a benign line proving the surface exists WITHOUT kicking real players (do NOT call `kickWithReason` on real connects in the demo — that would kick anyone who joins). E.g. just log `typeof c.kickWithReason` once, or add `c.print("s2script clients-demo: connected")` (a harmless console line to the connecting client). Keep it non-destructive.
- [ ] **Step 6 — typecheck + build.** `bash scripts/check-plugins-typecheck.sh` (expect `examples/clients-demo` OK + PASS); `node packages/cli/dist/cli.js build examples/clients-demo` (expect the `.s2sp`).
- [ ] **Step 7 — commit Task 2.**
  ```
  feat(clients): Client.print / Client.ip / Client.kickWithReason + demo

  print -> __s2_client_console_print; ip -> __s2_client_address (":port" stripped);
  kickWithReason = pending map + one lazily-wired persistent onActive (deliver
  chat+console then delay->isValid-guarded kick). .d.ts + clients-demo extended.
  ```

---

## Post-tasks (controller)

- Deploy: `package-addon.sh` (Task 1's fresh sniper binaries) → recreate `configs` (chmod 777) + `admins.json` → copy the 7 base `.s2sp` + `clients-demo` → `docker compose restart cs2`.
- Live gate (bots-provable): `Client.print` to a bot is skipped (no crash); `Clients.all()[0].ip` is `""` for a bot (no crash); the prelude surface loads; 6.18 ban path + sub-project-1 events unaffected; `RestartCount=0`. The VISUAL/real-client confirmations (a human sees the console line + their real `.ip` + a full `kickWithReason` admit→message→kick) → the **deferred live test** (same human-client bucket as sub-project 1's fresh-connect).
- Final whole-branch review → merge → push.
