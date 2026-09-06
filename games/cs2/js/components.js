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
    var liveBadges = [];
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
    var toastState = {};
    var calloutState = {};
    var bannerState = {};
    var motdSurfaceState = {};
    var motdOpen = {};
    var motdOpenAttempts = {};
    var dashboardSurfaceState = {};
    var dashboardControllers = [];
    var legacyDashboard = null;
    var DASH_SUPERSEDED = {};
    // Only retained reservations are reconciled. Failed tokenless states require an explicit retry.
    var focusParticipants = [];
    // Explicit invalidations share the one load-window frame subscription installed by ui.js.
    // A record belongs to one component/client lifetime, and can appear in this queue at most once.
    var dirtyUpdates = [];
    function newUpdateRecord(kind, live) {
      return { kind: kind, live: live, result: null, operation: 0, invalidation: 0,
        queued: false, state: null, repaint: null, retained: null, paintable: null,
        failureLogged: false };
    }
    function removeDirty(record) {
      if (!record || !record.queued) return;
      record.queued = false;
      for (var i = dirtyUpdates.length - 1; i >= 0; i--) {
        if (dirtyUpdates[i] === record) dirtyUpdates.splice(i, 1);
      }
    }
    function cancelDirtyState(st) {
      var record = st && st.updateRecord;
      if (!record || record.state !== st) return;
      removeDirty(record);
      record.state = null;
    }
    function queueDirty(st, repaint, retained, paintable) {
      var record = st && st.updateRecord;
      if (!record || !record.live() || !retained(st)) return;
      record.invalidation++;
      record.state = st;
      record.repaint = repaint;
      record.retained = retained;
      record.paintable = paintable;
      if (!record.queued) { record.queued = true; dirtyUpdates.push(record); }
    }
    function ensureDirtyQueued(record) {
      record.queued = true;
      if (dirtyUpdates.indexOf(record) < 0) dirtyUpdates.push(record);
    }
    function beginUpdate(record) {
      if (!record) return null;
      return { operation: ++record.operation, invalidation: record.invalidation };
    }
    function completeUpdate(record, attempt, result) {
      if (!record || !attempt || record.operation !== attempt.operation || !record.live() ||
          !result || typeof result.ok !== "boolean") return;
      record.result = result;
      if (result.ok) {
        record.failureLogged = false;
        // A successful repaint consumes only intent that existed before it started. An invalidate
        // raised by a provider increments the version and remains queued for the following frame.
        if (record.invalidation === attempt.invalidation) removeDirty(record);
      }
    }
    function deferredFailure(record, state, result) {
      if (!record || record.state && record.state !== state || !record.live() ||
          !result || result.ok !== false || !result.error || record.failureLogged) return;
      record.failureLogged = true;
      log("[hudkit] deferred " + record.kind + " update failed: " + result.error.message);
    }
    function settleDeferredState(st, priorInvalidation, result) {
      var record = st && st.updateRecord;
      deferredFailure(record, st, result);
      if (record && record.invalidation === priorInvalidation && record.queued) {
        removeDirty(record);
        record.state = null;
      }
    }
    function takeDirtyUpdates() {
      var records = dirtyUpdates.slice();
      dirtyUpdates = [];
      var pending = [];
      for (var i = 0; i < records.length; i++) {
        pending.push({ record: records[i], operation: records[i].operation,
          invalidation: records[i].invalidation });
      }
      return pending;
    }
    function drainDirtyUpdates(pending) {
      for (var i = 0; i < pending.length; i++) {
        var entry = pending[i];
        var record = entry.record;
        var st = record.state;
        if (!st || !record.live() || !record.retained(st)) {
          record.queued = false; record.state = null; continue;
        }
        // The frame snapshot is taken before focus restoration. A completed restoration fulfills
        // old intent; any invalidate raised by its provider stays in the live queue for next frame.
        if (record.operation !== entry.operation) {
          if (record.invalidation > entry.invalidation) ensureDirtyQueued(record);
          else { record.queued = false; record.state = null; }
          continue;
        }
        // Another provider may invalidate this record during the same frame. Coalesce its old and
        // new intent on the next frame rather than evaluating it after that provider returns.
        if (record.invalidation > entry.invalidation) { ensureDirtyQueued(record); continue; }
        if (!record.paintable(st)) {
          ensureDirtyQueued(record);
          continue;
        }
        record.queued = false;
        var result;
        var operationBefore = record.operation;
        try { result = record.repaint(st); }
        catch (err) { result = uiFail("PaintFailed", errorMessage(err, "hudkit: deferred repaint failed")); }
        // A superseded outer paint has no UiResult of its own. Its nested authoritative update has
        // already completed on this same lifetime record, so observe that result for diagnostics.
        if ((!result || typeof result.ok !== "boolean") && record.operation !== operationBefore &&
            record.result && typeof record.result.ok === "boolean") result = record.result;
        if (record.state === st && record.live() && record.retained(st)) {
          deferredFailure(record, st, result);
          if (!record.queued) record.state = null;
        }
      }
    }
    function focusOptions(opts) {
      try {
        var focus = opts ? opts.focus : undefined;
        if (typeof focus === "undefined") return uiOk(null);
        if (!focus || focus.mode !== "exclusive") return uiFail("InvalidArgument", "focus mode must be exclusive");
        var priority = focus.priority;
        if (typeof priority === "undefined") priority = 0;
        if (typeof priority !== "number" || !isFinite(priority) || Math.floor(priority) !== priority ||
            priority < -2147483648 || priority > 2147483647) {
          return uiFail("InvalidArgument", "focus priority must be a signed int32");
        }
        return uiOk(priority);
      } catch (err) { return uiFail("InvalidArgument", errorMessage(err, "invalid focus options")); }
    }
    function clearInteraction(st) {
      st.interactive = false;
      st.paintedRows = null; st.paintedTabs = null; st.footerFns = null;
      st.paintedOnPick = null; st.paintedOnClose = null;
    }
    function releaseFocus(st) {
      if (!st) return false;
      clearInteraction(st);
      // Host retirement bypasses these mirrors. Forget must not later raw-hide a covered root.
      if (hud._focus) hud._focus.invalidate(st.binding, st.root);
      var token = st.focusToken;
      st.focusToken = null;
      var index = focusParticipants.indexOf(st);
      if (index >= 0) focusParticipants.splice(index, 1);
      if (st.surfaceMap) {
        retireSurface(st.surfaceMap, st, true);
        return true;
      }
      if (!st.focusEnabled) return false;
      if (token && hud._focus) hud._focus.release(token);
      return true;
    }
    function focusAllows(st) {
      if (st.surfaceMap && !surfaceActive(st.surfaceMap, st)) return false;
      return !st.focusEnabled || !!st.focusToken && hud._focus.active(st.focusBinding, st.focusToken);
    }
    function focusPaintable(st) {
      if (st.surfaceMap) {
        var parentState = hud._surface.state(st.token);
        if (!surfaceCurrent(st.surfaceMap, st) || parentState !== "ready" && parentState !== "active") return false;
      }
      if (!st.focusEnabled) return true;
      var state = st.focusToken && hud._focus.state(st.focusToken);
      return state === "ready" || state === "active";
    }
    function invalidationPaintable(st) {
      // A tokenless failed focused state is dormant until an explicit update. invalidate() is one
      // such update and must be allowed to enter prepareFocus() to reserve again.
      return !st.focusEnabled || !st.focusToken || focusPaintable(st);
    }
    // true means a successful logical open that must wait without evaluating any providers.
    function prepareFocus(st, current) {
      if (!st.focusEnabled) return uiOk(false);
      clearInteraction(st);
      st.focusBinding = componentBinding(st.binding, current);
      if (!hud._focus) return uiFail("Unavailable", "surface focus is unavailable");
      if (!st.focusToken) {
        var reserved = hud._focus.reserve(st.focusBinding, st.root, st.focusPriority);
        if (!reserved.ok) return reserved;
        st.focusToken = reserved.value;
        focusParticipants.push(st);
      }
      var state = hud._focus.state(st.focusToken);
      if (state === "invalid") { releaseFocus(st); return staleResult(); }
      if (state === "covered" || state === "waiting") return uiOk(true);
      if (state === "ready") {
        hud._focus.invalidate(st.focusBinding, st.root);
        st.pendingRootOpts = { cursor: st.cursorWanted };
      }
      return uiOk(false);
    }
    function commitFocus(st) {
      if (st.surfaceMap && !activateSurface(st.surfaceMap, st)) return false;
      if (!st.focusEnabled) return true;
      var state = hud._focus.state(st.focusToken);
      return state === "active" || state === "ready" && hud._focus.activate(st.focusBinding, st.focusToken);
    }
    if (hud._focus && typeof hud._focus.onFrame === "function") hud._focus.onFrame(function () {
      // Snapshot before any provider can run. Invalidations raised during focus restoration or
      // ordinary deferred paint therefore belong to the following frame.
      var pendingDirty = takeDirtyUpdates();
      var states = focusParticipants.slice();
      for (var i = 0; i < states.length; i++) {
        var st = states[i];
        if (!st.focusToken) continue;
        if (!st.focusRetained()) { releaseFocus(st); st.focusDiscard(); continue; }
        var state = hud._focus.state(st.focusToken);
        if (state === "invalid") { releaseFocus(st); st.focusDiscard(); }
        else if (state === "covered" || state === "waiting") clearInteraction(st);
        else if (state === "ready") {
          var priorInvalidation = st.updateRecord && st.updateRecord.invalidation;
          try {
            var result = st.focusRepaint();
            if (result && result.ok === false) {
              settleDeferredState(st, priorInvalidation, result);
              releaseFocus(st);
            }
          }
          catch (err) {
            settleDeferredState(st, priorInvalidation,
              uiFail("PaintFailed", errorMessage(err, "hudkit: deferred focus repaint failed")));
            releaseFocus(st);
          }
        }
      }
      drainDirtyUpdates(pendingDirty);
    });
    var origForget = hud.forget;
    hud.forget = function (slot, client) {
      if (client && typeof hud._disconnectOwnsSlot === "function" && !hud._disconnectOwnsSlot(slot, client)) {
        origForget(slot, client);
        return;
      }
      for (var li = 0; li < liveModals.length; li++) liveModals[li].forget(slot);
      closeMotd(slot, false);
      for (var di = 0; di < dashboardControllers.length; di++) {
        closeDashboard(dashboardControllers[di], slot, false);
      }
      retireSurfaceSlot(toastState, slot, true);
      retireSurfaceSlot(calloutState, slot, true);
      retireSurfaceSlot(bannerState, slot, true);
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
    // show/hide toggle the hide class, so they touch the class vector too.
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
    // ── toasts ────────────────────────────────────────────────────────────────────────────────

    var OWNED_TOAST = "cs2:hudkit:owned:toast";
    var OWNED_CALLOUT = "cs2:hudkit:owned:callout";
    var OWNED_BANNER = "cs2:hudkit:owned:banner";
    var OWNED_MOTD = "cs2:hudkit:owned:motd";
    var OWNED_DASHBOARD = "cs2:hudkit:owned:dashboard";
    var TOAST_ROOTS = TOAST.map(function (item) { return item.id; });

    function surfaceSlot(map, slot) {
      var records = map[slot];
      if (!records) { records = []; map[slot] = records; }
      return records;
    }
    function surfaceCurrent(map, st) {
      return !!st && st.live && bindingValid(st.binding) &&
        surfaceSlot(map, st.slot)[st.lane] === st && hud._surface.state(st.token) !== "invalid";
    }
    function surfaceActive(map, st) {
      return surfaceCurrent(map, st) && hud._surface.active(st.binding, st.token);
    }
    function retireSurface(map, st, releaseHost) {
      if (!st || !st.live) return;
      st.live = false;
      var records = surfaceSlot(map, st.slot);
      if (records[st.lane] === st) delete records[st.lane];
      if (hud._surface && hud._surface.invalidate) hud._surface.invalidate(st.binding, st.root);
      if (releaseHost && st.token && hud._surface) hud._surface.release(st.token);
    }
    function retireSurfaceSlot(map, slot, releaseHost, mode) {
      var records = surfaceSlot(map, slot).slice();
      for (var i = 0; i < records.length; i++) {
        if (records[i] && (typeof mode === "undefined" || records[i].mode === mode)) {
          retireSurface(map, records[i], releaseHost);
        }
      }
    }
    function beginSurface(map, binding, key, mode, roots, profile) {
      if (!bindingValid(binding)) return staleResult();
      if (!hud._surface) return uiFail("Unavailable", "surface ownership is unavailable");
      var reserved = hud._surface.reserve(binding, key, mode, roots, profile);
      if (!reserved.ok) return reserved;
      var lane = reserved.value.lane;
      if (lane < 0 || lane >= roots.length) {
        hud._surface.release(reserved.value.token);
        return uiFail("Unavailable", "surface ownership returned an invalid lane");
      }
      var records = surfaceSlot(map, binding.slot);
      var previous = records[lane];
      if (previous) retireSurface(map, previous, false);
      var st = { binding: binding, slot: binding.slot, token: reserved.value.token, lane: lane,
        root: roots[lane], mode: mode, live: true };
      records[lane] = st;
      if (hud._surface.invalidate) hud._surface.invalidate(binding, st.root);
      return uiOk(st);
    }
    function activateSurface(map, st) {
      if (!surfaceCurrent(map, st)) return false;
      var state = hud._surface.state(st.token);
      return state === "active" || state === "ready" && hud._surface.activate(st.binding, st.token);
    }
    function surfaceHandle(map, st) {
      return {
        isValid: function () { return surfaceCurrent(map, st); },
        dispose: function () { retireSurface(map, st, true); }
      };
    }
    function paintSurface(map, st, steps) {
      function current() {
        if (!surfaceCurrent(map, st)) return false;
        var state = hud._surface.state(st.token);
        return state === "ready" || state === "active";
      }
      for (var i = 0; i < steps.length; i++) {
        var step = steps[i];
        var result = resultBoundDriver(st.binding, step.fn, current).apply(null, step.args);
        if (!result.ok) return result;
      }
      return activateSurface(map, st) ? uiOk(surfaceHandle(map, st)) :
        uiFail("PaintFailed", "surface activation failed");
    }
    function scheduleSurface(map, st, hold, fadeClass, fadeSeconds) {
      if (hold <= 0) return;
      afterSeconds(hold, function () {
        if (!surfaceActive(map, st)) return;
        resultBoundDriver(st.binding, driveSetClass, function () { return surfaceActive(map, st); })
          (st.slot, st.root, fadeClass, true);
        afterSeconds(fadeSeconds, function () {
          if (!surfaceActive(map, st)) return;
          retireSurface(map, st, true);
        });
      });
    }
    function finishSurface(map, begun, steps, hold, fadeClass, fadeSeconds) {
      var st = begun.value;
      var result;
      try { result = paintSurface(map, st, steps); }
      catch (err) { result = uiFail("PaintFailed", errorMessage(err, "surface paint failed")); }
      if (!result.ok) { retireSurface(map, st, true); return result; }
      scheduleSurface(map, st, hold, fadeClass, fadeSeconds);
      return result;
    }

    function tryToast(slot, opts, retainedBinding, mode) {
      var binding = retainedBinding || currentBinding(slot);
      var begun = beginSurface(toastState, binding, OWNED_TOAST, mode, TOAST_ROOTS, "visual");
      if (!begun.ok) return begun;
      var st = begun.value, o = opts || {}, t = TOAST[st.lane], steps = [];
      try {
        steps.push({ fn: driveSetText, args: [slot, t.title, o.title || ""] });
        steps.push({ fn: driveSetText, args: [slot, t.msg, o.message || ""] });
        var want = TOAST_VARIANT[o.variant] || null;
        for (var vk in TOAST_VARIANT) if (Object.prototype.hasOwnProperty.call(TOAST_VARIANT, vk)) {
          steps.push({ fn: driveSetClass, args: [slot, t.id, TOAST_VARIANT[vk], TOAST_VARIANT[vk] === want] });
        }
        steps.push({ fn: driveReveal, args: [slot, t.id, FADE.toast] });
        return finishSurface(toastState, begun, steps, o.holdSeconds == null ? 6 : o.holdSeconds,
          FADE.toast, 0.3);
      } catch (err) { retireSurface(toastState, st, true); return uiFail("PaintFailed", errorMessage(err, "toast paint failed")); }
    }
    function toast(slot, opts, retainedBinding) {
      var result = tryToast(slot, opts, retainedBinding, "legacy");
      return result.ok ? null : result.error.message;
    }

    // ── callout (hint: bottom-center, no cursor) ─────────────────────────────────────────────

    function tryCallout(slot, opts, retainedBinding, mode) {
      var binding = retainedBinding || currentBinding(slot);
      var begun = beginSurface(calloutState, binding, OWNED_CALLOUT, mode, ["s2_callout"], "visual");
      if (!begun.ok) return begun;
      var st = begun.value, o = opts || {}, steps = [];
      try {
        var title = o.title || "", message = o.message || "";
        steps.push({ fn: driveSetText, args: [slot, "s2_callout_title", title] });
        steps.push({ fn: driveSetText, args: [slot, "s2_callout_msg", message] });
        steps.push({ fn: title ? driveShow : driveHide, args: [slot, "s2_callout_title"] });
        steps.push({ fn: message ? driveShow : driveHide, args: [slot, "s2_callout_msg"] });
        var want = CALLOUT_VARIANT[o.variant] || null;
        for (var vk in CALLOUT_VARIANT) if (Object.prototype.hasOwnProperty.call(CALLOUT_VARIANT, vk)) {
          steps.push({ fn: driveSetClass, args: [slot, st.root, CALLOUT_VARIANT[vk], CALLOUT_VARIANT[vk] === want] });
        }
        steps.push({ fn: driveReveal, args: [slot, st.root, FADE.callout] });
        return finishSurface(calloutState, begun, steps, o.holdSeconds == null ? 4 : o.holdSeconds,
          FADE.callout, 0.25);
      } catch (err) { retireSurface(calloutState, st, true); return uiFail("PaintFailed", errorMessage(err, "callout paint failed")); }
    }
    function callout(slot, opts, retainedBinding) {
      var result = tryCallout(slot, opts, retainedBinding, "legacy");
      return result.ok ? null : result.error.message;
    }

    // ── banner (center-top, one at a time, no cursor) ────────────────────────────────────────

    function tryBanner(slot, opts, retainedBinding, mode) {
      var binding = retainedBinding || currentBinding(slot);
      var begun = beginSurface(bannerState, binding, OWNED_BANNER, mode, ["s2_banner"], "visual");
      if (!begun.ok) return begun;
      var st = begun.value, o = opts || {}, steps = [];
      try {
        steps.push({ fn: driveSetText, args: [slot, "s2_banner_text", o.text == null ? "" : String(o.text)] });
        steps.push({ fn: driveReveal, args: [slot, st.root, FADE.banner] });
        return finishSurface(bannerState, begun, steps, o.holdSeconds == null ? 5 : o.holdSeconds,
          FADE.banner, 0.25);
      } catch (err) { retireSurface(bannerState, st, true); return uiFail("PaintFailed", errorMessage(err, "banner paint failed")); }
    }
    function banner(slot, opts, retainedBinding) {
      var result = tryBanner(slot, opts, retainedBinding, "legacy");
      return result.ok ? null : result.error.message;
    }

    // ── MOTD (scrim + OK). Not a third center sheet. ─────────────────────────────────────────

    function closeMotd(slot, fromClick, expected) {
      var state = motdOpen[slot];
      if (!fromClick) delete motdOpenAttempts[slot];
      if (!state || (expected && expected !== state)) return;
      if (fromClick && (!state.interactive || !bindingValid(state.binding) || !focusAllows(state))) return;
      var onClose = state.onClose;
      delete motdOpen[slot];
      releaseFocus(state);
      if (fromClick && typeof onClose === "function") onClose(slot);
    }

    hud.onClick("s2_motd_ok", function (player) {
      var slot = slotOf(player);
      closeMotd(slot, true, motdOpen[slot]);
    });

    function invalidMotd(slot) {
      return { slot: slot, isValid: function () { return false; }, close: function () {} };
    }

    function paintMotd(slot, state) {
      var binding = state.binding, o = state.spec;
      function currentMotd() {
        return motdOpen[slot] === state && bindingValid(binding) && surfaceCurrent(motdSurfaceState, state);
      }
      if (!currentMotd()) return staleResult();
      var prepared = prepareFocus(state, currentMotd);
      if (!prepared.ok || prepared.value) return prepared.ok ? uiOk(undefined) : prepared;
      var error = null;
      function drive(fn) {
        return function () {
          if (error || !currentMotd()) return;
          if (!focusPaintable(state)) { error = uiFail("PaintFailed", "focus changed during paint"); return; }
          var result = resultBoundDriver(binding, fn, function () {
            return currentMotd() && focusPaintable(state);
          }).apply(null, arguments);
          if (!result.ok) error = result;
        };
      }
      var paintText = drive(driveSetText), paintShow = drive(driveShow), paintHide = drive(driveHide);
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
      paintShow(slot, "s2_motd", { cursor: state.cursorWanted });
      if (!currentMotd()) { releaseFocus(state); return staleResult(); }
      if (error) { releaseFocus(state); return error; }
      if (!commitFocus(state)) { releaseFocus(state); return uiFail("PaintFailed", "focus activation failed"); }
      state.interactive = true;
      return uiOk(undefined);
    }

    function tryMotd(slot, opts, retainedBinding, mode) {
      var binding = retainedBinding || currentBinding(slot);
      if (!bindingValid(binding)) return staleResult();
      var attempt = {}; motdOpenAttempts[slot] = attempt;
      var validated = focusOptions(opts);
      if (!validated.ok) return validated;
      if (!bindingValid(binding) || motdOpenAttempts[slot] !== attempt) return staleResult();
      var begun = beginSurface(motdSurfaceState, binding, OWNED_MOTD, mode, ["s2_motd"],
        validated.value === null ? "interactive" : "occupancy");
      if (!begun.ok) return begun;
      var o = opts || {};
      var state = begun.value;
      try {
        state.spec = o; state.onClose = o.onClose || null;
        state.focusEnabled = validated.value !== null; state.focusPriority = validated.value;
        state.cursorWanted = o.cursor !== false; state.interactive = false;
        state.surfaceMap = motdSurfaceState;
      } catch (err) {
        retireSurface(motdSurfaceState, state, true);
        return uiFail("InvalidArgument", errorMessage(err, "invalid MOTD options"));
      }
      if (!bindingValid(binding) || motdOpenAttempts[slot] !== attempt) {
        retireSurface(motdSurfaceState, state, true); return staleResult();
      }
      var previous = motdOpen[slot];
      if (previous && previous !== state) {
        clearInteraction(previous);
        previous.live = false;
        previous.focusToken = null;
        var oldIndex = focusParticipants.indexOf(previous);
        if (oldIndex >= 0) focusParticipants.splice(oldIndex, 1);
      }
      motdOpen[slot] = state;
      state.focusRetained = function () { return motdOpen[slot] === state && bindingValid(binding); };
      state.focusDiscard = function () { if (motdOpen[slot] === state) delete motdOpen[slot]; };
      state.focusRepaint = function () { return paintMotd(slot, state); };
      if (state.focusEnabled) {
        state.focusBinding = componentBinding(binding, function () {
          return motdOpen[slot] === state && surfaceCurrent(motdSurfaceState, state);
        });
        if (!hud._focus || typeof hud._focus.reserveLinked !== "function") {
          releaseFocus(state); state.focusDiscard();
          return uiFail("Unavailable", "surface focus is unavailable");
        }
        var linked = hud._focus.reserveLinked(state.focusBinding, state.root, state.focusPriority, state.token);
        if (!linked.ok) { releaseFocus(state); state.focusDiscard(); return linked; }
        state.focusToken = linked.value;
        focusParticipants.push(state);
      }
      var result;
      try { result = paintMotd(slot, state); }
      catch (err) {
        result = uiFail("PaintFailed", errorMessage(err, "MOTD paint failed"));
      }
      if (!result.ok) {
        releaseFocus(state); state.focusDiscard();
        return result;
      }
      return uiOk({
        slot: slot,
        isValid: function () { return motdOpen[slot] === state && surfaceCurrent(motdSurfaceState, state); },
        close: function () { closeMotd(slot, false, state); },
        dispose: function () { closeMotd(slot, false, state); }
      });
    }
    function motd(slot, opts, retainedBinding) {
      var result = tryMotd(slot, opts, retainedBinding, "legacy");
      if (!result.ok) { log("[hudkit] " + result.error.message); return invalidMotd(slot); }
      return result.value;
    }

    // ── dashboard (tabbed TopMenu hub). One physical root, independently owned controllers. ──

    function dashTabs(slot, spec) {
      if (!spec) return [];
      var got = typeof spec.tabs === "function" ? spec.tabs(slot) : (spec.tabs || []);
      return got || [];
    }

    function dashStateValid(st) {
      var controller = st && st.controller;
      return !!controller && !controller.released && st.componentGeneration === controller.generation &&
        bindingValid(st.binding) && controller.open[st.slot] === st;
    }

    function dashOwnedCurrent(st) {
      return dashStateValid(st) && surfaceCurrent(dashboardSurfaceState, st);
    }

    function dashUpdateRecord(controller, slot, binding, generation) {
      var record = controller.updateRecords[slot];
      if (record && record.componentGeneration === generation && record.live()) return record;
      record = newUpdateRecord("dashboard", function () {
        return !controller.released && generation === controller.generation && bindingValid(binding);
      });
      record.componentGeneration = generation;
      controller.updateRecords[slot] = record;
      return record;
    }

    function abandonReplacedDashboard(st) {
      if (!st) return;
      var controller = st.controller;
      cancelDirtyState(st);
      clearInteraction(st);
      var focusIndex = focusParticipants.indexOf(st);
      if (focusIndex >= 0) focusParticipants.splice(focusIndex, 1);
      st.focusToken = null;
      if (controller) {
        if (controller.open[st.slot] === st) delete controller.open[st.slot];
        delete controller.paintTransactions[st.slot];
      }
      retireSurface(dashboardSurfaceState, st, false);
    }

    function closeDashboard(controller, slot, fromClick, expected) {
      if (!controller) return;
      var st = controller.open[slot];
      if (expected && st !== expected) return;
      cancelDirtyState(st);
      delete controller.openAttempts[slot];
      delete controller.paintTransactions[slot];
      if (!st) return;
      delete controller.open[slot];
      var onClose = st.interactive && st.paintedOnClose;
      releaseFocus(st);
      if (fromClick && typeof onClose === "function") onClose(slot);
    }

    function dashCandidate(controller, slot, st) {
      var spec = controller.spec;
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

    function paintDashboard(controller, slot) {
      var st = controller.open[slot], result, transaction = {};
      var updateAttempt = beginUpdate(st && st.updateRecord);
      function ownsFailure() { return st && (controller.paintTransactions[slot] === transaction ||
        st.focusEnabled && !st.focusRetained()); }
      try { result = paintDashboardInner(controller, slot, transaction); }
      catch (err) {
        completeUpdate(st && st.updateRecord, updateAttempt,
          uiFail("PaintFailed", errorMessage(err, "hudkit: dashboard paint failed")));
        if (ownsFailure()) {
          if (st && st.controller.open[slot] === st) st.retryableFailure = true;
          releaseFocus(st);
        }
        throw err;
      }
      if (result !== DASH_SUPERSEDED) completeUpdate(st && st.updateRecord, updateAttempt, result);
      if (ownsFailure() && (result === DASH_SUPERSEDED || !result.ok)) {
        if (result !== DASH_SUPERSEDED && st && st.controller.open[slot] === st) {
          st.retryableFailure = true;
        }
        releaseFocus(st);
      }
      return result;
    }

    function ensureDashboardOwnership(st) {
      if (!dashStateValid(st)) return staleResult();
      if (dashOwnedCurrent(st)) return uiOk(undefined);
      if (!st.opening && !st.retryableFailure) return uiFail("Released", "dashboard surface released");
      if (!hud._surface || typeof hud._surface.reserve !== "function") {
        return uiFail("Unavailable", "surface ownership is unavailable");
      }
      var controller = st.controller;
      var previous = surfaceSlot(dashboardSurfaceState, st.slot)[0];
      var reserved = hud._surface.reserve(st.binding, OWNED_DASHBOARD, controller.mode, ["s2_dash"],
        st.focusEnabled ? "occupancy" : "interactive");
      if (!reserved.ok) return reserved;
      if (reserved.value.lane !== 0) {
        hud._surface.release(reserved.value.token);
        return uiFail("Unavailable", "surface ownership returned an invalid lane");
      }
      if (previous && previous !== st) abandonReplacedDashboard(previous);
      st.token = reserved.value.token;
      st.lane = 0;
      st.live = true;
      surfaceSlot(dashboardSurfaceState, st.slot)[0] = st;
      if (hud._surface.invalidate) hud._surface.invalidate(st.binding, st.root);
      if (!st.focusEnabled) return uiOk(undefined);
      st.focusBinding = componentBinding(st.binding, st.focusRetained);
      if (!hud._focus || typeof hud._focus.reserveLinked !== "function") {
        releaseFocus(st);
        return uiFail("Unavailable", "surface focus is unavailable");
      }
      var linked = hud._focus.reserveLinked(st.focusBinding, st.root, st.focusPriority, st.token);
      if (!linked.ok) { releaseFocus(st); return linked; }
      st.focusToken = linked.value;
      focusParticipants.push(st);
      return uiOk(undefined);
    }

    function dashboardInvalidationPaintable(st) {
      if (dashStateValid(st) && st.retryableFailure) return true;
      if (!dashOwnedCurrent(st)) return false;
      var parentState = hud._surface.state(st.token);
      return (parentState === "ready" || parentState === "active") && invalidationPaintable(st);
    }

    function paintDashboardInner(controller, slot, transaction) {
      var st = controller.open[slot];
      if (!dashStateValid(st)) return staleResult();
      var owned = ensureDashboardOwnership(st);
      if (!owned.ok) return owned;
      var parentState = hud._surface.state(st.token);
      if (parentState !== "ready" && parentState !== "active") {
        return uiFail("Busy", "dashboard surface is not paintable");
      }
      var spec = controller.spec;
      controller.paintTransactions[slot] = transaction;
      st.interactive = false;
      function current() {
        return controller.paintTransactions[slot] === transaction && controller.open[slot] === st &&
          controller.spec === spec && dashOwnedCurrent(st);
      }
      var prepared = prepareFocus(st, current);
      if (!prepared.ok) return prepared;
      if (prepared.value) {
        st.focusBinding = componentBinding(st.binding, st.focusRetained);
        return uiOk(undefined);
      }
      var candidate;
      try { candidate = dashCandidate(controller, slot, st); }
      catch (err) {
        if (!current()) return DASH_SUPERSEDED;
        return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid dashboard data"));
      }
      if (!current()) return DASH_SUPERSEDED;
      if (!candidate) {
        closeDashboard(controller, slot, false, st);
        return uiOk(undefined);
      }
      var error = null;
      function drive(fn) {
        var driveBound = resultBoundDriver(st.binding, fn, function () {
          return current() && focusPaintable(st);
        });
        return function () {
          if (error !== null || !current()) return;
          if (!focusPaintable(st)) { error = uiFail("PaintFailed", "focus changed during paint"); return; }
          var driveResult = driveBound.apply(null, arguments);
          if (!driveResult.ok) error = driveResult;
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
      if (!commitFocus(st)) return uiFail("PaintFailed", "focus activation failed");
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
      st.focusBinding = componentBinding(st.binding, st.focusRetained);
      st.interactive = true;
      delete st.opening;
      delete st.retryableFailure;
      return uiOk(undefined);
    }

    function tryOpenDashboardBound(controller, slot, opts, binding, generation, rollbackOnFailure) {
      var updateRecord = dashUpdateRecord(controller, slot, binding, generation);
      var updateAttempt = beginUpdate(updateRecord);
      var result = tryOpenDashboardBoundInner(controller, slot, opts, binding, generation,
        updateRecord, rollbackOnFailure);
      completeUpdate(updateRecord, updateAttempt, result);
      return result;
    }

    function tryOpenDashboardBoundInner(controller, slot, opts, binding, generation, updateRecord,
      rollbackOnFailure) {
      if (controller.released) return releasedResult("dashboard");
      if (generation !== controller.generation || !bindingValid(binding)) return staleResult();
      var attempt = {};
      controller.openAttempts[slot] = attempt;
      var validated = focusOptions(opts);
      if (!validated.ok) return validated;
      if (generation !== controller.generation || !bindingValid(binding)) return staleResult();
      var o = opts || {};
      var cursorWanted, tabId;
      try { cursorWanted = o.cursor !== false; tabId = o.tab || ""; }
      catch (err) { return uiFail("InvalidArgument", errorMessage(err, "invalid dashboard options")); }
      if (generation !== controller.generation || !bindingValid(binding)) return staleResult();
      if (controller.openAttempts[slot] !== attempt) return uiFail("PaintFailed", "dashboard open superseded");

      // An explicit controller may reopen its own live claim. Retire that exact token first; a
      // different controller remains protected by the host's explicit/legacy arbitration.
      if (controller.open[slot]) closeDashboard(controller, slot, false, controller.open[slot]);
      if (controller.openAttempts[slot] && controller.openAttempts[slot] !== attempt) {
        return uiFail("PaintFailed", "dashboard open superseded");
      }
      controller.openAttempts[slot] = attempt;
      var state = { controller: controller, binding: binding, slot: slot, lane: 0, token: null,
        root: "s2_dash", mode: controller.mode, live: false, opening: true };
      state.componentGeneration = generation;
      state.tabId = tabId;
      state.tabPage = 0;
      state.rowPage = 0;
      state.interactive = false;
      state.pendingRootOpts = { cursor: cursorWanted };
      state.focusEnabled = validated.value !== null;
      state.focusPriority = validated.value;
      state.root = "s2_dash";
      state.cursorWanted = cursorWanted;
      state.updateRecord = updateRecord;
      state.surfaceMap = dashboardSurfaceState;
      state.focusRetained = function () { return dashOwnedCurrent(state); };
      state.focusDiscard = function () {
        if (controller.open[slot] === state) delete controller.open[slot];
      };
      state.focusRepaint = function () { return paintDashboard(controller, slot); };
      controller.open[slot] = state;
      var result;
      try { result = paintDashboard(controller, slot); }
      catch (err) { result = uiFail("PaintFailed", errorMessage(err, "hudkit: dashboard paint failed")); }
      if (result === DASH_SUPERSEDED) {
        if (generation === controller.generation && bindingValid(binding) &&
            dashStateValid(controller.open[slot])) {
          return uiOk(makeDashView(controller, slot, binding, generation));
        }
        if (generation !== controller.generation || !bindingValid(binding)) return staleResult();
        return uiFail("PaintFailed", "dashboard open cancelled");
      }
      if (!result.ok) {
        delete state.opening;
        if ((rollbackOnFailure !== false || state.focusEnabled) && controller.open[slot] === state) {
          closeDashboard(controller, slot, false, state);
        } else if (controller.open[slot] !== state) releaseFocus(state);
        return result;
      }
      if (generation !== controller.generation || !bindingValid(binding)) return staleResult();
      return uiOk(makeDashView(controller, slot, binding, generation));
    }

    function openDashboardBound(controller, slot, opts, binding, generation) {
      var result = tryOpenDashboardBound(controller, slot, opts, binding, generation, false);
      if (!result.ok && result.error.code === "StaleClient") return staleOpen("dashboard");
      if (!result.ok && result.error.code === "Released") {
        throw new Error("hudkit: dashboard.open failed: released");
      }
      if (!result.ok && result.error.code === "InvalidArgument") throw new Error(result.error.message);
      if (!result.ok) return makeDashView(controller, slot, binding, generation);
      return result.value;
    }

    function makeDashView(controller, slot, binding, generation) {
      var updateRecord = dashUpdateRecord(controller, slot, binding, generation);
      function valid() {
        return !controller.released && generation === controller.generation && bindingValid(binding);
      }
      return {
        slot: slot,
        isValid: valid,
        open: function (opts) {
          return openDashboardBound(controller, slot, opts, binding, generation);
        },
        tryOpenResult: function (opts) {
          if (controller.released) return releasedResult("dashboard");
          if (!valid()) return staleResult();
          return tryOpenDashboardBound(controller, slot, opts, binding, generation);
        },
        close: function () { if (valid()) closeDashboard(controller, slot, false); },
        isOpen: function () {
          var st = controller.open[slot];
          return valid() && dashStateValid(st) && (dashOwnedCurrent(st) || !!st.retryableFailure);
        },
        setTab: function (tab) { if (valid()) controller.self.setTab(slot, tab); },
        refresh: function () { if (valid()) controller.self.refresh(slot); },
        invalidate: function () { if (valid()) controller.self.invalidate(slot); },
        lastUpdateResult: function () { return valid() ? updateRecord.result : null; },
        tryRefresh: function () {
          if (controller.released) return releasedResult("dashboard");
          if (!valid()) return staleResult();
          return controller.self.tryRefresh(slot);
        }
      };
    }

    function createDashboardController(spec, mode, disposable) {
      var controller = { spec: spec || {}, mode: mode, generation: 1, released: false,
        open: {}, paintTransactions: {}, openAttempts: {}, updateRecords: {}, self: null };
      var self = {
        open: function (slot, opts) {
          if (controller.released) throw new Error("hudkit: dashboard.open failed: released");
          return openDashboardBound(controller, slot, opts, currentBinding(slot), controller.generation);
        },
        tryOpenResult: function (slot, opts) {
          if (controller.released) return releasedResult("dashboard");
          return tryOpenDashboardBound(controller, slot, opts, captureBinding(slot), controller.generation);
        },
        close: function (slot) { if (!controller.released) closeDashboard(controller, slot, false); },
        isOpen: function (slot) {
          var st = controller.open[slot];
          return !controller.released && dashStateValid(st) &&
            (dashOwnedCurrent(st) || !!st.retryableFailure);
        },
        setTab: function (slot, tab) {
          var st = controller.open[slot];
          if (!dashStateValid(st) || !dashOwnedCurrent(st) && !st.retryableFailure) return;
          st.tabId = tab;
          st.rowPage = 0;
          paintDashboard(controller, slot);
        },
        refresh: function (slot) {
          if (controller.released) return;
          if (slot == null) {
            for (var key in controller.open) {
              var state = controller.open[key];
              if (dashStateValid(state) && (dashOwnedCurrent(state) || state.retryableFailure)) {
                paintDashboard(controller, Number(key));
              }
            }
          } else {
            var state = controller.open[slot];
            if (dashStateValid(state) && (dashOwnedCurrent(state) || state.retryableFailure)) {
              paintDashboard(controller, slot);
            }
          }
        },
        invalidate: function (slot) {
          if (controller.released) return;
          function invalidateOne(sl) {
            var st = controller.open[sl];
            if (!dashStateValid(st)) return;
            queueDirty(st, function () { return paintDashboard(controller, sl); }, function (expected) {
              return controller.open[sl] === expected &&
                (dashOwnedCurrent(expected) || !!expected.retryableFailure);
            }, dashboardInvalidationPaintable);
          }
          if (slot == null) {
            for (var key in controller.open) {
              if (Object.prototype.hasOwnProperty.call(controller.open, key)) invalidateOne(Number(key));
            }
          } else invalidateOne(slot);
        },
        tryRefresh: function (slot) {
          if (controller.released) return releasedResult("dashboard");
          if (slot == null) return uiFail("InvalidArgument", "needs a player slot");
          var binding = captureBinding(slot);
          if (!bindingValid(binding)) return staleResult();
          var st = controller.open[slot];
          if (!dashStateValid(st) || !dashOwnedCurrent(st) && !st.retryableFailure) {
            var updateRecord = dashUpdateRecord(controller, slot, binding, controller.generation);
            var unopenedAttempt = beginUpdate(updateRecord);
            var unopened = uiFail("InvalidArgument", "hudkit: dashboard is not open");
            completeUpdate(updateRecord, unopenedAttempt, unopened);
            return unopened;
          }
          var result;
          try { result = paintDashboard(controller, slot); }
          catch (err) { return uiFail("PaintFailed", errorMessage(err, "hudkit: dashboard paint failed")); }
          if (result === DASH_SUPERSEDED) {
            if (!bindingValid(binding)) return staleResult();
            return dashStateValid(controller.open[slot]) ? uiOk(undefined) :
              uiFail("PaintFailed", "dashboard refresh cancelled");
          }
          return result;
        },
        forSlot: function (slot) {
          return makeDashView(controller, slot, captureBinding(slot), controller.generation);
        }
      };
      if (disposable) self.dispose = function () {
        if (controller.released) return;
        var slots = [];
        for (var key in controller.open) {
          if (Object.prototype.hasOwnProperty.call(controller.open, key)) slots.push(Number(key));
        }
        // Fence public reentry before release/hide/capture effects. Cleanup below intentionally
        // operates on the snapshotted states even though their controller is now released.
        controller.released = true;
        controller.generation++;
        var index = dashboardControllers.indexOf(controller);
        if (index >= 0) dashboardControllers.splice(index, 1);
        for (var i = 0; i < slots.length; i++) closeDashboard(controller, slots[i], false);
        for (var recordKey in controller.updateRecords) {
          if (Object.prototype.hasOwnProperty.call(controller.updateRecords, recordKey)) {
            removeDirty(controller.updateRecords[recordKey]);
          }
        }
      };
      controller.self = self;
      dashboardControllers.push(controller);
      return controller;
    }

    function reconfigureLegacyDashboard(controller, spec) {
      var nextSpec = spec || {};
      var states = [];
      for (var key in controller.open) {
        if (!Object.prototype.hasOwnProperty.call(controller.open, key)) continue;
        var st = controller.open[key];
        if (!st) continue;
        states.push({ slot: Number(key), binding: st.binding, tab: st.tabId,
          cursor: st.cursorWanted, focusEnabled: st.focusEnabled, priority: st.focusPriority });
      }
      for (var recordKey in controller.updateRecords) {
        if (Object.prototype.hasOwnProperty.call(controller.updateRecords, recordKey)) {
          removeDirty(controller.updateRecords[recordKey]);
        }
      }
      for (var i = 0; i < states.length; i++) closeDashboard(controller, states[i].slot, false);
      controller.generation++;
      controller.updateRecords = {};
      controller.spec = nextSpec;
      var firstError = null;
      for (var si = 0; si < states.length && controller.spec === nextSpec; si++) {
        var prior = states[si];
        var options = { tab: prior.tab, cursor: prior.cursor };
        if (prior.focusEnabled) options.focus = { mode: "exclusive", priority: prior.priority };
        var result;
        try {
          result = tryOpenDashboardBound(controller, prior.slot, options, prior.binding,
            controller.generation, false);
        } catch (err) { if (!firstError) firstError = err; continue; }
        if (!result.ok && result.error.code === "InvalidArgument" && !firstError) {
          firstError = new Error(result.error.message);
        }
      }
      if (firstError) throw firstError;
      return controller.self;
    }

    function dashboard(spec) {
      if (!legacyDashboard) legacyDashboard = createDashboardController(spec, "legacy", false);
      else return reconfigureLegacyDashboard(legacyDashboard, spec);
      return legacyDashboard.self;
    }

    function tryOwnDashboard(spec) {
      try { return uiOk(createDashboardController(spec, "explicit", true).self); }
      catch (err) {
        return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid dashboard specification"));
      }
    }

    function activeDashboardState(slot) {
      var st = surfaceSlot(dashboardSurfaceState, slot)[0];
      return dashStateValid(st) && st.interactive && focusAllows(st) ? st : null;
    }

    hud.onClick("s2_dash_close", function (player) {
      var slot = slotOf(player), st = activeDashboardState(slot);
      if (st) closeDashboard(st.controller, slot, true, st);
    });
    for (var dti = 0; dti < DASH_TABS; dti++) {
      (function (tabIndex) {
        hud.onClick(DASH_TAB[tabIndex].id, function (player) {
          var slot = slotOf(player), st = activeDashboardState(slot);
          if (!st) return;
          var tab = st.paintedTabs && st.paintedTabs[tabIndex];
          if (!tab) return;
          st.tabId = tab.id;
          st.rowPage = 0;
          paintDashboard(st.controller, slot);
        });
      })(dti);
    }
    for (var dri = 0; dri < DASH_ROWS; dri++) {
      (function (rowIndex) {
        hud.onClick(DASH_ROW[rowIndex].id, function (player) {
          var slot = slotOf(player), st = activeDashboardState(slot);
          if (!st) return;
          var record = st.paintedRows && st.paintedRows[rowIndex];
          var onPick = st.paintedOnPick;
          var controller = st.controller;
          if (!record || record.row.disabled || typeof onPick !== "function") return;
          onPick(slot, st.paintedTabId, record.row, controller.self.forSlot(slot));
        });
      })(dri);
    }
    hud.onClick("s2_dash_prev", function (player) {
      var slot = slotOf(player), st = activeDashboardState(slot);
      if (!st) return;
      st.rowPage -= 1;
      paintDashboard(st.controller, slot);
    });
    hud.onClick("s2_dash_next", function (player) {
      var slot = slotOf(player), st = activeDashboardState(slot);
      if (!st) return;
      st.rowPage += 1;
      paintDashboard(st.controller, slot);
    });

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
      var shownBindings = {};
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
        var result = tryShowBadge(slot, data, binding);
        if (result.ok) shownBindings[slot] = binding;
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
            try {
              var result = tryShowBadge(slot, data, binding);
              if (result.ok) shownBindings[slot] = binding;
              return result;
            }
            catch (err) {
              return badgeReleased ? releasedResult("badge") :
                uiFail("PaintFailed", errorMessage(err, "hudkit: badge paint failed"));
            }
          },
          hide: function () {
            if (valid()) boundDriver(binding, hide, valid)(slot, slotIds.id);
            if (shownBindings[slot] === binding) delete shownBindings[slot];
          }
        };
      }
      selfBadge = {
        show: function (slot, data) {
          return showBadge(slot, data, currentBinding(slot));
        },
        hide: function (slot) {
          var binding = shownBindings[slot];
          if (!badgeReleased && bindingValid(binding)) boundDriver(binding, hide)(slot, slotIds.id);
          delete shownBindings[slot];
        },
        forSlot: function (slot) {
          return makeBadgeView(slot, captureBinding(slot));
        },
        release: function () {
          if (badgeReleased) return;
          badgeReleased = true;
          shownBindings = {};
          releaseSlot("badge", idx);
          var liveIndex = liveBadges.indexOf(selfBadge);
          if (liveIndex >= 0) liveBadges.splice(liveIndex, 1);
        }
      };
      liveBadges.push(selfBadge);
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
      var openAttempts = {};
      var paintTransactions = {};
      var modalSlotEpochs = {};
      var modalUpdateRecords = {};
      var SUPERSEDED = {};
      var self;

      function modalUpdateRecord(slot, binding, componentEpoch) {
        var record = modalUpdateRecords[slot];
        if (record && record.componentEpoch === componentEpoch && record.live()) return record;
        record = newUpdateRecord("modal", function () {
          return !released && componentEpoch === (modalSlotEpochs[slot] || 0) && bindingValid(binding);
        });
        record.componentEpoch = componentEpoch;
        modalUpdateRecords[slot] = record;
        return record;
      }

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
        if (request && request.selectIndex != null) {
          var selected = Math.max(0, Math.min(request.selectIndex, all.length > 0 ? all.length - 1 : 0));
          pageNumber = Math.floor(selected / pageSize);
          cursor = selected % pageSize;
        }
        if (request && request.pageDelta != null) {
          pageNumber = ((pageNumber + request.pageDelta) % pages + pages) % pages;
          cursor = 0;
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
        var st = candidate || open[slot], result, transaction = {};
        var updateAttempt = beginUpdate(st && st.updateRecord);
        function ownsFailure() { return st && (paintTransactions[slot] === transaction ||
          st.focusEnabled && !st.focusRetained()); }
        try { result = paintInner(slot, candidate, rootOpts, request, transaction); }
        catch (err) {
          completeUpdate(st && st.updateRecord, updateAttempt,
            uiFail("PaintFailed", errorMessage(err, "hudkit: modal paint failed")));
          if (ownsFailure()) releaseFocus(st);
          throw err;
        }
        if (result !== SUPERSEDED) completeUpdate(st && st.updateRecord, updateAttempt, result);
        if (ownsFailure() && (result === SUPERSEDED || !result.ok)) releaseFocus(st);
        return result;
      }
      function paintInner(slot, candidate, rootOpts, request, transaction) {
        if (released) return releasedResult("modal");
        var st = candidate || open[slot];
        if (!st) return uiFail("InvalidArgument", "hudkit: modal is not open");
        var expectedState = open[slot];
        if (expectedState) expectedState.interactive = false;
        st.interactive = false;
        paintTransactions[slot] = transaction;
        st.paintTransaction = transaction;
        st.paintExpectedState = expectedState;
        function current() {
          return !released && bindingValid(st.binding) &&
            st.componentEpoch === (modalSlotEpochs[slot] || 0) &&
            paintTransactions[slot] === transaction && open[slot] === expectedState;
        }
        if (!current()) return SUPERSEDED;
        if (st.focusEnabled && request) {
          if (request.selectIndex != null) st.focusRequest = { selectIndex: request.selectIndex };
          else if (request.pageDelta != null) {
            if (!st.focusRequest) st.focusRequest = {};
            st.focusRequest.pageDelta = (st.focusRequest.pageDelta || 0) + request.pageDelta;
          }
        }
        var prepared = prepareFocus(st, current);
        if (!prepared.ok) return prepared;
        if (prepared.value) {
          open[slot] = st;
          st.focusBinding = componentBinding(st.binding, st.focusRetained);
          return uiOk(undefined);
        }
        rootOpts = st.pendingRootOpts || rootOpts;
        if (st.focusEnabled) request = st.focusRequest;
        var snapshot;
        try { snapshot = modalCandidate(slot, st, request); }
        catch (err) {
          if (!current()) return SUPERSEDED;
          return uiFail("InvalidArgument", errorMessage(err, "hudkit: invalid modal data"));
        }
        if (!current()) return SUPERSEDED;
        var error = null;
        function drive(fn) {
          var driveBound = resultBoundDriver(st.binding, fn, function () { return current() && focusPaintable(st); });
          return function () {
            if (error !== null || !current()) return;
            if (!focusPaintable(st)) { error = uiFail("PaintFailed", "focus changed during paint"); return; }
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
        if (!commitFocus(st)) { releaseFocus(st); return uiFail("PaintFailed", "focus activation failed"); }
        delete st.pendingRootOpts; delete st.focusRequest;
        st.page = snapshot.pageNumber;
        st.cursor = snapshot.cursor;
        st.paintedRows = snapshot.paintedRows;
        st.paintedOffset = snapshot.paintedOffset;
        st.paintedPages = snapshot.pages;
        st.footerFns = snapshot.footerFns.slice();
        open[slot] = st;
        st.focusBinding = componentBinding(st.binding, st.focusRetained);
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
            if (!record || !focusAllows(st)) return;
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
            if (fn && focusAllows(open[slot])) fn(slot, self.forSlot(slot));
          });
        })(fi);
      }

      function makeModalView(slot, binding, componentEpoch) {
        var updateRecord = modalUpdateRecord(slot, binding, componentEpoch);
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
          invalidate: function () { if (valid()) self.invalidate(slot); },
          lastUpdateResult: function () { return valid() ? updateRecord.result : null; },
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
        var updateRecord = modalUpdateRecord(slot, binding, componentEpoch);
        var updateAttempt = beginUpdate(updateRecord);
        var result = tryOpenResultBoundInner(slot, opts, binding, componentEpoch, updateRecord);
        completeUpdate(updateRecord, updateAttempt, result);
        return result;
      }

      function tryOpenResultBoundInner(slot, opts, binding, componentEpoch, updateRecord) {
        if (released) return releasedResult("modal");
        if (!bindingValid(binding) || componentEpoch !== (modalSlotEpochs[slot] || 0)) return staleResult();
        // Getters in options may open/close synchronously, before a paint transaction exists.
        var attempt = {}; openAttempts[slot] = attempt;
        var validated = focusOptions(opts);
        if (!validated.ok) return validated;
        var cursorWanted;
        try { cursorWanted = !(opts && opts.cursor === false); }
        catch (err) { return uiFail("InvalidArgument", errorMessage(err, "invalid modal options")); }
        if (released) return releasedResult("modal");
        if (!bindingValid(binding) || componentEpoch !== (modalSlotEpochs[slot] || 0)) return staleResult();
        if (openAttempts[slot] !== attempt) return uiFail("PaintFailed", "modal open superseded");
        if (open[slot]) { cancelDirtyState(open[slot]); releaseFocus(open[slot]); }
        var candidate = { page: 0, cursor: 0, interactive: false, binding: binding,
          componentEpoch: componentEpoch, focusEnabled: validated.value !== null,
          focusPriority: validated.value, root: ids.root, cursorWanted: cursorWanted,
          updateRecord: updateRecord };
        candidate.focusRetained = function () { return !released && open[slot] === candidate &&
          bindingValid(binding) && componentEpoch === (modalSlotEpochs[slot] || 0); };
        candidate.focusDiscard = function () { if (open[slot] === candidate) delete open[slot]; };
        candidate.focusRepaint = function () { return paint(slot); };
        var result;
        try {
          result = paint(slot, candidate, { cursor: cursorWanted });
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
            try { if (!candidate.focusEnabled) boundDriver(binding, hide, function () {
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
          var st = open[slot];
          if (st && st.focusEnabled) {
            st.cursorWanted = !!on;
            if (!focusAllows(st)) return;
            var result = resultBoundDriver(st.binding, function () {
              return structuredCall("cursorForPanel", hud._cursorForPanel, [slot, ids.root, !!on]);
            }, st.focusRetained)();
            if (!result.ok) releaseFocus(st);
            return;
          }
          return hud._cursorForPanel(slot, ids.root, !!on);
        },
        close: function (slot) {
          if (released) return;
          var st = open[slot];
          cancelDirtyState(st);
          delete openAttempts[slot];
          delete paintTransactions[slot];
          delete open[slot];
          if (releaseFocus(st)) return;
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
        invalidate: function (slot) {
          function invalidateOne(sl) {
            var st = open[sl];
            if (!st || !self.isOpen(sl)) return;
            queueDirty(st, function () { return paint(sl); }, function (expected) {
              return open[sl] === expected && self.isOpen(sl);
            }, invalidationPaintable);
          }
          if (slot == null) {
            for (var k in open) if (Object.prototype.hasOwnProperty.call(open, k)) invalidateOne(Number(k));
          } else invalidateOne(slot);
        },
        tryRefresh: function (slot) {
          if (released) return releasedResult("modal");
          if (slot == null) return uiFail("InvalidArgument", "needs a player slot");
          var binding = captureBinding(slot);
          if (!bindingValid(binding)) return staleResult();
          if (!self.isOpen(slot)) {
            var updateRecord = modalUpdateRecord(slot, binding, modalSlotEpochs[slot] || 0);
            var unopenedAttempt = beginUpdate(updateRecord);
            var unopened = uiFail("InvalidArgument", "hudkit: modal is not open");
            completeUpdate(updateRecord, unopenedAttempt, unopened);
            return unopened;
          }
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
          delete openAttempts[slot];
          cancelDirtyState(open[slot]);
          releaseFocus(open[slot]);
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
          for (var ur in modalUpdateRecords) {
            if (Object.prototype.hasOwnProperty.call(modalUpdateRecords, ur)) removeDirty(modalUpdateRecords[ur]);
          }
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
      function clearLegacy(map, key, roots, profile) {
        if (!hud._surface) return;
        var result = hud._surface.clearLegacy(binding, key, roots, profile);
        var records = surfaceSlot(map, slot).slice();
        for (var i = 0; i < records.length; i++) {
          var record = records[i];
          if (record && record.mode === "legacy" &&
              (result.ok || hud._surface.state(record.token) === "invalid")) {
            retireSurface(map, record, false);
          }
        }
        if (!result.ok) log("hideAll " + key + " failed: " + result.error.code + ": " + result.error.message);
      }
      clearLegacy(toastState, OWNED_TOAST, TOAST_ROOTS, "visual");
      clearLegacy(calloutState, OWNED_CALLOUT, ["s2_callout"], "visual");
      clearLegacy(bannerState, OWNED_BANNER, ["s2_banner"], "visual");
      if (hud._surface) {
        var motdClear = hud._surface.clearLegacy(binding, OWNED_MOTD, ["s2_motd"], "interactive");
        var motdState = motdOpen[slot];
        if (motdState && motdState.mode === "legacy" &&
            (motdClear.ok || hud._surface.state(motdState.token) === "invalid")) {
          delete motdOpen[slot]; releaseFocus(motdState);
        }
        if (!motdClear.ok) log("hideAll " + OWNED_MOTD + " failed: " +
          motdClear.error.code + ": " + motdClear.error.message);
      }
      if (hud._surface) {
        var dashClear = hud._surface.clearLegacy(binding, OWNED_DASHBOARD, ["s2_dash"], "interactive");
        var dashState = surfaceSlot(dashboardSurfaceState, slot)[0];
        if (dashState && dashState.mode === "legacy" &&
            (dashClear.ok || hud._surface.state(dashState.token) === "invalid")) {
          abandonReplacedDashboard(dashState);
        }
        if (!dashClear.ok) log("hideAll " + OWNED_DASHBOARD + " failed: " +
          dashClear.error.code + ": " + dashClear.error.message);
      }
      for (var m2 = 0; m2 < liveModals.length; m2++) liveModals[m2].close(slot);
      for (var b2 = 0; b2 < liveBadges.length; b2++) liveBadges[b2].hide(slot);
    }

    function forgetSlot(slot) { hud.forget(slot); }

    function makeKitPlayer(slot, binding) {
      function valid() { return bindingValid(binding); }
      return {
        slot: slot,
        isValid: valid,
        toast: function (spec) { return valid() ? toast(slot, spec, binding) : "stale client"; },
        tryOwnToast: function (spec) { return valid() ? tryToast(slot, spec, binding, "explicit") : staleResult(); },
        callout: function (spec) { return valid() ? callout(slot, spec, binding) : "stale client"; },
        tryOwnCallout: function (spec) { return valid() ? tryCallout(slot, spec, binding, "explicit") : staleResult(); },
        banner: function (spec) { return valid() ? banner(slot, spec, binding) : "stale client"; },
        tryOwnBanner: function (spec) { return valid() ? tryBanner(slot, spec, binding, "explicit") : staleResult(); },
        motd: function (spec) { return valid() ? motd(slot, spec, binding) : invalidMotd(slot); },
        tryOwnMotd: function (spec) { return valid() ? tryMotd(slot, spec, binding, "explicit") : staleResult(); },
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
      tryOwnDashboard: tryOwnDashboard,
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
      // Test/benchmark seam: this is the actual dirty-record queue, not an inferred counter.
      _pendingInvalidationCount: function () { return dirtyUpdates.length; },
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
      tryOwnDashboard: kitFn("tryOwnDashboard"),
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
