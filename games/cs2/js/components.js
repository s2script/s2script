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

  // Opt-in diagnostics (`globalThis.__s2_ui_debug = true`, e.g. from a dev plugin or the
  // hot-reload console). Read per use, not latched, so it can be flipped on a live server while
  // chasing a misbehaving sheet. Today it arms exactly one check: the per-slot footer-set
  // divergence warning in modal() below.
  function uiDebug() { return globalThis.__s2_ui_debug === true; }

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
    var motdOnClose = {};
    var dashSpec = null;
    var dashOpen = {};
    var origForget = hud.forget;
    hud.forget = function (slot) {
      for (var li = 0; li < liveModals.length; li++) liveModals[li].forget(slot);
      closeMotd(slot, false);
      closeDash(slot, false);
      origForget(slot);
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

    function toast(slot, opts) {
      var o = opts || {};
      var i = toastNext % TOASTS;
      toastNext++;
      var t = TOAST[i];
      // A generation stamp per slot: if this toast is replaced before its hold expires, the older
      // timer must not yank the newer one off screen.
      toastGen[i]++;
      var gen = toastGen[i];

      setText(slot, t.title, o.title || "");
      setText(slot, t.msg, o.message || "");
      var want = TOAST_VARIANT[o.variant] || null;
      for (var vk in TOAST_VARIANT) {
        if (Object.prototype.hasOwnProperty.call(TOAST_VARIANT, vk)) {
          setClass(slot, t.id, TOAST_VARIANT[vk], TOAST_VARIANT[vk] === want);
        }
      }
      reveal(slot, t.id, FADE.toast);

      var hold = o.holdSeconds == null ? 6 : o.holdSeconds;
      if (hold <= 0) return null;
      afterSeconds(hold, function () {
        if (toastGen[i] !== gen) return;
        setClass(slot, t.id, FADE.toast, true);
        afterSeconds(0.3, function () {
          if (toastGen[i] !== gen) return;
          hide(slot, t.id);
        });
      });
      return null;
    }

    // ── callout (hint: bottom-center, no cursor) ─────────────────────────────────────────────

    function callout(slot, opts) {
      var o = opts || {};
      calloutGen[slot] = (calloutGen[slot] || 0) + 1;
      var gen = calloutGen[slot];
      var title = o.title || "";
      var message = o.message || "";
      setText(slot, "s2_callout_title", title);
      setText(slot, "s2_callout_msg", message);
      if (!title) hide(slot, "s2_callout_title"); else show(slot, "s2_callout_title");
      if (!message) hide(slot, "s2_callout_msg"); else show(slot, "s2_callout_msg");
      var want = CALLOUT_VARIANT[o.variant] || null;
      for (var vk in CALLOUT_VARIANT) {
        if (Object.prototype.hasOwnProperty.call(CALLOUT_VARIANT, vk)) {
          setClass(slot, "s2_callout", CALLOUT_VARIANT[vk], CALLOUT_VARIANT[vk] === want);
        }
      }
      reveal(slot, "s2_callout", FADE.callout);
      var hold = o.holdSeconds == null ? 4 : o.holdSeconds;
      if (hold <= 0) return null;
      afterSeconds(hold, function () {
        if (calloutGen[slot] !== gen) return;
        setClass(slot, "s2_callout", FADE.callout, true);
        afterSeconds(0.25, function () {
          if (calloutGen[slot] !== gen) return;
          hide(slot, "s2_callout");
        });
      });
      return null;
    }

    // ── banner (center-top, one at a time, no cursor) ────────────────────────────────────────

    function banner(slot, opts) {
      var o = opts || {};
      bannerGen[slot] = (bannerGen[slot] || 0) + 1;
      var gen = bannerGen[slot];
      setText(slot, "s2_banner_text", o.text == null ? "" : String(o.text));
      reveal(slot, "s2_banner", FADE.banner);
      var hold = o.holdSeconds == null ? 5 : o.holdSeconds;
      if (hold <= 0) return null;
      afterSeconds(hold, function () {
        if (bannerGen[slot] !== gen) return;
        setClass(slot, "s2_banner", FADE.banner, true);
        afterSeconds(0.25, function () {
          if (bannerGen[slot] !== gen) return;
          hide(slot, "s2_banner");
        });
      });
      return null;
    }

    // ── MOTD (scrim + OK). Not a third center sheet. ─────────────────────────────────────────

    function closeMotd(slot, fromClick) {
      if (!motdOpen[slot]) return;
      delete motdOpen[slot];
      hide(slot, "s2_motd");
      hud.cursor(slot, false);
      var fn = motdOnClose[slot];
      delete motdOnClose[slot];
      if (fromClick && typeof fn === "function") fn(slot);
    }

    hud.onClick("s2_motd_ok", function (player) {
      closeMotd(slotOf(player), true);
    });

    function motd(slot, opts) {
      var o = opts || {};
      motdOpen[slot] = true;
      motdOnClose[slot] = o.onClose || null;
      setText(slot, "s2_motd_title", o.title == null ? "" : String(o.title));
      var sub = o.subtitle == null ? "" : String(o.subtitle);
      setText(slot, "s2_motd_sub", sub);
      if (!sub) hide(slot, "s2_motd_sub"); else show(slot, "s2_motd_sub");
      var sections = Array.isArray(o.sections) ? o.sections : [];
      for (var i = 0; i < MOTD_SECTIONS; i++) {
        var sec = sections[i] || {};
        var heading = sec.heading == null ? "" : String(sec.heading);
        var body = sec.body == null ? "" : String(sec.body);
        setText(slot, MOTD_SECTION[i].h, heading);
        setText(slot, MOTD_SECTION[i].p, body);
        if (!heading) hide(slot, MOTD_SECTION[i].h); else show(slot, MOTD_SECTION[i].h);
        if (!body) hide(slot, MOTD_SECTION[i].p); else show(slot, MOTD_SECTION[i].p);
      }
      var note = o.note == null ? "" : String(o.note);
      setText(slot, "s2_motd_note", note);
      if (!note) hide(slot, "s2_motd_note"); else show(slot, "s2_motd_note");
      setText(slot, "s2_motd_ok_t", o.ok == null ? "OK" : String(o.ok));
      show(slot, "s2_motd", { cursor: o.cursor !== false });
      return {
        slot: slot,
        close: function () { closeMotd(slot, false); }
      };
    }

    // ── dashboard (tabbed TopMenu hub). One spec, one root. Not a modal pool slot. ───────────

    function dashTabs(slot) {
      if (!dashSpec) return [];
      var got = typeof dashSpec.tabs === "function" ? dashSpec.tabs(slot) : (dashSpec.tabs || []);
      return got || [];
    }

    function closeDash(slot, fromClick) {
      if (!dashOpen[slot]) return;
      delete dashOpen[slot];
      hide(slot, "s2_dash");
      hud.cursor(slot, false);
      if (fromClick && dashSpec && typeof dashSpec.onClose === "function") dashSpec.onClose(slot);
    }

    function paintDash(slot) {
      var st = dashOpen[slot];
      if (!st || !dashSpec) return;
      var tabs = dashTabs(slot);
      if (tabs.length === 0) { closeDash(slot, false); return; }
      var found = false;
      for (var ti = 0; ti < tabs.length; ti++) {
        if (tabs[ti] && tabs[ti].id === st.tabId) { found = true; break; }
      }
      if (!found) { st.tabId = tabs[0].id; st.rowPage = 0; }

      setText(slot, "s2_dash_title", typeof dashSpec.title === "function" ? dashSpec.title(slot) : (dashSpec.title || ""));
      var sub = typeof dashSpec.subtitle === "function" ? dashSpec.subtitle(slot, st.tabId) : dashSpec.subtitle;
      setText(slot, "s2_dash_sub", sub == null ? "" : String(sub));
      setText(slot, "s2_dash_close_t", dashSpec.closeText || "Close");

      var tabPages = Math.max(1, Math.ceil(tabs.length / DASH_TABS));
      if (st.tabPage >= tabPages) st.tabPage = tabPages - 1;
      if (st.tabPage < 0) st.tabPage = 0;
      var tabSlice = tabs.slice(st.tabPage * DASH_TABS, st.tabPage * DASH_TABS + DASH_TABS);
      for (var t = 0; t < DASH_TABS; t++) {
        var tab = tabSlice[t];
        if (!tab) { hide(slot, DASH_TAB[t].id); continue; }
        show(slot, DASH_TAB[t].id);
        setText(slot, DASH_TAB[t].text, tab.title || tab.id);
        setClass(slot, DASH_TAB[t].id, "s2-tab-active", tab.id === st.tabId);
      }

      var rows = typeof dashSpec.rows === "function" ? (dashSpec.rows(slot, st.tabId) || []) : [];
      var rowPages = Math.max(1, Math.ceil(rows.length / DASH_ROWS));
      if (st.rowPage >= rowPages) st.rowPage = rowPages - 1;
      if (st.rowPage < 0) st.rowPage = 0;
      var rowSlice = rows.slice(st.rowPage * DASH_ROWS, st.rowPage * DASH_ROWS + DASH_ROWS);
      for (var r = 0; r < DASH_ROWS; r++) {
        var row = rowSlice[r];
        if (!row) { hide(slot, DASH_ROW[r].id); continue; }
        show(slot, DASH_ROW[r].id);
        setText(slot, DASH_ROW[r].a, row.a == null ? "" : String(row.a));
        setText(slot, DASH_ROW[r].b, row.b == null ? "" : String(row.b));
        setClass(slot, DASH_ROW[r].id, "s2-toggle-disabled", !!row.disabled);
        setClass(slot, DASH_ROW[r].id, "s2-toggle-active", false);
      }

      var tabTitle = "";
      for (var tt = 0; tt < tabs.length; tt++) {
        if (tabs[tt].id === st.tabId) { tabTitle = tabs[tt].title || tabs[tt].id; break; }
      }
      var status = tabTitle;
      if (rowPages > 1) status += "  ·  " + (st.rowPage + 1) + "/" + rowPages;
      setText(slot, "s2_dash_status", status);
      setText(slot, "s2_dash_prev_t", "‹ Prev");
      setText(slot, "s2_dash_next_t", "Next ›");
      if (rowPages > 1) { show(slot, "s2_dash_prev"); show(slot, "s2_dash_next"); }
      else { hide(slot, "s2_dash_prev"); hide(slot, "s2_dash_next"); }
    }

    hud.onClick("s2_dash_close", function (player) {
      closeDash(slotOf(player), true);
    });
    for (var dti = 0; dti < DASH_TABS; dti++) {
      (function (tabIndex) {
        hud.onClick(DASH_TAB[tabIndex].id, function (player) {
          var slot = slotOf(player);
          var st = dashOpen[slot];
          if (!st) return;
          var tabs = dashTabs(slot);
          var tab = tabs[st.tabPage * DASH_TABS + tabIndex];
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
          if (!st || !dashSpec) return;
          var rows = typeof dashSpec.rows === "function" ? (dashSpec.rows(slot, st.tabId) || []) : [];
          var row = rows[st.rowPage * DASH_ROWS + rowIndex];
          if (!row || row.disabled) return;
          if (typeof dashSpec.onPick === "function") {
            dashSpec.onPick(slot, st.tabId, row, dashSelf.forSlot(slot));
          }
        });
      })(dri);
    }
    hud.onClick("s2_dash_prev", function (player) {
      var slot = slotOf(player);
      var st = dashOpen[slot];
      if (!st) return;
      st.rowPage -= 1;
      paintDash(slot);
    });
    hud.onClick("s2_dash_next", function (player) {
      var slot = slotOf(player);
      var st = dashOpen[slot];
      if (!st) return;
      st.rowPage += 1;
      paintDash(slot);
    });

    var dashSelf;
    function dashboard(spec) {
      dashSpec = spec || {};
      if (dashSelf) {
        for (var k in dashOpen) { if (dashOpen[k]) paintDash(Number(k)); }
        return dashSelf;
      }
      dashSelf = {
        open: function (slot, opts) {
          var o = opts || {};
          var tabs = dashTabs(slot);
          var tabId = o.tab || (tabs[0] && tabs[0].id) || "";
          dashOpen[slot] = { tabId: tabId, tabPage: 0, rowPage: 0 };
          paintDash(slot);
          setClass(slot, "s2_dash", FADE.dash, false);
          show(slot, "s2_dash", { cursor: o.cursor !== false });
          return dashSelf.forSlot(slot);
        },
        close: function (slot) { closeDash(slot, false); },
        isOpen: function (slot) { return !!dashOpen[slot]; },
        setTab: function (slot, tabId) {
          var st = dashOpen[slot];
          if (!st) return;
          st.tabId = tabId;
          st.rowPage = 0;
          paintDash(slot);
        },
        refresh: function (slot) {
          if (slot == null) { for (var k in dashOpen) { if (dashOpen[k]) paintDash(Number(k)); } }
          else if (dashOpen[slot]) paintDash(slot);
        },
        forSlot: function (slot) {
          return {
            slot: slot,
            open: function (opts) { return dashSelf.open(slot, opts); },
            close: function () { dashSelf.close(slot); },
            isOpen: function () { return dashSelf.isOpen(slot); },
            setTab: function (tabId) { dashSelf.setTab(slot, tabId); },
            refresh: function () { dashSelf.refresh(slot); }
          };
        }
      };
      return dashSelf;
    }

    // ── badges (persistent corner HUD) ────────────────────────────────────────────────────────

    function badge(spec) {
      var s = spec || {};
      var idx = claim("badge", BADGES, ownerTag);
      if (idx < 0) { log("badge pool exhausted (" + BADGES + " in use) — request ignored"); return null; }
      var slotIds = BADGE[idx];
      var cornerCls = CORNER[s.corner] || CORNER.tr;
      var accentCls = BADGE_ACCENT[s.accent] || null;
      var selfBadge;
      selfBadge = {
        show: function (slot, data) {
          var dd = data || {};
          setText(slot, slotIds.title, dd.title || s.title || "");
          setText(slot, slotIds.text, dd.text || "");
          for (var k in CORNER) {
            if (Object.prototype.hasOwnProperty.call(CORNER, k)) {
              setClass(slot, slotIds.id, CORNER[k], CORNER[k] === cornerCls);
            }
          }
          for (var ak in BADGE_ACCENT) {
            if (Object.prototype.hasOwnProperty.call(BADGE_ACCENT, ak)) {
              setClass(slot, slotIds.id, BADGE_ACCENT[ak], BADGE_ACCENT[ak] === accentCls);
            }
          }
          reveal(slot, slotIds.id, FADE.badge);
          return selfBadge.forSlot(slot);
        },
        hide: function (slot) { hide(slot, slotIds.id); },
        forSlot: function (slot) {
          return {
            slot: slot,
            show: function (data) { return selfBadge.show(slot, data); },
            hide: function () { selfBadge.hide(slot); }
          };
        },
        release: function () { releaseSlot("badge", idx); }
      };
      return selfBadge;
    }

    // ── modals (title + paged list + detail + footer buttons) ─────────────────────────────────

    function modal(spec) {
      var s = spec || {};
      var idx = claim("modal", MODALS, ownerTag);
      if (idx < 0) { log("modal pool exhausted (" + MODALS + " in use) — request ignored"); return null; }
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
      var open = {};   // slot -> { page, cursor }
      var self;

      function rowsFor(slot) {
        var got = typeof s.rows === "function" ? s.rows(slot) : (s.rows || []);
        return got || [];
      }

      function pageCount(slot) {
        return Math.max(1, Math.ceil(rowsFor(slot).length / pageSize));
      }

      // Debug-mode divergence detector for the constraint documented on `footerFns` below: the
      // caller-button handlers the previous paint resolved, and for which slot. Only the CALLER's
      // buttons are compared — the pager closures footerPlan appends are minted fresh every call
      // (identity always differs) and dispatch on the CLICK-time slot, so they are safe to share
      // and would only drown the signal. Warns once per claimed modal: the condition re-fires on
      // every repaint, and the first line already names the fix.
      var footerDebugPrev = null;
      var footerDebugWarned = false;
      function debugCheckFooters(slot, mine) {
        if (footerDebugWarned || !uiDebug()) { footerDebugPrev = null; return; }
        var prev = footerDebugPrev;
        footerDebugPrev = { slot: slot, fns: [] };
        for (var i = 0; i < mine.length; i++) footerDebugPrev.fns.push(mine[i].onClick);
        // Only a LIVE divergence is the hazard: if the earlier slot has closed, its clicks can no
        // longer reach the shared table, so two sequential single-viewer uses stay quiet.
        if (!prev || prev.slot === slot || !open[prev.slot]) return;
        var differs = prev.fns.length !== footerDebugPrev.fns.length;
        for (var d = 0; !differs && d < prev.fns.length; d++) {
          if (prev.fns[d] !== footerDebugPrev.fns[d]) differs = true;
        }
        if (!differs) return;
        footerDebugWarned = true;
        log("modal s2_m" + idx + ": buttons(slot) returned a DIFFERENT set for slot " + slot +
            " than for slot " + prev.slot + " (" + footerDebugPrev.fns.length + " vs " +
            prev.fns.length + " handler(s), or differing identities). The footer handler table " +
            "is one array per MODAL, rebuilt on every paint — the last paint's handlers dispatch " +
            "for EVERY player with the sheet open, so slot " + prev.slot + "'s clicks would now " +
            "run slot " + slot + "'s handlers. Return the same handler set for every slot (vary " +
            "text/variant/rows instead), or claim a separate modal per button set.");
      }

      // Caller buttons first; prev/next take the trailing footer slots, and only when the data
      // actually needs paging — a one-page list shows no pager.
      function footerPlan(slot) {
        var plan = [];
        var mine = typeof s.buttons === "function" ? (s.buttons(slot) || []) : (s.buttons || []);
        if (typeof s.buttons === "function") debugCheckFooters(slot, mine);
        for (var i = 0; i < mine.length && plan.length < FOOTERS; i++) {
          plan.push({ text: mine[i].text, variant: mine[i].variant || "ghost", fn: mine[i].onClick });
        }
        if (pageCount(slot) > 1 && plan.length + 2 <= FOOTERS) {
          plan.push({ text: "‹ Prev", variant: "ghost", fn: function (sl) { self.page(sl, -1); } });
          plan.push({ text: "Next ›", variant: "ghost", fn: function (sl) { self.page(sl, 1); } });
        }
        return plan;
      }

      // ONE ARRAY PER MODAL, rebuilt on every paint — deliberately NOT per-slot. Button TEXT and
      // visibility go through per-player natives, but a click arrives as (panel id, player) and
      // must resolve to a handler somewhere, and this table is that somewhere. The consequence is
      // a real contract: a `buttons(slot)` function may vary text and variant per slot, but it
      // must return the SAME handlers in the same order for every slot, because the LAST paint's
      // handlers dispatch for every player with the sheet open. This cost a live incident (a
      // player's shop footer dispatching an admin's Ban handler); debugCheckFooters above is the
      // tripwire, and the ModalSpec.buttons doc in packages/cs2/ui.d.ts is the author-facing
      // statement of the rule.
      var footerFns = [];

      function paint(slot) {
        if (released) return;
        var st = open[slot];
        if (!st) return;
        var all = rowsFor(slot);
        var pages = Math.max(1, Math.ceil(all.length / pageSize));
        if (st.page >= pages) st.page = pages - 1;
        var page = all.slice(st.page * pageSize, st.page * pageSize + pageSize);

        setText(slot, ids.title, typeof s.title === "function" ? s.title(slot) : (s.title || ""));
        var sub = typeof s.subtitle === "function" ? s.subtitle(slot) : s.subtitle;
        setText(slot, ids.sub, sub == null ? (pages > 1 ? (st.page + 1) + "/" + pages : "") : sub);

        // A sheet with no rows is a confirm dialog, not an empty list — hide the container rather
        // than leaving eight blank rows on screen.
        if (page.length === 0) hide(slot, ids.list); else show(slot, ids.list);
        for (var i = 0; i < ROWS; i++) {
          var row = page[i];
          if (!row) { hide(slot, ids.rows[i].id); continue; }
          show(slot, ids.rows[i].id);
          setText(slot, ids.rows[i].a, row.a);
          setText(slot, ids.rows[i].b, row.b);
          setText(slot, ids.rows[i].c, row.c);
          setClass(slot, ids.rows[i].id, CLS.selected, i === st.cursor);
          // Cosmetic only. onPick still fires for a disabled row so the caller can explain why —
          // a row that silently does nothing reads as a broken menu.
          setClass(slot, ids.rows[i].id, CLS.disabled, !!row.disabled);
          // Tone is orthogonal to selected/disabled: a row can be the cursor, unaffordable AND
          // bad at once. Every tone is written each paint (like the footer variants) because
          // `setClass` holds no cache — the untaken ones must be cleared or a row keeps the tone
          // of whatever occupied that slot on the previous page.
          var wantTone = LI_TONE[row.tone];
          for (var tk in LI_TONE) {
            if (Object.prototype.hasOwnProperty.call(LI_TONE, tk)) {
              setClass(slot, ids.rows[i].id, LI_TONE[tk], LI_TONE[tk] === wantTone);
            }
          }
        }

        // ABSOLUTE index, like `onPick` and `cursor()`. Callers index their own full list with
        // it; handing over a page-relative one described the wrong row on every page but the
        // first, while the `row` argument beside it was correct — so it looked right until a
        // list got long enough to page.
        var absCursor = st.page * pageSize + st.cursor;
        var det = typeof s.detail === "function" ? (s.detail(slot, page[st.cursor], absCursor) || []) : [];
        if (det.length === 0) hide(slot, ids.detailBox); else show(slot, ids.detailBox);
        for (var d = 0; d < DETAIL; d++) setText(slot, ids.detail[d], det[d] == null ? "" : det[d]);

        footerFns = [];
        var plan = footerPlan(slot);
        for (var f = 0; f < FOOTERS; f++) {
          if (!plan[f]) { hide(slot, ids.footers[f].id); footerFns.push(null); continue; }
          show(slot, ids.footers[f].id);
          setText(slot, ids.footers[f].text, plan[f].text);
          var wantBtn = BTN_VARIANT[plan[f].variant] || BTN_VARIANT.ghost;
          for (var bk in BTN_VARIANT) {
            if (Object.prototype.hasOwnProperty.call(BTN_VARIANT, bk)) {
              setClass(slot, ids.footers[f].id, BTN_VARIANT[bk], BTN_VARIANT[bk] === wantBtn);
            }
          }
          footerFns.push(plan[f].fn || null);
        }
      }

      for (var ri = 0; ri < ROWS; ri++) {
        (function (rowIndex) {
          onClick(ids.rows[rowIndex].id, function (player) {
            var slot = slotOf(player);
            var st = open[slot];
            if (!st) return;
            st.cursor = rowIndex;
            var all = rowsFor(slot);
            var row = all[st.page * pageSize + rowIndex];
            if (row && s.onPick) s.onPick(slot, st.page * pageSize + rowIndex, row, self.forSlot(slot));
            paint(slot);
          });
        })(ri);
      }
      for (var fi = 0; fi < FOOTERS; fi++) {
        (function (fIndex) {
          onClick(ids.footers[fIndex].id, function (player) {
            var slot = slotOf(player);
            if (!open[slot]) return;
            var fn = footerFns[fIndex];
            if (fn) fn(slot, self.forSlot(slot));
          });
        })(fi);
      }

      self = {
        open: function (slot, opts) {
          if (released) throw new Error("hudkit: modal has been released");
          open[slot] = { page: 0, cursor: 0 };
          for (var wk in SHEET_WIDTH) {
            if (Object.prototype.hasOwnProperty.call(SHEET_WIDTH, wk) && SHEET_WIDTH[wk] !== "") {
              setClass(slot, ids.root, SHEET_WIDTH[wk], SHEET_WIDTH[wk] === widthCls);
            }
          }
          paint(slot);                                        // fill first…
          setClass(slot, ids.root, FADE.sheet, false);         // …never leave it transparent…
          show(slot, ids.root, { cursor: !(opts && opts.cursor === false) });
          return self.forSlot(slot);
        },
        setCursor: function (slot, on) {
          if (released) return;
          hud.cursor(slot, !!on);
        },
        close: function (slot) {
          if (released) return;
          delete open[slot];
          hide(slot, ids.root);
          hud.cursor(slot, false);
        },
        isOpen: function (slot) { return !!open[slot]; },
        refresh: function (slot) {
          if (slot == null) { for (var k in open) { if (open[k]) paint(Number(k)); } }
          else if (open[slot]) paint(slot);
        },
        page: function (slot, delta) {
          var st = open[slot];
          if (!st) return;
          var pages = pageCount(slot);
          st.page = ((st.page + delta) % pages + pages) % pages;
          st.cursor = 0;
          paint(slot);
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
          var st = open[slot];
          if (!st) return;
          var total = rowsFor(slot).length;
          var i = Math.max(0, Math.min(index, total > 0 ? total - 1 : 0));
          st.page = Math.floor(i / pageSize);
          st.cursor = i % pageSize;
          paint(slot);
        },
        /** ABSOLUTE index of the highlighted row — the same space `onPick` reports in. */
        cursor: function (slot) {
          var st = open[slot];
          if (!st) return -1;
          return st.page * pageSize + st.cursor;
        },
        forget: function (slot) { delete open[slot]; },
        forSlot: function (slot) {
          return {
            slot: slot,
            open: function (opts) { return self.open(slot, opts); },
            close: function () { self.close(slot); },
            isOpen: function () { return self.isOpen(slot); },
            refresh: function () { self.refresh(slot); },
            page: function (delta) { self.page(slot, delta); },
            select: function (index) { self.select(slot, index); },
            cursor: function () { return self.cursor(slot); },
            forget: function () { self.forget(slot); }
          };
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

    function hideAll(slot) {
      calloutGen[slot] = (calloutGen[slot] || 0) + 1;
      bannerGen[slot] = (bannerGen[slot] || 0) + 1;
      hide(slot, "s2_callout");
      hide(slot, "s2_banner");
      closeMotd(slot, false);
      closeDash(slot, false);
      for (var m2 = 0; m2 < MODALS; m2++) hide(slot, MODAL[m2].root);
      for (var t2 = 0; t2 < TOASTS; t2++) hide(slot, TOAST[t2].id);
      for (var b2 = 0; b2 < BADGES; b2++) hide(slot, BADGE[b2].id);
      hud.cursor(slot, false);
    }

    function forgetSlot(slot) { hud.forget(slot); }

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
      dashboard: dashboard,
      badge: badge,
      toast: toast,
      callout: callout,
      banner: banner,
      motd: motd,
      hideAll: hideAll,
      forget: forgetSlot,
      forSlot: function (slot) {
        return {
          slot: slot,
          toast: function (spec) { return toast(slot, spec); },
          callout: function (spec) { return callout(slot, spec); },
          banner: function (spec) { return banner(slot, spec); },
          motd: function (spec) { return motd(slot, spec); },
          hideAll: function () { hideAll(slot); },
          forget: function () { forgetSlot(slot); }
        };
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
    function hostKit() {
      if (sharedKit) return sharedKit;
      if (!liveBase) return null;   // no load ctx yet — refuse rather than mint a dead-context kit
      return defaultKit(liveBase);
    }
    function kitFn(name) {
      return function (a, b) {
        var kit = hostKit();
        if (!kit) {
          // Loud on purpose: the old stand-in path returned an object that painted and silently
          // never delivered a click, which cost live-server debugging rounds. A null with a
          // reason is strictly better than a component that half-works.
          log("hudkit." + name + "() before this plugin's context exists — returning null " +
              "(call it from OnPluginStart or later, not module top-level)");
          return null;
        }
        return kit[name](a, b);
      };
    }
    globalThis.__s2pkg_cs2.hudkit = {
      whenLive: whenLive,
      modal: kitFn("modal"),
      dashboard: kitFn("dashboard"),
      badge: kitFn("badge"),
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
      get: function () { var kit = hostKit(); return kit && kit.layout; }
    });
    Object.defineProperty(globalThis.__s2pkg_cs2.hudkit, "hud", {
      get: function () { var kit = hostKit(); return kit && kit.hud; }
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
