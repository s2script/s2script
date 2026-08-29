// @s2script/cs2 — ctx.ui.components(): a generic Panorama component library.
//
// WHY THIS EXISTS. ctx.ui.hud() is the primitive: it drives panel ids and dialog variables that
// some .xml declares. Used directly, every plugin needs its OWN layout, which means every plugin
// author must open Workshop Tools and publish an addon before drawing a single row. That is a
// non-starter for a plugin ecosystem, and it does not scale for a second reason:
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
// ES5 IIFE, concatenated after ui.js. Decorates the existing ctx.ui factory rather than editing it.
(function () {
  var prevUi = globalThis.__s2pkg_game_ctx && globalThis.__s2pkg_game_ctx.ui;
  if (typeof prevUi !== "function") return;

  var MODALS = 2;
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
  var FADE = { sheet: "s2-sheet-out", toast: "s2-toast-out", badge: "s2-hudbadge-out" };
  var TOAST_VARIANT = { good: "s2-toast-good", warn: "s2-toast-warn", bad: "s2-toast-bad" };
  var BADGE_ACCENT = { accent: "s2-hudbadge-accent", good: "s2-hudbadge-good",
                       warn: "s2-hudbadge-warn", bad: "s2-hudbadge-bad" };
  var BTN_VARIANT = { primary: "s2-btn-primary", good: "s2-btn-good", bad: "s2-btn-bad",
                      warn: "s2-btn-warn", ghost: "s2-btn-ghost" };
  var SHEET_WIDTH = { sm: "s2-sheet-sm", md: "", lg: "s2-sheet-lg", xl: "s2-sheet-xl" };
  var CORNER = { tl: "s2-corner-tl", tr: "s2-corner-tr", bl: "s2-corner-bl", br: "s2-corner-br" };

  function log(msg) { if (globalThis.console) console.log("[s2script/ui] " + msg); }

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

  var LIB_DESCRIPTOR = {
    addons: ["3790153369"],
    resource: "panorama/layout/custom_game/s2script_lib.xml",
    hideClass: CLS.hide,
    text: TEXT,
    buttons: BUTTONS,
    meters: {}
  };

  // ── shared state ────────────────────────────────────────────────────────────────────────────
  // Pool claims are HOST-GLOBAL, not per-plugin: the intern vectors live on the entity, so two
  // plugins must not both believe they own modal 0. Keyed on globalThis so a plugin reload does
  // not silently re-issue a slot another plugin still holds.

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
    var p = pool();
    var taken = p[kind];
    for (var i = 0; i < count; i++) {
      if (!taken[i]) { taken[i] = owner; return i; }
    }
    return -1;
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
  // is silently ignored. `after(ms, fn)` is the callback form. Only the toast hold needs a timer at
  // all now; showing a panel does not.
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
    var hud = hudApi.hud(descriptor || LIB_DESCRIPTOR);
    var ownerTag = "plugin";

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

    // ── badges (persistent corner HUD) ────────────────────────────────────────────────────────

    function badge(spec) {
      var s = spec || {};
      var idx = claim("badge", BADGES, ownerTag);
      if (idx < 0) { log("badge pool exhausted (" + BADGES + " in use) — request ignored"); return null; }
      var slotIds = BADGE[idx];
      var cornerCls = CORNER[s.corner] || CORNER.tr;
      var accentCls = BADGE_ACCENT[s.accent] || null;
      return {
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
        },
        hide: function (slot) { hide(slot, slotIds.id); },
        release: function () { pool().badge[idx] = null; }
      };
    }

    // ── modals (title + paged list + detail + footer buttons) ─────────────────────────────────

    function modal(spec) {
      var s = spec || {};
      var idx = claim("modal", MODALS, ownerTag);
      if (idx < 0) { log("modal pool exhausted (" + MODALS + " in use) — request ignored"); return null; }
      var ids = MODAL[idx];
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

      // Caller buttons first; prev/next take the trailing footer slots, and only when the data
      // actually needs paging — a one-page list shows no pager.
      function footerPlan(slot) {
        var plan = [];
        var mine = s.buttons || [];
        for (var i = 0; i < mine.length && plan.length < FOOTERS; i++) {
          plan.push({ text: mine[i].text, variant: mine[i].variant || "ghost", fn: mine[i].onClick });
        }
        if (pageCount(slot) > 1 && plan.length + 2 <= FOOTERS) {
          plan.push({ text: "‹ Prev", variant: "ghost", fn: function (sl) { self.page(sl, -1); } });
          plan.push({ text: "Next ›", variant: "ghost", fn: function (sl) { self.page(sl, 1); } });
        }
        return plan;
      }

      var footerFns = [];   // resolved per paint, read by the click handlers below

      function paint(slot) {
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
          hud.onClick(ids.rows[rowIndex].id, function (slot) {
            var st = open[slot];
            if (!st) return;
            st.cursor = rowIndex;
            var all = rowsFor(slot);
            var row = all[st.page * pageSize + rowIndex];
            if (row && s.onPick) s.onPick(slot, st.page * pageSize + rowIndex, row);
            paint(slot);
          });
        })(ri);
      }
      for (var fi = 0; fi < FOOTERS; fi++) {
        (function (fIndex) {
          hud.onClick(ids.footers[fIndex].id, function (slot) {
            if (!open[slot]) return;
            var fn = footerFns[fIndex];
            if (fn) fn(slot);
          });
        })(fi);
      }

      self = {
        open: function (slot) {
          open[slot] = { page: 0, cursor: 0 };
          for (var wk in SHEET_WIDTH) {
            if (Object.prototype.hasOwnProperty.call(SHEET_WIDTH, wk) && SHEET_WIDTH[wk] !== "") {
              setClass(slot, ids.root, SHEET_WIDTH[wk], SHEET_WIDTH[wk] === widthCls);
            }
          }
          paint(slot);                                        // fill first…
          setClass(slot, ids.root, FADE.sheet, false);         // …never leave it transparent…
          show(slot, ids.root, { cursor: true });              // …then reveal, never blank
        },
        close: function (slot) {
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
        release: function () { pool().modal[idx] = null; }
      };
      return self;
    }

    function hideAll(slot) {
      for (var m2 = 0; m2 < MODALS; m2++) hide(slot, MODAL[m2].root);
      for (var t2 = 0; t2 < TOASTS; t2++) hide(slot, TOAST[t2].id);
      for (var b2 = 0; b2 < BADGES; b2++) hide(slot, BADGE[b2].id);
      hud.cursor(slot, false);
    }

    return {
      descriptor: LIB_DESCRIPTOR,
      hud: hud,
      /**
       * Spawn the pool's layout entity. Same timing as `createLayout` — player-join, events,
       * commands, or any post-ready callback. `components()` also spawns once a client is active,
       * so this is only needed to force a spawn before the first `hud()` / `components()` call.
       */
      ensure: function () { return hudApi.createLayout(descriptor || LIB_DESCRIPTOR); },
      modal: modal,
      badge: badge,
      toast: toast,
      hideAll: hideAll,
      forget: function (slot) { hud.forget(slot); },
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

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var base = prevUi(reg, viaId);
      if (base && typeof base.hud === "function" && !base.components) {
        base.components = function (descriptor) { return makeComponents(base, descriptor); };
      }
      return base;
    }
  });
})();
