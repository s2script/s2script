// @s2script/cs2 — hudkit: a generic Panorama component library over one shared custom_hud_layout.
//
// WHY THIS EXISTS. CustomHudLayout.create() is the primitive: it drives panel ids and dialog
// variables that some .xml declares. Used directly, every plugin needs its OWN layout, which means
// every plugin author must open Workshop Tools and publish an addon before drawing a single row.
// That is a non-starter for a plugin ecosystem, and it does not scale for a second reason:
//
//   `CCSCustomHudLayout` interns every panel id, class name and dialog variable the SERVER
//   references into three networked vectors, each capped at 1024 ("The maximum number of panel ids
//   has been reached"). Those vectors belong to the ENTITY, not the layout, and every plugin shares
//   them. 1024 is generous for one plugin and consumed multiplicatively by bespoke ones: at ~69 ids
//   per private layout the wall arrives around plugin 14, and it fails late — plugin #4 breaking
//   plugin #1 by existing.
//
// A shared pool of generic components inverts that: fixed ids, reused by everyone, so two plugins
// showing a list cost what one does. Interning is lazy — a name costs nothing until first
// referenced — so a generous pool stays free until used. Cost becomes proportional to what is
// on screen globally, paid once, rather than to plugin count.
//
// So plugin authors describe DATA (rows, titles, handlers) and never touch an id. Paging,
// selection, per-player state, the two-phase reveal and the intern budget are all handled here.
//
// ES5 IIFE, concatenated after ui.js. Decorates the existing `ui` factory (`__s2pkg_game_ctx.ui`)
// rather than editing it.
(function () {
  var prevUi = globalThis.__s2pkg_game_ctx && globalThis.__s2pkg_game_ctx.ui;
  if (typeof prevUi !== "function") return;

  // Six center sheets. NOT arbitrary and NOT an engine limit: it is exactly how many `s2_m*` panel
  // trees `s2script_lib.xml` defines, and a server can only address panels the client's layout
  // already contains. Raising this without adding the markup would hand out sheets that paint
  // nothing.
  //
  // It was 2 for as long as nothing needed more, and then three surfaces in one plugin (a shop, a
  // round log, an admin queue) plus the framework's own menu renderer wanted four between them.
  // The failure was quiet — `modal pool exhausted` in the server log, a shop silently degrading to
  // the chat menu and a round log to the console — which is the worst way for a budget to be spent.
  //
  // The real ceiling is the intern budget (1024 per vector, see INTERN_CAP). Each sheet costs ~51
  // panel ids; six puts the layout at 432 of 1024, leaving room for roughly as many again.
  //
  // A client on an older addon has only `s2_m0`/`s2_m1`. Sheets 2-5 address ids that client does
  // not have, and Panorama ignores unknown ids silently — so such a client sees nothing for the
  // third concurrent sheet rather than breaking. Claims are handed out lowest-first, so the common
  // case keeps working on an old addon.
  var MODALS = 6;
  var ROWS = 8;
  var DETAIL = 4;
  var FOOTERS = 5;
  var TOASTS = 4;
  var BADGES = 4;

  // The engine refuses at >= 1024 per vector (CMP dword [..], 0x400 / JL past the warning, three
  // byte-identical 331-byte find-or-add routines). Warn well before that: the failure mode is a
  // value silently not arriving, which is far harder to diagnose than a log line.
  //
  // THREE SEPARATE BUDGETS, not one shared pool of 3072. Panel ids, class names and dialog
  // variables each get their own 1024, so they must be counted separately — a single combined
  // total is wrong in both directions: it cries wolf at 900 spread evenly, and stays quiet while
  // one vector alone runs out. Note also that `setText` spends from TWO of them, interning its
  // panel id and its variable name into different vectors.
  var INTERN_CAP = 1024;
  var INTERN_WARN_AT = 850;

  var CLS = {
    hide: "s2-hide",
    selected: "s2-li-selected",
    disabled: "s2-li-disabled"
  };
  // The fade class differs per component family — one shared "out" class would transition the
  // wrong property on the wrong element.
  var FADE = { sheet: "s2-sheet-out", toast: "s2-toast-out", badge: "s2-hudbadge-out",
               callout: "s2-callout-out", banner: "s2-banner-out", dash: "s2-dash-out" };
  var TOAST_VARIANT = { good: "s2-toast-good", warn: "s2-toast-warn", bad: "s2-toast-bad" };
  var CALLOUT_VARIANT = { good: "s2-callout-good", warn: "s2-callout-warn", bad: "s2-callout-bad" };
  var BADGE_ACCENT = { accent: "s2-hudbadge-accent", good: "s2-hudbadge-good",
                       warn: "s2-hudbadge-warn", bad: "s2-hudbadge-bad" };
  var BTN_VARIANT = { primary: "s2-btn-primary", good: "s2-btn-good", bad: "s2-btn-bad",
                      warn: "s2-btn-warn", ghost: "s2-btn-ghost" };
  // Row tone. Applied to the row BUTTON, so the stylesheet tints the cell through a descendant
  // rule the way `.s2-btn-good .s2-btn-label` already does — the row's own classes stay untouched,
  // which is what keeps `selected` and `disabled` composable with a tone.
  var LI_TONE = { good: "s2-li-good", warn: "s2-li-warn", bad: "s2-li-bad" };
  var SHEET_WIDTH = { sm: "s2-sheet-sm", md: "", lg: "s2-sheet-lg", xl: "s2-sheet-xl" };
  var CORNER = { tl: "s2-corner-tl", tr: "s2-corner-tr", bl: "s2-corner-bl", br: "s2-corner-br" };

  function log(msg) { if (globalThis.console) console.log("[s2script/ui] " + msg); }
  function uiOk(value) { return { ok: true, value: value }; }
  function uiFail(code, message) { return { ok: false, error: { code: code, message: message } }; }
  function staleResult() { return uiFail("StaleClient", "hudkit: stale client or component"); }
  function releasedResult(name) { return uiFail("Released", "hudkit: " + name + " has been released"); }
  function errorMessage(err, fallback) {
    return (err && typeof err.message === "string" ? err.message : String(err)) || fallback;
  }

  function slotOf(player) {
    if (typeof player === "number") return player;
    return player && typeof player.slot === "number" ? player.slot : -1;
  }

  // ── the library descriptor ──────────────────────────────────────────────────────────────────
  // Every id below is a LITERAL. Never build one by concatenation at drive time — that mints a
  // fresh intern per call and exhausts the vectors. The table is generated once, here, and the
  // same strings are reused for every plugin, every player and every page.

  var TEXT = {};
  var BUTTONS = [];
  var PANELS = [];

  function declareText(id) { TEXT[id] = id; PANELS.push(id); return id; }
  function declareButton(id) { BUTTONS.push(id); PANELS.push(id); return id; }

  var MODAL = [];
  for (var m = 0; m < MODALS; m++) {
    var base = "s2_m" + m;
    var spec = {
      root: base,
      title: declareText(base + "_title"),
      sub: declareText(base + "_sub"),
      // Containers for the row list and the detail block, so a sheet used as a plain confirm
      // dialog hides them wholesale instead of showing eight empty rows.
      list: base + "_list",
      detailBox: base + "_detail",
      rows: [],
      detail: [],
      footers: []
    };
    PANELS.push(spec.list); PANELS.push(spec.detailBox);
    PANELS.push(base);
    for (var r = 0; r < ROWS; r++) {
      var rid = base + "_r" + r;
      spec.rows.push({
        id: declareButton(rid),
        a: declareText(rid + "_a"),
        b: declareText(rid + "_b"),
        c: declareText(rid + "_c")
      });
    }
    for (var d = 0; d < DETAIL; d++) spec.detail.push(declareText(base + "_d" + d));
    for (var f = 0; f < FOOTERS; f++) {
      spec.footers.push({ id: declareButton(base + "_f" + f), text: declareText(base + "_f" + f + "_t") });
    }
    MODAL.push(spec);
  }

  var TOAST = [];
  for (var t = 0; t < TOASTS; t++) {
    var tid = "s2_t" + t;
    PANELS.push(tid);
    TOAST.push({ id: tid, bar: tid + "_bar", title: declareText(tid + "_title"),
                 msg: declareText(tid + "_msg") });
    PANELS.push(tid + "_bar");
  }

  var BADGE = [];
  for (var b = 0; b < BADGES; b++) {
    var bid = "s2_b" + b;
    PANELS.push(bid);
    BADGE.push({ id: bid, title: declareText(bid + "_title"), text: declareText(bid + "_text") });
  }

  // Vote rail lives on this same lib layout (s2_vote*). voterail.js drives the ids
  // through hudkit.layout. Not a second CustomHudLayout, and not a pooled center sheet — it is
  // one dedicated root, which is why it does not spend from the `s2_m*` pool.
  var VOTE_OPTIONS = 9;
  PANELS.push("s2_vote");
  declareText("s2_vote_q");
  declareText("s2_vote_sub");
  for (var v = 0; v < VOTE_OPTIONS; v++) {
    var vid = "s2_vote_o" + v;
    declareButton(vid);
    declareText(vid + "_t");
    declareText(vid + "_c");
  }

  // Callout / banner / MOTD: one root each, like the vote rail. MOTD is a scrim
  // overlay with OK, not a third center sheet (no s2_m2).
  var MOTD_SECTIONS = 3;
  PANELS.push("s2_callout");
  declareText("s2_callout_title");
  declareText("s2_callout_msg");
  PANELS.push("s2_banner");
  declareText("s2_banner_text");
  PANELS.push("s2_motd");
  declareText("s2_motd_title");
  declareText("s2_motd_sub");
  var MOTD_SECTION = [];
  for (var ms = 0; ms < MOTD_SECTIONS; ms++) {
    MOTD_SECTION.push({ h: declareText("s2_motd_h" + ms), p: declareText("s2_motd_p" + ms) });
  }
  declareText("s2_motd_note");
  declareButton("s2_motd_ok");
  declareText("s2_motd_ok_t");

  // TopMenu hub. One dedicated root (like MOTD / the vote rail), not a pooled `s2_m*` sheet.
  var DASH_TABS = 8;
  var DASH_ROWS = 8;
  PANELS.push("s2_dash");
  declareText("s2_dash_title");
  declareText("s2_dash_sub");
  declareButton("s2_dash_close");
  declareText("s2_dash_close_t");
  var DASH_TAB = [];
  for (var dt = 0; dt < DASH_TABS; dt++) {
    var dtid = "s2_dash_t" + dt;
    DASH_TAB.push({ id: declareButton(dtid), text: declareText(dtid + "_t") });
  }
  var DASH_ROW = [];
  for (var dr = 0; dr < DASH_ROWS; dr++) {
    var drid = "s2_dash_r" + dr;
    DASH_ROW.push({
      id: declareButton(drid),
      a: declareText(drid + "_a"),
      b: declareText(drid + "_b")
    });
  }
  declareText("s2_dash_status");
  declareButton("s2_dash_prev");
  declareText("s2_dash_prev_t");
  declareButton("s2_dash_next");
  declareText("s2_dash_next_t");

  var LIB_DESCRIPTOR = {
    addons: ["3790153369"],
    resource: "panorama/layout/custom_game/s2script_lib.xml",
    hideClass: CLS.hide,
    text: TEXT,
    buttons: BUTTONS,
    meters: {}
  };

  // ── shared state ────────────────────────────────────────────────────────────────────────────
  // Pool claims live HOST-SIDE (`__s2_ui_pool_claim` / `__s2_ui_pool_release`), because nothing in
  // this file can be global: this prelude is evaluated once PER PLUGIN CONTEXT, so `globalThis`
  // here is per-plugin. A previous version kept the claim table on `globalThis.__s2ui_pool` under
  // a comment calling it "HOST-GLOBAL" — it never was. Every plugin's menuhud claimed s2_m0 in
  // its own private table, two plugins' second sheets both got s2_m1, and both painted the SAME
  // panels (ui.js finds the one layout entity by targetname, identically from every context).
  // The host table is keyed by the real calling plugin id (read host-side at claim time — the
  // `owner` argument below only feeds the fallback), and every claim is ledgered so plugin unload
  // frees its slots without trusting the plugin's own cleanup code to have run.
  //
  // `globalThis.__s2ui_pool` survives for two narrower jobs: the claim FALLBACK when the natives
  // are absent (node test mounts; a core predating them — per-context claims are still better
  // than crashing), and the intern ledgers below, which are per-context ON PURPOSE: interning on
  // the entity is find-or-add and every plugin charges the same fixed s2_* names, so one
  // context's count of distinct-names-referenced tracks the entity-wide total closely enough to
  // warn on — no host round-trip per setText needed.

  function pool() {
    if (!globalThis.__s2ui_pool) {
      globalThis.__s2ui_pool = {
        modal: [], badge: [],
        // One ledger per engine vector. Interning is idempotent (find-or-add), so a name costs
        // its slot once and re-setting it forever after is free — these only climb on FIRST use.
        seen: { panelIds: {}, classNames: {}, variables: {} },
        count: { panelIds: 0, classNames: 0, variables: 0 },
        warned: { panelIds: false, classNames: false, variables: false }
      };
    }
    return globalThis.__s2ui_pool;
  }

  function claim(kind, count, owner) {
    if (typeof globalThis.__s2_ui_pool_claim === "function") {
      var got = globalThis.__s2_ui_pool_claim(kind, count);
      return typeof got === "number" ? got : -1;
    }
    var p = pool();
    var taken = p[kind];
    for (var i = 0; i < count; i++) {
      if (!taken[i]) { taken[i] = owner; return i; }
    }
    return -1;
  }

  // Release mirrors claim: host-side when the native exists (owner-checked there — a plugin
  // cannot free a slot another plugin holds), context-local otherwise.
  function releaseSlot(kind, idx) {
    if (typeof globalThis.__s2_ui_pool_release === "function") {
      globalThis.__s2_ui_pool_release(kind, idx);
      return;
    }
    var taken = pool()[kind];
    if (taken) taken[idx] = null;
  }

  /** Charge a name to one vector's ledger, once, and warn as that vector's ceiling approaches. */
  function chargeTo(vector, name) {
    var p = pool();
    if (Object.prototype.hasOwnProperty.call(p.seen[vector], name)) return;
    p.seen[vector][name] = true;
    p.count[vector]++;
    if (p.count[vector] >= INTERN_WARN_AT && !p.warned[vector]) {
      p.warned[vector] = true;
      log("WARNING: " + p.count[vector] + " distinct " + vector + " interned on the HUD entity " +
          "(cap " + INTERN_CAP + " for this vector). Past it the engine refuses the name and the " +
          "value never arrives — the only signal is \"The maximum number of ... has been " +
          "reached\" in the server console.");
    }
  }

  function internPanel(id) { chargeTo("panelIds", id); }
  function internClass(cls) { chargeTo("classNames", cls); }
  function internVar(name) { chargeTo("variables", name); }

  // `__s2pkg_timers.delay()` takes NO callback — it returns a Promise, and a function passed to it
  // is silently ignored. `after(ms, fn)` is the callback form. Toast / callout / banner holds
  // use this; showing a panel does not.
  function afterSeconds(seconds, fn) {
    var t = globalThis.__s2pkg_timers;
    if (t && typeof t.after === "function") { t.after(Math.max(0, seconds) * 1000, fn); return true; }
    if (t && typeof t.delay === "function") {
      var p = t.delay(Math.max(0, seconds) * 1000);
      if (p && typeof p.then === "function") { p.then(fn); return true; }
    }
    return false;
  }

  function makeComponents(hudApi, descriptor) {
    var hud = hudApi.create ? hudApi.create(descriptor || LIB_DESCRIPTOR) : hudApi.hud(descriptor || LIB_DESCRIPTOR);
    // Only the FALLBACK pool stores this; the host natives read the real calling plugin id
    // themselves (a JS-supplied tag was the literal "plugin" for every caller — useless).
    var ownerTag = "plugin";
    function captureBinding(slot) {
      if (typeof hud._captureBinding === "function") return hud._captureBinding(slot);
      return { slot: slot, client: null, view: null, fallback: true };
    }
    function bindingValid(binding) {
      if (!binding) return false;
      if (binding.fallback) {
        return typeof binding._componentIsValid !== "function" || binding._componentIsValid();
      }
      return typeof hud._bindingIsValid === "function" && hud._bindingIsValid(binding);
    }
    function componentBinding(binding, componentIsValid) {
      if (!binding || typeof componentIsValid !== "function") return binding;
      var derived = {};
      for (var key in binding) {
        if (Object.prototype.hasOwnProperty.call(binding, key)) derived[key] = binding[key];
      }
      var inherited = binding._componentIsValid;
      derived._componentIsValid = function () {
        return (typeof inherited !== "function" || inherited()) && componentIsValid();
      };
      return derived;
    }
    function currentBinding(slot) {
      var binding = captureBinding(slot);
      return bindingValid(binding) ? binding : null;
    }
    function withBinding(binding, fn) {
      if (binding && binding.fallback) return bindingValid(binding) ? fn() : "stale client";
      if (typeof hud._withBinding === "function") return hud._withBinding(binding, fn);
      return "stale client";
    }
    function boundDriver(binding, fn, componentIsValid) {
      var driveBinding = componentBinding(binding, componentIsValid);
      return function () {
        var args = arguments;
        return withBinding(driveBinding, function () { return fn.apply(null, args); });
      };
    }
    function resultBoundDriver(binding, fn, componentIsValid) {
      var driveBinding = componentBinding(binding, componentIsValid);
      return function () {
        if (!bindingValid(driveBinding)) return staleResult();
        var args = arguments;
        var result = withBinding(driveBinding, function () { return fn.apply(null, args); });
        if (result && result.ok === false) return result;
        if (!bindingValid(driveBinding)) return staleResult();
        return result && typeof result.ok === "boolean" ? result :
          uiFail("PaintFailed", "hudkit: UI drive failed");
      };
    }
    function staleOpen(name) { throw new Error("hudkit: " + name + ".open failed: stale client or component"); }
    var liveModals = [];
    var modalRoutes = {};
    // Install each engine click route once, during kit initialization in the load window.
    // Claims only replace the current JS destination; a released handle cannot keep listening.
    for (var mi = 0; mi < MODALS; mi++) {
      (function (index) {
        function bind(id) {
          hud.onClick(id, function (player) {
            var routes = modalRoutes[index];
            if (routes && routes[id]) routes[id](player);
          });
        }
        for (var r = 0; r < ROWS; r++) bind(MODAL[index].rows[r].id);
        for (var f = 0; f < FOOTERS; f++) bind(MODAL[index].footers[f].id);
      })(mi);
    }
    var calloutGen = {};
    var bannerGen = {};
    var motdOpen = {};
    var dashSpec = null;
    var dashOpen = {};
    var dashGeneration = 0;
    var DASH_SUPERSEDED = {};
    // Slot-level paint authority survives replacement of the per-open state object. A provider
    // may synchronously open/close/rebind, so a generation kept only on that object cannot fence
    // the obsolete caller that still holds it on its stack.
    var dashPaintTransactions = {};
    var origForget = hud.forget;
    hud.forget = function (slot, client) {
      if (client && typeof hud._disconnectOwnsSlot === "function" && !hud._disconnectOwnsSlot(slot, client)) {
        origForget(slot, client);
        return;
      }
      for (var li = 0; li < liveModals.length; li++) liveModals[li].forget(slot);
      closeMotd(slot, false);
      closeDash(slot, false);
      calloutGen[slot] = (calloutGen[slot] || 0) + 1;
      bannerGen[slot] = (bannerGen[slot] || 0) + 1;
      origForget(slot, client);
    };

    // `set` spends from two vectors: the panel id and the dialog variable name. In this library
    // the two strings are equal by convention, but they are charged to different ledgers.
    //
    // NOTE: every drive here goes through the *ForPlayer* natives, because those are the only two
    // we have sigscanned. The engine also exposes all-player `SetHasClass` / `SetDialogVariableString`
    // (per Valve's point_script.d.ts), and `m_vecPlayerLayoutStates` is per-player embedded state —
    // so a genuinely shared value (a toast title everyone sees) currently costs one state entry per
    // player where it could cost one. Arming the all-player calls is a worthwhile follow-up, gated
    // on confirming they actually paint: Valve's own setup.js uses ForPlayer exclusively and never
    // calls the all-player form once, which is suggestive but not proof.
    //
    // CONFIDENTIALITY: "ForPlayer" is per-slot STORAGE, not per-recipient DELIVERY. Observed live:
    // a spectator sees the spectated player's panels, because the client renders whichever slot it
    // is VIEWING. (That the full state vector reaches every client is an inference from its
    // CUtlVectorEmbeddedNetworkVar type, not packet-captured fact.) Nothing painted through here
    // is private to its slot — see examples/hud-lab/README.md, "Per-slot state is storage, not
    // delivery", including the one-entity-per-recipient escape and its two un-gated questions.
    function setText(slot, id, value) {
      internPanel(id); internVar(id);
      return hud.set(slot, id, value == null ? "" : String(value));
    }
    function setClass(slot, id, cls, on) {
      internPanel(id); internClass(cls);
      return hud.setClass(slot, id, cls, on);
    }
    // show/hide toggle the hide class, so they touch the class vector too.
    function show(slot, id, opts) { internPanel(id); internClass(CLS.hide); return hud.show(slot, id, opts); }
    function hide(slot, id) { internPanel(id); internClass(CLS.hide); return hud.hide(slot, id); }

    // Structured component paints consume the low-level source result directly. The fallback is
    // only for a custom/test HudLayout without `_drive`; it assigns a generic source category and
    // never inspects English text.
    function structuredCall(name, legacy, args) {
      if (hud._drive && typeof hud._drive[name] === "function") {
        return hud._drive[name].apply(hud._drive, args);
      }
      var error = legacy.apply(null, args);
      return error === null || typeof error === "undefined" ? uiOk(undefined) :
        uiFail("PaintFailed", String(error) || "hudkit: UI drive failed");
    }
    function driveSetText(slot, id, value) {
      internPanel(id); internVar(id);
      return structuredCall("setText", hud.setText || hud.set,
        [slot, id, value == null ? "" : String(value)]);
    }
    function driveSetClass(slot, id, cls, on) {
      internPanel(id); internClass(cls);
      return structuredCall("setClass", hud.setClass, [slot, id, cls, on]);
    }
    function driveShow(slot, id, opts) {
      internPanel(id); internClass(CLS.hide);
      return structuredCall("show", hud.show, [slot, id, opts]);
    }
    function driveHide(slot, id) {
      internPanel(id); internClass(CLS.hide);
      return structuredCall("hide", hud.hide, [slot, id]);
    }
    function driveReveal(slot, id, fadeCls) {
      var result = driveSetClass(slot, id, fadeCls, false);
      return result.ok ? driveShow(slot, id) : result;
    }

    function copyRow(row) {
      if (!row) return null;
      var copy = {};
      for (var key in row) {
        if (Object.prototype.hasOwnProperty.call(row, key)) copy[key] = row[key];
      }
      copy.a = row.a;
      if ("id" in row) copy.id = row.id;
      if ("b" in row) copy.b = row.b;
      if ("c" in row) copy.c = row.c;
      if ("disabled" in row) copy.disabled = row.disabled;
      if ("tone" in row) copy.tone = row.tone;
      return copy;
    }

    function copyDashRow(row) {
      if (!row) return null;
      var copy = {};
      for (var key in row) {
        if (Object.prototype.hasOwnProperty.call(row, key)) copy[key] = row[key];
      }
      copy.id = row.id;
      copy.a = row.a;
      if ("b" in row) copy.b = row.b;
      if ("disabled" in row) copy.disabled = row.disabled;
      return copy;
    }

    function copyDashTab(tab) { return { id: tab.id, title: tab.title }; }

    function rejectDuplicateIds(values, kind, optional) {
      var seen = Object.create(null);
      for (var i = 0; i < values.length; i++) {
        var value = values[i];
        if (!value) continue;
        var id = value.id;
        if (optional && typeof id === "undefined") continue;
        var key = "$" + String(id);
        if (seen[key]) throw new Error("hudkit: duplicate " + kind + " id '" + String(id) + "'");
        seen[key] = true;
      }
    }

    // ── two-phase reveal ──────────────────────────────────────────────────────────────────────
    // `visibility: collapse` cannot be transitioned, so a fade needs: drop the collapse, let a
    // frame land, then clear the fade class. Plugin authors never see this.

    // Show it. No fade-in.
    //
    // This used to set the fade class, un-hide, and clear the fade a frame later so the panel
    // animated in. That made a cosmetic animation a HARD DEPENDENCY of anything appearing at all:
    // if the "clear" half did not run for any reason, the panel was un-hidden at `opacity: 0` —
    // present, cursor-grabbing, and completely invisible, with no error on either side. It cost
    // two rounds of debugging on a live server and looked exactly like a broken addon.
    //
    // A panel that appears instantly is a fine trade for one that cannot silently vanish. The
    // fade class is cleared FIRST, so a panel left transparent by an older build recovers.
    function reveal(slot, id, fadeCls) {
      setClass(slot, id, fadeCls, false);
      show(slot, id);
    }

    // ── toasts ────────────────────────────────────────────────────────────────────────────────

    var toastGen = [];
    var toastNext = 0;
    for (var ti = 0; ti < TOASTS; ti++) toastGen.push(0);

    function toast(slot, opts, retainedBinding) {
      var binding = retainedBinding || currentBinding(slot);
      if (!bindingValid(binding)) return "stale client";
      var o = opts || {};
      var i = toastNext % TOASTS;
      toastNext++;
      var t = TOAST[i];
      // A generation stamp per slot: if this toast is replaced before its hold expires, the older
      // timer must not yank the newer one off screen.
      toastGen[i]++;
      var gen = toastGen[i];
      function currentToast() { return toastGen[i] === gen; }
      var paintText = boundDriver(binding, setText, currentToast);
      var paintClass = boundDriver(binding, setClass, currentToast);
      var paintReveal = boundDriver(binding, reveal, currentToast);
      var paintHide = boundDriver(binding, hide, currentToast);

      paintText(slot, t.title, o.title || "");
      paintText(slot, t.msg, o.message || "");
      var want = TOAST_VARIANT[o.variant] || null;
      for (var vk in TOAST_VARIANT) {
        if (Object.prototype.hasOwnProperty.call(TOAST_VARIANT, vk)) {
          paintClass(slot, t.id, TOAST_VARIANT[vk], TOAST_VARIANT[vk] === want);
        }
      }
      paintReveal(slot, t.id, FADE.toast);

      var hold = o.holdSeconds == null ? 6 : o.holdSeconds;
      if (hold <= 0) return null;
      afterSeconds(hold, function () {
        if (toastGen[i] !== gen || !bindingValid(binding)) return;
        paintClass(slot, t.id, FADE.toast, true);
        afterSeconds(0.3, function () {
          if (toastGen[i] !== gen || !bindingValid(binding)) return;
          paintHide(slot, t.id);
        });
      });
      return null;
    }

    // ── callout (hint: bottom-center, no cursor) ─────────────────────────────────────────────

    function callout(slot, opts, retainedBinding) {
      var binding = retainedBinding || currentBinding(slot);
      if (!bindingValid(binding)) return "stale client";
      var o = opts || {};
      calloutGen[slot] = (calloutGen[slot] || 0) + 1;
      var gen = calloutGen[slot];
      function currentCallout() { return calloutGen[slot] === gen; }
      var paintText = boundDriver(binding, setText, currentCallout);
      var paintClass = boundDriver(binding, setClass, currentCallout);
      var paintShow = boundDriver(binding, show, currentCallout);
      var paintHide = boundDriver(binding, hide, currentCallout);
      var paintReveal = boundDriver(binding, reveal, currentCallout);
      var title = o.title || "";
      var message = o.message || "";
      paintText(slot, "s2_callout_title", title);
      paintText(slot, "s2_callout_msg", message);
      if (!title) paintHide(slot, "s2_callout_title"); else paintShow(slot, "s2_callout_title");
      if (!message) paintHide(slot, "s2_callout_msg"); else paintShow(slot, "s2_callout_msg");
      var want = CALLOUT_VARIANT[o.variant] || null;
      for (var vk in CALLOUT_VARIANT) {
        if (Object.prototype.hasOwnProperty.call(CALLOUT_VARIANT, vk)) {
          paintClass(slot, "s2_callout", CALLOUT_VARIANT[vk], CALLOUT_VARIANT[vk] === want);
        }
      }
      paintReveal(slot, "s2_callout", FADE.callout);
      var hold = o.holdSeconds == null ? 4 : o.holdSeconds;
      if (hold <= 0) return null;
      afterSeconds(hold, function () {
        if (calloutGen[slot] !== gen || !bindingValid(binding)) return;
        paintClass(slot, "s2_callout", FADE.callout, true);
        afterSeconds(0.25, function () {
          if (calloutGen[slot] !== gen || !bindingValid(binding)) return;
          paintHide(slot, "s2_callout");
        });
      });
      return null;
    }

    // ── banner (center-top, one at a time, no cursor) ────────────────────────────────────────

    function banner(slot, opts, retainedBinding) {
      var binding = retainedBinding || currentBinding(slot);
      if (!bindingValid(binding)) return "stale client";
      var o = opts || {};
      bannerGen[slot] = (bannerGen[slot] || 0) + 1;
      var gen = bannerGen[slot];
      function currentBanner() { return bannerGen[slot] === gen; }
      var paintText = boundDriver(binding, setText, currentBanner);
      var paintClass = boundDriver(binding, setClass, currentBanner);
      var paintReveal = boundDriver(binding, reveal, currentBanner);
      var paintHide = boundDriver(binding, hide, currentBanner);
      paintText(slot, "s2_banner_text", o.text == null ? "" : String(o.text));
      paintReveal(slot, "s2_banner", FADE.banner);
      var hold = o.holdSeconds == null ? 5 : o.holdSeconds;
      if (hold <= 0) return null;
      afterSeconds(hold, function () {
        if (bannerGen[slot] !== gen || !bindingValid(binding)) return;
        paintClass(slot, "s2_banner", FADE.banner, true);
        afterSeconds(0.25, function () {
          if (bannerGen[slot] !== gen || !bindingValid(binding)) return;
          paintHide(slot, "s2_banner");
        });
      });
      return null;
    }

    // ── MOTD (scrim + OK). Not a third center sheet. ─────────────────────────────────────────

    function closeMotd(slot, fromClick, expected) {
      var state = motdOpen[slot];
      if (!state || (expected && expected !== state) || !bindingValid(state.binding)) return;
      function currentMotd() { return motdOpen[slot] === state; }
      boundDriver(state.binding, hide, currentMotd)(slot, "s2_motd");
      if (!currentMotd()) return;
      delete motdOpen[slot];
      if (fromClick && typeof state.onClose === "function") state.onClose(slot);
    }

    hud.onClick("s2_motd_ok", function (player) {
      var slot = slotOf(player);
      closeMotd(slot, true, motdOpen[slot]);
    });

    function invalidMotd(slot) {
      return { slot: slot, isValid: function () { return false; }, close: function () {} };
    }

    function motd(slot, opts, retainedBinding) {
      var binding = retainedBinding || currentBinding(slot);
      if (!bindingValid(binding)) return invalidMotd(slot);
      var o = opts || {};
      var state = { binding: binding, onClose: o.onClose || null };
      motdOpen[slot] = state;
      function currentMotd() { return motdOpen[slot] === state; }
      var paintText = boundDriver(binding, setText, currentMotd);
      var paintShow = boundDriver(binding, show, currentMotd);
      var paintHide = boundDriver(binding, hide, currentMotd);
      paintText(slot, "s2_motd_title", o.title == null ? "" : String(o.title));
      var sub = o.subtitle == null ? "" : String(o.subtitle);
      paintText(slot, "s2_motd_sub", sub);
      if (!sub) paintHide(slot, "s2_motd_sub"); else paintShow(slot, "s2_motd_sub");
      var sections = Array.isArray(o.sections) ? o.sections : [];
      for (var i = 0; i < MOTD_SECTIONS; i++) {
        var sec = sections[i] || {};
        var heading = sec.heading == null ? "" : String(sec.heading);
        var body = sec.body == null ? "" : String(sec.body);
        paintText(slot, MOTD_SECTION[i].h, heading);
        paintText(slot, MOTD_SECTION[i].p, body);
        if (!heading) paintHide(slot, MOTD_SECTION[i].h); else paintShow(slot, MOTD_SECTION[i].h);
        if (!body) paintHide(slot, MOTD_SECTION[i].p); else paintShow(slot, MOTD_SECTION[i].p);
      }
      var note = o.note == null ? "" : String(o.note);
      paintText(slot, "s2_motd_note", note);
      if (!note) paintHide(slot, "s2_motd_note"); else paintShow(slot, "s2_motd_note");
      paintText(slot, "s2_motd_ok_t", o.ok == null ? "OK" : String(o.ok));
      paintShow(slot, "s2_motd", { cursor: o.cursor !== false });
      return {
        slot: slot,
        isValid: function () { return motdOpen[slot] === state && bindingValid(binding); },
        close: function () { closeMotd(slot, false, state); }
      };
    }

    // ── dashboard (tabbed TopMenu hub). One spec, one root. Not a modal pool slot. ───────────

    function dashTabs(slot, spec) {
      if (!spec) return [];
      var got = typeof spec.tabs === "function" ? spec.tabs(slot) : (spec.tabs || []);
      return got || [];
    }

    function dashStateValid(state) {
      return !!state && state.componentGeneration === dashGeneration && bindingValid(state.binding);
    }

    function closeDash(slot, fromClick) {
      var st = dashOpen[slot];
      delete dashPaintTransactions[slot];
      if (!st) return;
      delete dashOpen[slot];
      if (!dashStateValid(st)) return;
      boundDriver(st.binding, hide, function () { return dashStateValid(st); })(slot, "s2_dash");
      if (fromClick && st.interactive && typeof st.paintedOnClose === "function") st.paintedOnClose(slot);
    }

    function dashCandidate(slot, st, spec) {
      var rawTabs = dashTabs(slot, spec);
      var tabs = [];
      for (var ti = 0; ti < rawTabs.length; ti++) tabs.push(copyDashTab(rawTabs[ti]));
      rejectDuplicateIds(tabs, "tab", false);
      if (tabs.length === 0) return null;

      var tabId = st.tabId;
      var found = false;
      for (var fi = 0; fi < tabs.length; fi++) {
        if (tabs[fi].id === tabId) { found = true; break; }
      }
      var rowPage = st.rowPage;
      if (!found) { tabId = tabs[0].id; rowPage = 0; }

      var tabPages = Math.max(1, Math.ceil(tabs.length / DASH_TABS));
      var tabPage = st.tabPage;
      if (tabPage >= tabPages) tabPage = tabPages - 1;
      if (tabPage < 0) tabPage = 0;
      var tabSlice = tabs.slice(tabPage * DASH_TABS, tabPage * DASH_TABS + DASH_TABS);

      var rawRows = typeof spec.rows === "function" ? (spec.rows(slot, tabId) || []) : [];
      var rows = [];
      for (var ri = 0; ri < rawRows.length; ri++) rows.push(copyDashRow(rawRows[ri]));
      rejectDuplicateIds(rows, "row", false);
      var rowPages = Math.max(1, Math.ceil(rows.length / DASH_ROWS));
      if (rowPage >= rowPages) rowPage = rowPages - 1;
      if (rowPage < 0) rowPage = 0;
      var paintedOffset = rowPage * DASH_ROWS;
      var rowSlice = rows.slice(paintedOffset, paintedOffset + DASH_ROWS);
      var paintedRows = [];
      for (var pi = 0; pi < rowSlice.length; pi++) {
        paintedRows.push(rowSlice[pi] ?
          { id: rowSlice[pi].id, index: paintedOffset + pi, row: rowSlice[pi] } : null);
      }

      var tabTitle = "";
      for (var tt = 0; tt < tabs.length; tt++) {
        if (tabs[tt].id === tabId) { tabTitle = tabs[tt].title || tabs[tt].id; break; }
      }
      var status = tabTitle;
      if (rowPages > 1) status += "  ·  " + (rowPage + 1) + "/" + rowPages;
      return {
        tabId: tabId, tabPage: tabPage, rowPage: rowPage,
        tabs: tabSlice, rows: rowSlice, paintedTabs: tabSlice.slice(),
        paintedRows: paintedRows, paintedOffset: paintedOffset, paintedTabId: tabId,
        title: typeof spec.title === "function" ? spec.title(slot) : (spec.title || ""),
        subtitle: typeof spec.subtitle === "function" ? spec.subtitle(slot, tabId) : spec.subtitle,
        closeText: spec.closeText || "Close", status: status, rowPages: rowPages,
        onPick: spec.onPick, onClose: spec.onClose
      };
    }

    function paintDash(slot) {
      var st = dashOpen[slot];
      if (!dashStateValid(st)) return staleResult();
      if (!dashSpec) return uiFail("InvalidArgument", "hudkit: dashboard is not configured");
      var spec = dashSpec;
      var transaction = {};
      dashPaintTransactions[slot] = transaction;
      st.interactive = false;
      function current() {
        return dashPaintTransactions[slot] === transaction && dashOpen[slot] === st && dashSpec === spec &&
          dashStateValid(st);
      }
      var candidate;
      try { candidate = dashCandidate(slot, st, spec); }
      catch (err) {
        return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid dashboard data"));
      }
      if (!current()) return DASH_SUPERSEDED;
      if (!candidate) {
        var closeResult = resultBoundDriver(st.binding, driveHide, function () {
          return dashSpec === spec && dashOpen[slot] === st && dashStateValid(st);
        })(slot, "s2_dash");
        if (dashOpen[slot] === st) {
          delete dashPaintTransactions[slot];
          delete dashOpen[slot];
        }
        return closeResult;
      }
      var error = null;
      function drive(fn) {
        var driveBound = resultBoundDriver(st.binding, fn, current);
        return function () {
          if (error !== null || !current()) return;
          var result = driveBound.apply(null, arguments);
          if (!result.ok) error = result;
        };
      }
      var paintText = drive(driveSetText), paintClass = drive(driveSetClass);
      var paintShow = drive(driveShow), paintHide = drive(driveHide);

      paintText(slot, "s2_dash_title", candidate.title);
      paintText(slot, "s2_dash_sub", candidate.subtitle == null ? "" : String(candidate.subtitle));
      paintText(slot, "s2_dash_close_t", candidate.closeText);

      for (var t = 0; t < DASH_TABS; t++) {
        var tab = candidate.tabs[t];
        if (!tab) { paintHide(slot, DASH_TAB[t].id); continue; }
        paintShow(slot, DASH_TAB[t].id);
        paintText(slot, DASH_TAB[t].text, tab.title || tab.id);
        paintClass(slot, DASH_TAB[t].id, "s2-tab-active", tab.id === candidate.tabId);
      }

      for (var r = 0; r < DASH_ROWS; r++) {
        var row = candidate.rows[r];
        if (!row) { paintHide(slot, DASH_ROW[r].id); continue; }
        paintShow(slot, DASH_ROW[r].id);
        paintText(slot, DASH_ROW[r].a, row.a == null ? "" : String(row.a));
        paintText(slot, DASH_ROW[r].b, row.b == null ? "" : String(row.b));
        paintClass(slot, DASH_ROW[r].id, "s2-toggle-disabled", !!row.disabled);
        paintClass(slot, DASH_ROW[r].id, "s2-toggle-active", false);
      }

      paintText(slot, "s2_dash_status", candidate.status);
      paintText(slot, "s2_dash_prev_t", "‹ Prev");
      paintText(slot, "s2_dash_next_t", "Next ›");
      if (candidate.rowPages > 1) { paintShow(slot, "s2_dash_prev"); paintShow(slot, "s2_dash_next"); }
      else { paintHide(slot, "s2_dash_prev"); paintHide(slot, "s2_dash_next"); }
      if (st.pendingRootOpts) {
        paintClass(slot, "s2_dash", FADE.dash, false);
        paintShow(slot, "s2_dash", st.pendingRootOpts);
      }
      if (!current()) return DASH_SUPERSEDED;
      if (error) return error;
      st.tabId = candidate.tabId;
      st.tabPage = candidate.tabPage;
      st.rowPage = candidate.rowPage;
      st.paintedTabs = candidate.paintedTabs;
      st.paintedRows = candidate.paintedRows;
      st.paintedOffset = candidate.paintedOffset;
      st.paintedTabId = candidate.paintedTabId;
      st.paintedOnPick = candidate.onPick;
      st.paintedOnClose = candidate.onClose;
      delete st.pendingRootOpts;
      st.interactive = true;
      return uiOk(undefined);
    }

    hud.onClick("s2_dash_close", function (player) {
      var slot = slotOf(player);
      var st = dashOpen[slot];
      if (!dashStateValid(st) || !st.interactive) return;
      closeDash(slot, true);
    });
    for (var dti = 0; dti < DASH_TABS; dti++) {
      (function (tabIndex) {
        hud.onClick(DASH_TAB[tabIndex].id, function (player) {
          var slot = slotOf(player);
          var st = dashOpen[slot];
          if (!dashStateValid(st) || !st.interactive) return;
          var tab = st.paintedTabs && st.paintedTabs[tabIndex];
          if (!tab) return;
          st.tabId = tab.id;
          st.rowPage = 0;
          paintDash(slot);
        });
      })(dti);
    }
    for (var dri = 0; dri < DASH_ROWS; dri++) {
      (function (rowIndex) {
        hud.onClick(DASH_ROW[rowIndex].id, function (player) {
          var slot = slotOf(player);
          var st = dashOpen[slot];
          if (!dashStateValid(st) || !st.interactive) return;
          var record = st.paintedRows && st.paintedRows[rowIndex];
          if (!record || record.row.disabled) return;
          if (typeof st.paintedOnPick === "function") {
            st.paintedOnPick(slot, st.paintedTabId, record.row, dashSelf.forSlot(slot));
          }
        });
      })(dri);
    }
    hud.onClick("s2_dash_prev", function (player) {
      var slot = slotOf(player);
      var st = dashOpen[slot];
      if (!dashStateValid(st) || !st.interactive) return;
      st.rowPage -= 1;
      paintDash(slot);
    });
    hud.onClick("s2_dash_next", function (player) {
      var slot = slotOf(player);
      var st = dashOpen[slot];
      if (!dashStateValid(st) || !st.interactive) return;
      st.rowPage += 1;
      paintDash(slot);
    });

    var dashSelf;
    function dashboard(spec) {
      var nextSpec = spec || {};
      if (dashSelf) {
        dashGeneration++;
        var slots = [];
        for (var key in dashOpen) {
          if (!dashOpen[key]) continue;
          dashOpen[key].interactive = false;
          dashOpen[key].componentGeneration = dashGeneration;
          delete dashPaintTransactions[key];
          slots.push(Number(key));
        }
        dashSpec = nextSpec;
        var firstError = null;
        for (var si = 0; si < slots.length && dashSpec === nextSpec; si++) {
          try {
            if (dashOpen[slots[si]]) {
              var result = paintDash(slots[si]);
              if (result !== DASH_SUPERSEDED && !result.ok &&
                  result.error.code === "InvalidArgument" && !firstError) {
                firstError = new Error(result.error.message);
              }
            }
          }
          catch (err) { if (!firstError) firstError = err; }
        }
        if (firstError) throw firstError;
        return dashSelf;
      }
      dashSpec = nextSpec;
      dashGeneration++;
      function tryOpenDashBound(slot, opts, binding, generation, rollbackOnFailure) {
        if (generation !== dashGeneration || !bindingValid(binding)) return staleResult();
        var o = opts || {};
        if (dashOpen[slot]) dashOpen[slot].interactive = false;
        delete dashPaintTransactions[slot];
        var candidate = { tabId: o.tab || "", tabPage: 0, rowPage: 0, interactive: false,
          pendingRootOpts: { cursor: o.cursor !== false }, binding: binding,
          componentGeneration: generation };
        dashOpen[slot] = candidate;
        var result;
        try { result = paintDash(slot); }
        catch (err) {
          result = uiFail("PaintFailed", errorMessage(err, "hudkit: dashboard paint failed"));
        }
        if (result === DASH_SUPERSEDED) {
          if (generation === dashGeneration && bindingValid(binding) && dashStateValid(dashOpen[slot])) {
            return uiOk(makeDashView(slot, binding, generation));
          }
          if (generation !== dashGeneration || !bindingValid(binding)) return staleResult();
          return uiFail("PaintFailed", "dashboard open cancelled");
        }
        if (!result.ok) {
          if (rollbackOnFailure !== false && dashOpen[slot] === candidate) {
            delete dashPaintTransactions[slot];
            delete dashOpen[slot];
            try { boundDriver(binding, hide, function () {
              return generation === dashGeneration;
            })(slot, "s2_dash"); }
            catch (_) { /* Preserve the original failure. */ }
          }
          return result;
        }
        if (generation !== dashGeneration || !bindingValid(binding)) return staleResult();
        return uiOk(makeDashView(slot, binding, generation));
      }
      function openDashBound(slot, opts, binding, generation) {
        var result = tryOpenDashBound(slot, opts, binding, generation, false);
        if (!result.ok && result.error.code === "StaleClient") return staleOpen("dashboard");
        if (!result.ok && result.error.code === "InvalidArgument") {
          throw new Error(result.error.message);
        }
        if (!result.ok) return makeDashView(slot, binding, generation);
        return result.value;
      }
      function makeDashView(slot, binding, generation) {
        function valid() { return generation === dashGeneration && bindingValid(binding); }
        return {
          slot: slot,
          isValid: valid,
          open: function (opts) { return openDashBound(slot, opts, binding, generation); },
          tryOpenResult: function (opts) {
            if (!valid()) return staleResult();
            return tryOpenDashBound(slot, opts, binding, generation);
          },
          close: function () { if (valid()) dashSelf.close(slot); },
          isOpen: function () { return valid() && dashSelf.isOpen(slot); },
          setTab: function (tabId) { if (valid()) dashSelf.setTab(slot, tabId); },
          refresh: function () { if (valid()) dashSelf.refresh(slot); },
          tryRefresh: function () {
            if (!valid()) return staleResult();
            return dashSelf.tryRefresh(slot);
          }
        };
      }
      dashSelf = {
        open: function (slot, opts) {
          var binding = currentBinding(slot);
          return openDashBound(slot, opts, binding, dashGeneration);
        },
        tryOpenResult: function (slot, opts) {
          return tryOpenDashBound(slot, opts, captureBinding(slot), dashGeneration);
        },
        close: function (slot) { closeDash(slot, false); },
        isOpen: function (slot) { return dashStateValid(dashOpen[slot]); },
        setTab: function (slot, tabId) {
          var st = dashOpen[slot];
          if (!dashStateValid(st)) return;
          st.tabId = tabId;
          st.rowPage = 0;
          paintDash(slot);
        },
        refresh: function (slot) {
          if (slot == null) { for (var k in dashOpen) { if (dashStateValid(dashOpen[k])) paintDash(Number(k)); } }
          else if (dashStateValid(dashOpen[slot])) paintDash(slot);
        },
        tryRefresh: function (slot) {
          if (slot == null) return uiFail("InvalidArgument", "needs a player slot");
          var binding = captureBinding(slot);
          if (!bindingValid(binding)) return staleResult();
          var st = dashOpen[slot];
          if (!dashStateValid(st)) return uiFail("InvalidArgument", "hudkit: dashboard is not open");
          var result;
          try { result = paintDash(slot); }
          catch (err) {
            return uiFail("PaintFailed", errorMessage(err, "hudkit: dashboard paint failed"));
          }
          if (result === DASH_SUPERSEDED) {
            if (!bindingValid(binding)) return staleResult();
            return dashStateValid(dashOpen[slot]) ? uiOk(undefined) :
              uiFail("PaintFailed", "dashboard refresh cancelled");
          }
          return result;
        },
        forSlot: function (slot) {
          return makeDashView(slot, captureBinding(slot), dashGeneration);
        }
      };
      return dashSelf;
    }

    // ── badges (persistent corner HUD) ────────────────────────────────────────────────────────

    function createBadge(spec, onClaim) {
      var s = spec || {};
      var idx = claim("badge", BADGES, ownerTag);
      if (idx < 0) { log("badge pool exhausted (" + BADGES + " in use) — request ignored"); return null; }
      if (onClaim) onClaim(idx);
      var slotIds = BADGE[idx];
      var cornerCls = CORNER[s.corner] || CORNER.tr;
      var accentCls = BADGE_ACCENT[s.accent] || null;
      var badgeReleased = false;
      var selfBadge;
      function tryShowBadge(slot, data, binding) {
        if (badgeReleased) return releasedResult("badge");
        if (!bindingValid(binding)) return staleResult();
        function currentBadge() { return !badgeReleased; }
        var paintText = resultBoundDriver(binding, driveSetText, currentBadge);
        var paintClass = resultBoundDriver(binding, driveSetClass, currentBadge);
        var paintReveal = resultBoundDriver(binding, driveReveal, currentBadge);
        var error = null;
        function paint(fn) {
          var args = Array.prototype.slice.call(arguments, 1);
          if (error) return;
          var result = fn.apply(null, args);
          if (!result.ok) error = result;
        }
        var dd = data || {};
        paint(paintText, slot, slotIds.title, dd.title || s.title || "");
        paint(paintText, slot, slotIds.text, dd.text || "");
        for (var k in CORNER) {
          if (Object.prototype.hasOwnProperty.call(CORNER, k)) {
            paint(paintClass, slot, slotIds.id, CORNER[k], CORNER[k] === cornerCls);
          }
        }
        for (var ak in BADGE_ACCENT) {
          if (Object.prototype.hasOwnProperty.call(BADGE_ACCENT, ak)) {
            paint(paintClass, slot, slotIds.id, BADGE_ACCENT[ak], BADGE_ACCENT[ak] === accentCls);
          }
        }
        paint(paintReveal, slot, slotIds.id, FADE.badge);
        if (badgeReleased) return releasedResult("badge");
        return error || uiOk(undefined);
      }
      function showBadge(slot, data, binding) {
        tryShowBadge(slot, data, binding);
        return makeBadgeView(slot, binding);
      }
      function makeBadgeView(slot, binding) {
        function valid() { return !badgeReleased && bindingValid(binding); }
        return {
          slot: slot,
          isValid: valid,
          show: function (data) { if (valid()) return showBadge(slot, data, binding); },
          tryShow: function (data) {
            if (badgeReleased) return releasedResult("badge");
            if (!valid()) return staleResult();
            try { return tryShowBadge(slot, data, binding); }
            catch (err) {
              return badgeReleased ? releasedResult("badge") :
                uiFail("PaintFailed", errorMessage(err, "hudkit: badge paint failed"));
            }
          },
          hide: function () { if (valid()) boundDriver(binding, hide, valid)(slot, slotIds.id); }
        };
      }
      selfBadge = {
        show: function (slot, data) {
          return showBadge(slot, data, currentBinding(slot));
        },
        hide: function (slot) { if (!badgeReleased && currentBinding(slot)) hide(slot, slotIds.id); },
        forSlot: function (slot) {
          return makeBadgeView(slot, captureBinding(slot));
        },
        release: function () {
          if (badgeReleased) return;
          badgeReleased = true;
          releaseSlot("badge", idx);
        }
      };
      return selfBadge;
    }
    function badge(spec) { return createBadge(spec, null); }

    // ── modals (title + paged list + detail + footer buttons) ─────────────────────────────────

    function createModal(spec, onClaim) {
      var s = spec || {};
      var idx = claim("modal", MODALS, ownerTag);
      if (idx < 0) { log("modal pool exhausted (" + MODALS + " in use) — request ignored"); return null; }
      if (onClaim) onClaim(idx);
      var ids = MODAL[idx];
      var released = false;
      var routes = {};
      modalRoutes[idx] = routes;
      // Another plugin may have painted this tree since our last claim. Its writes are absent
      // from this context's diff cache, so a new owner must repaint even unchanged values.
      if (typeof hud.invalidatePanelTree === "function") hud.invalidatePanelTree(ids.root);
      function onClick(id, handler) { routes[id] = handler; }
      var pageSize = Math.min(ROWS, s.pageSize || ROWS);
      var widthCls = SHEET_WIDTH[s.width || "md"];
      // Each slot retains navigation state plus only the last completely submitted action table.
      // `interactive` is cleared before every attempt, so a partial drive can never dispatch the
      // previous actions over visuals that may already have changed.
      var open = {};
      var paintTransactions = {};
      var modalSlotEpochs = {};
      var SUPERSEDED = {};
      var self;

      function rowsFor(slot) {
        var got = typeof s.rows === "function" ? s.rows(slot) : (s.rows || []);
        return got || [];
      }

      // Caller buttons first; prev/next take the trailing footer slots, and only when the data
      // actually needs paging — a one-page list shows no pager.
      function footerPlan(slot, pages) {
        var plan = [];
        var mine = typeof s.buttons === "function" ? (s.buttons(slot) || []) : (s.buttons || []);
        for (var i = 0; i < mine.length && plan.length < FOOTERS; i++) {
          plan.push({ text: mine[i].text, variant: mine[i].variant || "ghost", fn: mine[i].onClick });
        }
        if (pages > 1 && plan.length + 2 <= FOOTERS) {
          plan.push({ text: "‹ Prev", variant: "ghost", fn: function (sl) { self.page(sl, -1); } });
          plan.push({ text: "Next ›", variant: "ghost", fn: function (sl) { self.page(sl, 1); } });
        }
        return plan;
      }

      function modalCandidate(slot, st, request) {
        var raw = rowsFor(slot);
        var all = [];
        for (var ai = 0; ai < raw.length; ai++) all.push(copyRow(raw[ai]));
        rejectDuplicateIds(all, "row", true);
        var pages = Math.max(1, Math.ceil(all.length / pageSize));
        var pageNumber = st.page;
        var cursor = st.cursor;
        if (request && request.pageDelta != null) {
          pageNumber = ((pageNumber + request.pageDelta) % pages + pages) % pages;
          cursor = 0;
        } else if (request && request.selectIndex != null) {
          var selected = Math.max(0, Math.min(request.selectIndex, all.length > 0 ? all.length - 1 : 0));
          pageNumber = Math.floor(selected / pageSize);
          cursor = selected % pageSize;
        }
        if (pageNumber >= pages) pageNumber = pages - 1;
        if (pageNumber < 0) pageNumber = 0;
        var paintedOffset = pageNumber * pageSize;
        var page = all.slice(paintedOffset, paintedOffset + pageSize);
        var paintedRows = [];
        for (var pi = 0; pi < page.length; pi++) {
          paintedRows.push(page[pi] ?
            { id: page[pi].id, index: paintedOffset + pi, row: page[pi] } : null);
        }
        var absCursor = paintedOffset + cursor;
        var detail = typeof s.detail === "function" ? (s.detail(slot, page[cursor], absCursor) || []) : [];
        var plan = footerPlan(slot, pages);
        var footerFns = [];
        for (var fi = 0; fi < plan.length; fi++) footerFns.push(plan[fi].fn || null);
        return {
          pageNumber: pageNumber, cursor: cursor, pages: pages, page: page,
          paintedRows: paintedRows, paintedOffset: paintedOffset,
          title: typeof s.title === "function" ? s.title(slot) : (s.title || ""),
          subtitle: typeof s.subtitle === "function" ? s.subtitle(slot) : s.subtitle,
          detail: detail, footerPlan: plan, footerFns: footerFns
        };
      }

      // Footer actions belong to the player's last painted view. Another player's paint
      // must not change the actions behind this player's buttons (including automatic pagers).
      function paint(slot, candidate, rootOpts, request) {
        if (released) return releasedResult("modal");
        var st = candidate || open[slot];
        if (!st) return uiFail("InvalidArgument", "hudkit: modal is not open");
        var expectedState = open[slot];
        if (expectedState) expectedState.interactive = false;
        st.interactive = false;
        var transaction = {};
        paintTransactions[slot] = transaction;
        st.paintTransaction = transaction;
        st.paintExpectedState = expectedState;
        function current() {
          return !released && bindingValid(st.binding) &&
            st.componentEpoch === (modalSlotEpochs[slot] || 0) &&
            paintTransactions[slot] === transaction && open[slot] === expectedState;
        }
        if (!current()) return SUPERSEDED;
        var snapshot;
        try { snapshot = modalCandidate(slot, st, request); }
        catch (err) {
          return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid modal data"));
        }
        if (!current()) return SUPERSEDED;
        var error = null;
        function drive(fn) {
          var driveBound = resultBoundDriver(st.binding, fn, current);
          return function () {
            if (error !== null || !current()) return;
            var result = driveBound.apply(null, arguments);
            if (!result.ok) error = result;
          };
        }
        var paintText = drive(driveSetText), paintClass = drive(driveSetClass);
        var paintShow = drive(driveShow), paintHide = drive(driveHide);

        if (rootOpts) {
          for (var wk in SHEET_WIDTH) {
            if (Object.prototype.hasOwnProperty.call(SHEET_WIDTH, wk) && SHEET_WIDTH[wk] !== "") {
              paintClass(slot, ids.root, SHEET_WIDTH[wk], SHEET_WIDTH[wk] === widthCls);
            }
          }
        }

        paintText(slot, ids.title, snapshot.title);
        paintText(slot, ids.sub, snapshot.subtitle == null ?
          (snapshot.pages > 1 ? (snapshot.pageNumber + 1) + "/" + snapshot.pages : "") : snapshot.subtitle);

        // A sheet with no rows is a confirm dialog, not an empty list — hide the container rather
        // than leaving eight blank rows on screen.
        if (snapshot.page.length === 0) paintHide(slot, ids.list); else paintShow(slot, ids.list);
        for (var i = 0; i < ROWS; i++) {
          var row = snapshot.page[i];
          if (!row) { paintHide(slot, ids.rows[i].id); continue; }
          paintShow(slot, ids.rows[i].id);
          paintText(slot, ids.rows[i].a, row.a);
          paintText(slot, ids.rows[i].b, row.b);
          paintText(slot, ids.rows[i].c, row.c);
          paintClass(slot, ids.rows[i].id, CLS.selected, i === snapshot.cursor);
          // Cosmetic only. onPick still fires for a disabled row so the caller can explain why —
          // a row that silently does nothing reads as a broken menu.
          paintClass(slot, ids.rows[i].id, CLS.disabled, !!row.disabled);
          // Tone is orthogonal to selected/disabled: a row can be the cursor, unaffordable AND
          // bad at once. Every tone is written each paint (like the footer variants) because
          // `setClass` holds no cache — the untaken ones must be cleared or a row keeps the tone
          // of whatever occupied that slot on the previous page.
          var wantTone = LI_TONE[row.tone];
          for (var tk in LI_TONE) {
            if (Object.prototype.hasOwnProperty.call(LI_TONE, tk)) {
              paintClass(slot, ids.rows[i].id, LI_TONE[tk], LI_TONE[tk] === wantTone);
            }
          }
        }

        // ABSOLUTE index, like `onPick` and `cursor()`. Callers index their own full list with
        // it; handing over a page-relative one described the wrong row on every page but the
        // first, while the `row` argument beside it was correct — so it looked right until a
        // list got long enough to page.
        if (snapshot.detail.length === 0) paintHide(slot, ids.detailBox); else paintShow(slot, ids.detailBox);
        for (var d = 0; d < DETAIL; d++) paintText(slot, ids.detail[d],
          snapshot.detail[d] == null ? "" : snapshot.detail[d]);

        for (var f = 0; f < FOOTERS; f++) {
          if (!snapshot.footerPlan[f]) { paintHide(slot, ids.footers[f].id); continue; }
          paintShow(slot, ids.footers[f].id);
          paintText(slot, ids.footers[f].text, snapshot.footerPlan[f].text);
          var wantBtn = BTN_VARIANT[snapshot.footerPlan[f].variant] || BTN_VARIANT.ghost;
          for (var bk in BTN_VARIANT) {
            if (Object.prototype.hasOwnProperty.call(BTN_VARIANT, bk)) {
              paintClass(slot, ids.footers[f].id, BTN_VARIANT[bk], BTN_VARIANT[bk] === wantBtn);
            }
          }
        }
        if (rootOpts) {
          paintClass(slot, ids.root, FADE.sheet, false);
          paintShow(slot, ids.root, rootOpts);
        }
        if (!current()) return SUPERSEDED;
        if (error) return error;
        st.page = snapshot.pageNumber;
        st.cursor = snapshot.cursor;
        st.paintedRows = snapshot.paintedRows;
        st.paintedOffset = snapshot.paintedOffset;
        st.paintedPages = snapshot.pages;
        st.footerFns = snapshot.footerFns.slice();
        open[slot] = st;
        st.interactive = true;
        return uiOk(undefined);
      }

      for (var ri = 0; ri < ROWS; ri++) {
        (function (rowIndex) {
          onClick(ids.rows[rowIndex].id, function (player) {
            var slot = slotOf(player);
            var st = open[slot];
            if (!st || !st.interactive || !bindingValid(st.binding) ||
                st.componentEpoch !== (modalSlotEpochs[slot] || 0)) return;
            var record = st.paintedRows && st.paintedRows[rowIndex];
            if (!record) return;
            st.cursor = rowIndex;
            if (s.onPick) s.onPick(slot, record.index, record.row, self.forSlot(slot));
            if (open[slot] === st) paint(slot);
          });
        })(ri);
      }
      for (var fi = 0; fi < FOOTERS; fi++) {
        (function (fIndex) {
          onClick(ids.footers[fIndex].id, function (player) {
            var slot = slotOf(player);
            if (!open[slot] || !open[slot].interactive || !bindingValid(open[slot].binding) ||
                open[slot].componentEpoch !== (modalSlotEpochs[slot] || 0)) return;
            var fn = open[slot].footerFns && open[slot].footerFns[fIndex];
            if (fn) fn(slot, self.forSlot(slot));
          });
        })(fi);
      }

      function makeModalView(slot, binding, componentEpoch) {
        function valid() {
          return !released && bindingValid(binding) && componentEpoch === (modalSlotEpochs[slot] || 0);
        }
        return {
          slot: slot,
          isValid: valid,
          open: function (opts) {
            if (released) throw new Error("hudkit: modal.open failed: modal has been released");
            if (!valid()) return staleOpen("modal");
            var result = tryOpenBound(slot, opts, binding, componentEpoch);
            if (!result.ok) throw new Error("hudkit: modal.open failed: " + result.error);
            return result.view;
          },
          tryOpen: function (opts) {
            if (!valid()) return { ok: false, error: "hudkit: stale client or component" };
            return tryOpenBound(slot, opts, binding, componentEpoch);
          },
          tryOpenResult: function (opts) {
            if (released) return releasedResult("modal");
            if (!valid()) return staleResult();
            return tryOpenResultBound(slot, opts, binding, componentEpoch);
          },
          close: function () { if (valid()) self.close(slot); },
          isOpen: function () { return valid() && self.isOpen(slot); },
          refresh: function () { if (valid()) self.refresh(slot); },
          tryRefresh: function () {
            if (released) return releasedResult("modal");
            if (!valid()) return staleResult();
            return self.tryRefresh(slot);
          },
          page: function (delta) { if (valid()) self.page(slot, delta); },
          select: function (index) { if (valid()) self.select(slot, index); },
          cursor: function () { return valid() ? self.cursor(slot) : -1; },
          forget: function () { if (valid()) self.forget(slot); }
        };
      }

      function tryOpenResultBound(slot, opts, binding, componentEpoch) {
        if (released) return releasedResult("modal");
        if (!bindingValid(binding) || componentEpoch !== (modalSlotEpochs[slot] || 0)) return staleResult();
        var candidate = { page: 0, cursor: 0, interactive: false, binding: binding,
          componentEpoch: componentEpoch };
        var result;
        try {
          result = paint(slot, candidate, { cursor: !(opts && opts.cursor === false) });
        } catch (err) {
          result = uiFail("PaintFailed", errorMessage(err, "hudkit: modal paint failed"));
        }
        if (result === SUPERSEDED) {
          if (!released && bindingValid(binding) && componentEpoch === (modalSlotEpochs[slot] || 0) &&
              self.isOpen(slot)) {
            return uiOk(makeModalView(slot, binding, componentEpoch));
          }
          if (released) return releasedResult("modal");
          if (!bindingValid(binding) || componentEpoch !== (modalSlotEpochs[slot] || 0)) return staleResult();
          return uiFail("PaintFailed", "modal open cancelled");
        }
        if (released) return releasedResult("modal");
        if (!bindingValid(binding) || componentEpoch !== (modalSlotEpochs[slot] || 0)) return staleResult();
        if (!result.ok) {
          if (!released && paintTransactions[slot] === candidate.paintTransaction &&
              open[slot] === candidate.paintExpectedState) {
            delete paintTransactions[slot];
            delete open[slot];
            try { boundDriver(binding, hide, function () {
              return !released && componentEpoch === (modalSlotEpochs[slot] || 0);
            })(slot, ids.root); }
            catch (_) { /* Preserve the original failure. */ }
          }
          return result;
        }
        return uiOk(makeModalView(slot, binding, componentEpoch));
      }

      function tryOpenBound(slot, opts, binding, componentEpoch) {
        var result = tryOpenResultBound(slot, opts, binding, componentEpoch);
        return result.ok ? { ok: true, view: result.value } :
          { ok: false, error: result.error.message };
      }

      self = {
        tryOpen: function (slot, opts) {
          return tryOpenBound(slot, opts, captureBinding(slot), modalSlotEpochs[slot] || 0);
        },
        tryOpenResult: function (slot, opts) {
          return tryOpenResultBound(slot, opts, captureBinding(slot), modalSlotEpochs[slot] || 0);
        },
        open: function (slot, opts) {
          var result = self.tryOpen(slot, opts);
          if (!result.ok) throw new Error("hudkit: modal.open failed: " + result.error);
          return result.view;
        },
        setCursor: function (slot, on) {
          if (released || !currentBinding(slot)) return;
          return hud._cursorForPanel(slot, ids.root, !!on);
        },
        close: function (slot) {
          if (released) return;
          var st = open[slot];
          delete paintTransactions[slot];
          delete open[slot];
          if (!st || !bindingValid(st.binding) || st.componentEpoch !== (modalSlotEpochs[slot] || 0)) return;
          boundDriver(st.binding, hide, function () {
            return !released && st.componentEpoch === (modalSlotEpochs[slot] || 0);
          })(slot, ids.root);
        },
        isOpen: function (slot) {
          var st = open[slot];
          return !!st && bindingValid(st.binding) && st.componentEpoch === (modalSlotEpochs[slot] || 0);
        },
        refresh: function (slot) {
          if (slot == null) { for (var k in open) { if (self.isOpen(Number(k))) paint(Number(k)); } }
          else if (self.isOpen(slot)) paint(slot);
        },
        tryRefresh: function (slot) {
          if (released) return releasedResult("modal");
          if (slot == null) return uiFail("InvalidArgument", "needs a player slot");
          var binding = captureBinding(slot);
          if (!bindingValid(binding)) return staleResult();
          if (!self.isOpen(slot)) return uiFail("InvalidArgument", "hudkit: modal is not open");
          var result;
          try { result = paint(slot); }
          catch (err) {
            return released ? releasedResult("modal") :
              uiFail("PaintFailed", errorMessage(err, "hudkit: modal paint failed"));
          }
          if (result === SUPERSEDED) {
            if (released) return releasedResult("modal");
            if (!bindingValid(binding)) return staleResult();
            return self.isOpen(slot) ? uiOk(undefined) :
              uiFail("PaintFailed", "modal refresh cancelled");
          }
          return released ? releasedResult("modal") : result;
        },
        page: function (slot, delta) {
          if (!self.isOpen(slot)) return;
          paint(slot, null, null, { pageDelta: delta });
        },
        /**
         * Select by ABSOLUTE index into the full row list, paging to it if needed.
         *
         * Every index a caller sees here is absolute. `onPick` already reported one, and having
         * `cursor()` hand back a page-RELATIVE index instead meant "buy what is selected" bought
         * row N of page 1 no matter which page you were on — silently wrong, and wrong in a way
         * that only shows up once a list is long enough to page.
         */
        select: function (slot, index) {
          if (!self.isOpen(slot)) return;
          paint(slot, null, null, { selectIndex: index });
        },
        /** ABSOLUTE index of the highlighted row — the same space `onPick` reports in. */
        cursor: function (slot) {
          var st = open[slot];
          if (!self.isOpen(slot)) return -1;
          return st.page * pageSize + st.cursor;
        },
        forget: function (slot) {
          modalSlotEpochs[slot] = (modalSlotEpochs[slot] || 0) + 1;
          delete paintTransactions[slot]; delete open[slot];
        },
        forSlot: function (slot) {
          return makeModalView(slot, captureBinding(slot), modalSlotEpochs[slot] || 0);
        },
        release: function () {
          if (released) return;
          for (var sl in open) {
            if (Object.prototype.hasOwnProperty.call(open, sl)) self.close(Number(sl));
          }
          released = true;
          delete modalRoutes[idx];
          releaseSlot("modal", idx);
          for (var rm = 0; rm < liveModals.length; rm++) {
            if (liveModals[rm] === self) { liveModals.splice(rm, 1); break; }
          }
        }
      };
      liveModals.push(self);
      return self;
    }
    function modal(spec) { return createModal(spec, null); }

    function hideAll(slot, retainedBinding) {
      var binding = retainedBinding || currentBinding(slot);
      if (!bindingValid(binding)) return;
      var paintHide = boundDriver(binding, hide);
      calloutGen[slot] = (calloutGen[slot] || 0) + 1;
      bannerGen[slot] = (bannerGen[slot] || 0) + 1;
      paintHide(slot, "s2_callout");
      paintHide(slot, "s2_banner");
      closeMotd(slot, false);
      closeDash(slot, false);
      for (var m2 = 0; m2 < MODALS; m2++) paintHide(slot, MODAL[m2].root);
      for (var t2 = 0; t2 < TOASTS; t2++) paintHide(slot, TOAST[t2].id);
      for (var b2 = 0; b2 < BADGES; b2++) paintHide(slot, BADGE[b2].id);
      withBinding(binding, function () { return hud.cursor(slot, false); });
    }

    function forgetSlot(slot) { hud.forget(slot); }

    function makeKitPlayer(slot, binding) {
      function valid() { return bindingValid(binding); }
      return {
        slot: slot,
        isValid: valid,
        toast: function (spec) { return valid() ? toast(slot, spec, binding) : "stale client"; },
        callout: function (spec) { return valid() ? callout(slot, spec, binding) : "stale client"; },
        banner: function (spec) { return valid() ? banner(slot, spec, binding) : "stale client"; },
        motd: function (spec) { return valid() ? motd(slot, spec, binding) : invalidMotd(slot); },
        hideAll: function () { if (valid()) hideAll(slot, binding); },
        forget: function () { if (valid()) hud.forget(slot, binding.client); }
      };
    }

    return {
      spec: LIB_DESCRIPTOR,
      descriptor: LIB_DESCRIPTOR,
      layout: hud,
      hud: hud,
      /**
       * Spawn the pool's layout entity. Same timing as `createLayout` — player-join, events,
       * commands, or any post-ready callback. `kit` also spawns once a client is active,
       * so this is only needed to force a spawn before the first create() / kit access.
       */
      ensure: function () {
        if (typeof hud.ensure === "function") return hud.ensure();
        return hudApi.createLayout(descriptor || LIB_DESCRIPTOR);
      },
      modal: modal,
      tryModal: function (spec) {
        var claimedIndex = -1;
        try {
          var claimed = createModal(spec, function (idx) { claimedIndex = idx; });
          return claimed ? uiOk(claimed) : uiFail("PoolExhausted", "hudkit: modal pool exhausted");
        } catch (err) {
          if (claimedIndex >= 0) {
            delete modalRoutes[claimedIndex];
            releaseSlot("modal", claimedIndex);
          }
          return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid modal specification"));
        }
      },
      dashboard: dashboard,
      badge: badge,
      tryBadge: function (spec) {
        var claimedIndex = -1;
        try {
          var claimed = createBadge(spec, function (idx) { claimedIndex = idx; });
          return claimed ? uiOk(claimed) : uiFail("PoolExhausted", "hudkit: badge pool exhausted");
        } catch (err) {
          if (claimedIndex >= 0) releaseSlot("badge", claimedIndex);
          return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid badge specification"));
        }
      },
      toast: toast,
      callout: callout,
      banner: banner,
      motd: motd,
      hideAll: hideAll,
      forget: forgetSlot,
      forSlot: function (slot) {
        return makeKitPlayer(slot, captureBinding(slot));
      },
      budget: function () {
        var p = pool();
        return {
          panelIds: p.count.panelIds,
          classNames: p.count.classNames,
          variables: p.count.variables,
          declared: PANELS.length,
          warnAt: INTERN_WARN_AT,
          cap: INTERN_CAP
        };
      }
    };
  }

  // One kit per V8 context. ctx.ui.kit and hudkit.* must be the same instance so modal pool
  // claims stay consistent. Do NOT close over __s2_game_ns("ui"): that proxy throws
  // "ui outside the load window" during run_prelude, which is before __s2_load_ctx exists.
  //
  // The kit binds LAZILY to the ui base the core builds for this plugin's load ctx (the
  // `gameCtx.ui(ctxReg, viaId)` call in __s2_make_ctx). hostKit() used to build its OWN base
  // right here at prelude eval, over a stand-in registrar (`function (fn) { fn(); }`), because
  // menuhud claims its modal before any load ctx exists. That base was a second, parallel ui
  // instance whose onMapStart / onActive / click-hook registrations ran outside the load-window
  // ledger — its `ready` flag latched only if a client happened to be active at prelude eval,
  // its map-start reset never ran, and panels claimed through it PAINTED but never received a
  // click (measured on a live server; plugins worked around it by re-deriving the kit from
  // their own ctx via CustomHudLayout.components(hudkit.spec)). The framework should not need
  // that workaround: everything below waits for the real base instead.
  var sharedKit = null;
  var liveBase = null;     // the load-ctx ui base — set once __s2_make_ctx invokes the factory below
  var liveWaiters = [];    // whenLive callbacks queued before the base exists (menuhud, voterail)

  function defaultKit(hudApi) {
    if (!sharedKit) sharedKit = makeComponents(hudApi, undefined);
    return sharedKit;
  }

  // First real base wins (there is exactly one per context in production: a context hosts one
  // plugin, and __s2_make_ctx builds its game namespaces once per load). Waiters run inside
  // __s2_make_ctx — the load window is open, so a claim made here registers its click hook
  // through the plugin's ledgered registrar, not a stand-in.
  function announceLive(base) {
    if (liveBase) return;
    liveBase = base;
    var waiters = liveWaiters;
    liveWaiters = [];
    for (var w = 0; w < waiters.length; w++) {
      // A throw here would abort __s2_make_ctx and fail the whole plugin load over a broken
      // renderer registration — degrade to a logged miss instead.
      try { waiters[w](defaultKit(base)); }
      catch (e) { log("whenLive callback failed: " + ((e && e.stack) || e)); }
    }
  }

  /** Run `cb(kit)` once this plugin's ctx-bound kit exists (immediately if it already does). */
  function whenLive(cb) {
    if (typeof cb !== "function") return;
    if (liveBase) {
      try { cb(defaultKit(liveBase)); }
      catch (e) { log("whenLive callback failed: " + ((e && e.stack) || e)); }
      return;
    }
    liveWaiters.push(cb);
  }

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var base = prevUi(reg, viaId);
      if (base && (typeof base.hud === "function" || typeof base.create === "function") && !base.components) {
        var kitsByResource = {};
        function kitOf(descriptor) {
          // ui.create already interns layouts by resource; component bindings must share that
          // identity too, or an explicit copy of hudkit.spec installs every handler twice.
          if (!descriptor || descriptor.resource === LIB_DESCRIPTOR.resource) return defaultKit(base);
          var resource = descriptor.resource;
          if (!Object.prototype.hasOwnProperty.call(kitsByResource, resource)) {
            kitsByResource[resource] = makeComponents(base, descriptor);
          }
          return kitsByResource[resource];
        }
        Object.defineProperty(base, "kit", {
          get: function () { return kitOf(); },
          enumerable: true
        });
        base.components = kitOf;
        base.toast = function (slot, spec) { return kitOf().toast(slot, spec); };
        // This factory's only production caller is __s2_make_ctx, so a base landing here IS the
        // plugin's load-ctx instance: adopt it as the one the module-level hudkit resolves to.
        announceLive(base);
      }
      return base;
    }
  });
  if (globalThis.__s2pkg_cs2) {
    function hostKit(name) {
      if (sharedKit) return sharedKit;
      if (!liveBase) {
        throw new Error("s2script: hudkit." + name + " requires plugin context; " +
          "use it in OnPluginStart or a later callback");
      }
      return defaultKit(liveBase);
    }
    function kitFn(name) {
      return function (a, b) {
        return hostKit(name)[name](a, b);
      };
    }
    globalThis.__s2pkg_cs2.hudkit = {
      whenLive: whenLive,
      modal: kitFn("modal"),
      tryModal: kitFn("tryModal"),
      dashboard: kitFn("dashboard"),
      badge: kitFn("badge"),
      tryBadge: kitFn("tryBadge"),
      toast: kitFn("toast"),
      callout: kitFn("callout"),
      banner: kitFn("banner"),
      motd: kitFn("motd"),
      forSlot: kitFn("forSlot"),
      hideAll: kitFn("hideAll"),
      forget: kitFn("forget"),
      ensure: kitFn("ensure"),
      budget: kitFn("budget")
    };
    Object.defineProperty(globalThis.__s2pkg_cs2.hudkit, "layout", {
      get: function () { return hostKit("layout").layout; }
    });
    Object.defineProperty(globalThis.__s2pkg_cs2.hudkit, "hud", {
      get: function () { return hostKit("hud").hud; }
    });
    // The spec/descriptor are static module data (makeComponents always hands back
    // LIB_DESCRIPTOR), so they stay readable before the kit exists — the documented
    // CustomHudLayout.components(hudkit.spec) pattern must not depend on resolution order.
    Object.defineProperty(globalThis.__s2pkg_cs2.hudkit, "spec", {
      get: function () { return LIB_DESCRIPTOR; }
    });
    Object.defineProperty(globalThis.__s2pkg_cs2.hudkit, "descriptor", {
      get: function () { return LIB_DESCRIPTOR; }
    });
  }
})();
