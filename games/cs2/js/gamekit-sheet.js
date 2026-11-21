// @s2script/cs2 — GameHud panels: the sheet and the prompt, on s2script_gamepanels.xml.
//
// SHEET. A full panel a player opens (rules, settings, a shop, a scoreboard of anything): side
// tabs on the left, one body on the right, a pager at the bottom. A section's body is one of five
// kinds: a numbered list, paragraphs, toggles, a card grid or a table with row actions. Bodies page
// themselves, so a plugin hands over all 14 rules and the sheet shows 6 at a time. List numbers are
// the item's place in the whole section (or its own `n`), so "rule 3" stays rule 3 on every page
// and a plugin can open a sheet on the page that holds it, highlighted. Side tabs that do not fit
// collapse to their group headers, and past that scroll with up and down slots.
//
// PROMPT. One question with up to four clickable answers, for one player, at a moment the plugin
// picks. It takes the cursor and has a Close button. If the HUD refuses it, the question goes to
// chat and that player may answer by typing the number; nobody else's chat is touched.
//
// Both are per player and claimed host-side (`gamekit:sheet:<slot>`, `gamekit:prompt:<slot>`).
//
// ES5 IIFE, concatenated after gamekit-panels.js, whose shared layout handle it uses.
(function () {
  var U = globalThis.__s2pkg_gamekit_util;
  var prevUi = globalThis.__s2pkg_game_ctx && globalThis.__s2pkg_game_ctx.ui;
  if (!U || typeof prevUi !== "function") return;

  var GK = U.GK;
  var SPEC = GK.spec;
  var HIDE = SPEC.hideClass;
  var SH = SPEC.sheet;
  var ANSWERS = SPEC.prompt.answers;
  var SEL = "gk-sel";
  var ON = "gk-on";
  var HL = "gk-hl";
  var GRP = "gk-grp";
  var DISABLED = "s2-btn-disabled";
  var LIST_PAGE = 6;
  var BODY_IDS = { list: "gk_sh_list", text: "gk_sh_para", toggles: "gk_sh_tog", grid: "gk_sh_grid", table: "gk_sh_table" };
  // Every panel id of the sheet starts with one of these (cache invalidation on a fresh claim).
  var SHEET_ROOTS = ["gk_sheet", "gk_sh"];
  var TONES = Object.keys(SPEC.tones || {});

  function str(v) { return v == null ? "" : String(v); }
  function has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); }
  function guard(what, fn) {
    try { return fn(); } catch (e) { U.log(what + " threw: " + ((e && e.stack) || e)); return undefined; }
  }

  // ── body model ─────────────────────────────────────────────────────────────────────────────
  function bodyOf(section) {
    var b = section && section.body ? section.body : { kind: "list", items: [] };
    var kind = BODY_IDS[b.kind] ? b.kind : "list";
    return { kind: kind, src: b };
  }

  function itemsOf(body) {
    var b = body.src;
    if (body.kind === "text") return Array.isArray(b.paragraphs) ? b.paragraphs : [];
    if (body.kind === "table") return Array.isArray(b.rows) ? b.rows : [];
    return Array.isArray(b.items) ? b.items : [];
  }

  function perPage(body) {
    if (body.kind === "list") {
      var n = Math.floor(Number(body.src.perPage));
      return n > 0 ? Math.min(n, SH.list) : Math.min(LIST_PAGE, SH.list);
    }
    if (body.kind === "text") return SH.paragraphs;
    if (body.kind === "toggles") return SH.toggles;
    if (body.kind === "grid") return SH.grid;
    return SH.tableRows;
  }

  function pageCount(body) { return Math.max(1, Math.ceil(itemsOf(body).length / perPage(body))); }

  /** A list item's stable number: its own `n`, else its place in the whole section. */
  function listNumber(item, index) {
    return item && typeof item === "object" && item.n != null ? str(item.n) : String(index + 1);
  }
  function listText(item) { return item && typeof item === "object" ? str(item.text) : str(item); }

  function findListIndex(body, n) {
    if (body.kind !== "list" || n == null) return -1;
    var items = itemsOf(body);
    for (var i = 0; i < items.length; i++) if (listNumber(items[i], i) === String(n)) return i;
    return -1;
  }

  // ── side tabs ──────────────────────────────────────────────────────────────────────────────
  // Entries: { kind: "group", group, first } | { kind: "tab", index } | { kind: "up" } | { kind: "down" }.
  function sideEntries(st) {
    var all = [];
    var lastGroup = null;
    var i;
    for (i = 0; i < st.sections.length; i++) {
      var g = st.sections[i].group == null ? null : String(st.sections[i].group);
      if (g !== null && g !== lastGroup) all.push({ kind: "group", group: g, first: i });
      lastGroup = g;
      all.push({ kind: "tab", index: i, group: g });
    }
    var list = all;
    if (list.length > SH.side) {
      // Collapse every group but the open section's.
      var openGroup = st.sections[st.section] ? st.sections[st.section].group : null;
      openGroup = openGroup == null ? null : String(openGroup);
      list = [];
      for (i = 0; i < all.length; i++) {
        var e = all[i];
        if (e.kind === "tab" && e.group !== null && e.group !== openGroup) continue;
        list.push(e);
      }
    }
    if (list.length <= SH.side) { st.sideOffset = 0; return list; }
    // Still too many: scroll. The first slot is an up arrow when there is more above and the
    // last a down arrow when there is more below; the rest show entries, keeping the open tab in view.
    function room(o) {
      var r = SH.side - (o > 0 ? 1 : 0);
      return o + r < list.length ? r - 1 : r;
    }
    var at = 0;
    for (i = 0; i < list.length; i++) if (list[i].kind === "tab" && list[i].index === st.section) at = i;
    var off = Math.max(0, Math.min(st.sideOffset || 0, list.length - room(list.length)));
    if (st.sideFollow) {
      if (at < off) off = at;
      for (var pass = 0; pass < 3 && at >= off + room(off); pass++) off = at - room(off) + 1;
      st.sideFollow = false;
    }
    st.sideOffset = off;
    var out = [];
    if (off > 0) out.push({ kind: "up" });
    var r = room(off);
    for (i = off; i < off + r && i < list.length; i++) out.push(list[i]);
    if (off + r < list.length) out.push({ kind: "down" });
    return out;
  }

  function makeSheets(kit) {
    var sheets = {};   // slot -> state
    var prompts = {};  // slot -> state

    function note(acc, err) { if (!kit.ok(err) && acc.first === null) acc.first = err; }

    // ── sheet painting ───────────────────────────────────────────────────────────────────────
    function paintSide(st, acc) {
      var slot = st.slot;
      var entries = sideEntries(st);
      st.side = entries;
      for (var i = 0; i < SH.side; i++) {
        var id = "gk_sh_s" + i;
        var e = entries[i];
        if (!e) {
          if (st.shown[id]) emptyRow(st, "side", id, acc); else note(acc, kit.hide(slot, id));
          continue;
        }
        var label = "";
        var meta = "";
        if (e.kind === "group") { label = e.group; }
        else if (e.kind === "up") { label = "▲"; }
        else if (e.kind === "down") { label = "▼"; }
        else { label = st.sections[e.index].label; meta = str(st.sections[e.index].meta); }
        note(acc, kit.setText(slot, id + "_t", label));
        note(acc, kit.setText(slot, id + "_m", meta));
        note(acc, kit.setClass(slot, id, GRP, e.kind === "group"));
        note(acc, kit.setClass(slot, id, SEL, e.kind === "tab" && e.index === st.section));
        showRow(st, "side", id, acc);
      }
      note(acc, kit.setClass(slot, "gk_sh_more", HIDE, true));
    }

    // ── body rows ────────────────────────────────────────────────────────────────────────────
    // The row ids of each body kind, and the text and state each row carries.
    function rowsOf(kind) {
      var out = [];
      var i;
      if (kind === "list") for (i = 0; i < SH.list; i++) out.push("gk_sh_li" + i);
      else if (kind === "text") for (i = 0; i < SH.paragraphs; i++) out.push("gk_sh_p" + i);
      else if (kind === "toggles") for (i = 0; i < SH.toggles; i++) out.push("gk_sh_tg" + i);
      else if (kind === "grid") for (i = 0; i < SH.grid; i++) out.push("gk_sh_g" + i);
      else if (kind === "table") for (i = 0; i < SH.tableRows; i++) out.push("gk_sh_tr" + i);
      else if (kind === "side") for (i = 0; i < SH.side; i++) out.push("gk_sh_s" + i);
      return out;
    }

    /** Hide a row and drop everything painted on it: its text, its state classes, its buttons. */
    function emptyRow(st, kind, id, acc) {
      var slot = st.slot;
      var c;
      note(acc, kit.hide(slot, id));
      if (kind === "list") {
        note(acc, kit.setText(slot, id + "_n", ""));
        note(acc, kit.setText(slot, id + "_t", ""));
        note(acc, kit.setClass(slot, id, HL, false));
      } else if (kind === "text") {
        note(acc, kit.setText(slot, id, ""));
      } else if (kind === "toggles" || kind === "grid") {
        var vars = kind === "toggles" ? ["_t", "_v"] : ["_t", "_p", "_d"];
        for (c = 0; c < vars.length; c++) note(acc, kit.setText(slot, id + vars[c], ""));
        note(acc, kit.setClass(slot, id, kind === "toggles" ? ON : SEL, false));
        note(acc, kit.setClass(slot, id, DISABLED, false));
        note(acc, kit.P().setDisabled(slot, id, false));
      } else if (kind === "table") {
        for (c = 0; c < SH.tableCols; c++) note(acc, kit.setText(slot, id + "_c" + c, ""));
        for (var a = 0; a < SH.rowActions; a++) {
          var x = id + "_x" + a;
          note(acc, kit.hide(slot, x));
          note(acc, kit.setText(slot, x + "_t", ""));
          note(acc, kit.setClass(slot, x, DISABLED, false));
          note(acc, kit.P().setDisabled(slot, x, false));
        }
      } else if (kind === "side") {
        note(acc, kit.setText(slot, id + "_t", ""));
        note(acc, kit.setText(slot, id + "_m", ""));
        note(acc, kit.setClass(slot, id, GRP, false));
        note(acc, kit.setClass(slot, id, SEL, false));
      }
      delete st.shown[id];
    }

    function showRow(st, kind, id, acc) {
      st.shown[id] = kind;
      note(acc, kit.show(st.slot, id));
    }

    // Show the body in use and hide every other kind's container AND its rows, so nothing an
    // earlier body painted (this sheet's or another plugin's) can stay on screen under the new one.
    // Rows this sheet showed are emptied as well; the rest are only hidden, which the diff cache
    // answers locally once they are.
    function hideBodies(st, keep, acc) {
      var slot = st.slot;
      for (var k in BODY_IDS) {
        if (!has(BODY_IDS, k)) continue;
        note(acc, kit.setClass(slot, BODY_IDS[k], HIDE, k !== keep));
        if (k === keep) continue;
        var rows = rowsOf(k);
        for (var i = 0; i < rows.length; i++) {
          if (st.shown[rows[i]]) emptyRow(st, k, rows[i], acc);
          else note(acc, kit.hide(slot, rows[i]));
        }
        if (k === "table") for (var c = 0; c < SH.tableCols; c++) note(acc, kit.setClass(slot, "gk_sh_th" + c, HIDE, true));
      }
    }

    /** Empty the whole sheet panel (on close), so the next sheet for this player starts clean. */
    function emptySheet(st) {
      var acc = { first: null };
      var slot = st.slot;
      hideBodies(st, null, acc);
      var side = rowsOf("side");
      for (var i = 0; i < side.length; i++) {
        if (st.shown[side[i]]) emptyRow(st, "side", side[i], acc);
        else note(acc, kit.hide(slot, side[i]));
      }
      var texts = ["gk_sh_title", "gk_sh_right", "gk_sh_ft", "gk_sh_h", "gk_sh_k"];
      for (i = 0; i < texts.length; i++) note(acc, kit.setText(slot, texts[i], ""));
      note(acc, kit.setClass(slot, "gk_sh_k", HIDE, true));
      note(acc, kit.setClass(slot, "gk_sh_pager", HIDE, true));
      st.shown = {};
      return acc.first;
    }

    /** Set an action label's tone. Right after a claim, clear every other tone: whatever painted
     *  this panel before may have left one this context never recorded. */
    function toneOf(st, id, cls, acc) {
      if (st.fresh) {
        for (var t = 0; t < TONES.length; t++) {
          if ("gk-t-" + TONES[t] !== cls) note(acc, kit.setClass(st.slot, id, "gk-t-" + TONES[t], false));
        }
      }
      note(acc, kit.swap(st.slot, id, "text", cls));
    }

    function pageSlice(st, body) {
      var per = perPage(body);
      var items = itemsOf(body);
      var start = st.page * per;
      return { start: start, items: items.slice(start, start + per) };
    }

    function paintBody(st, acc) {
      var slot = st.slot;
      var section = st.sections[st.section];
      var body = bodyOf(section);
      var pages = pageCount(body);
      if (st.page >= pages) st.page = pages - 1;
      if (st.page < 0) st.page = 0;
      var heading = section ? (section.heading != null ? section.heading : section.label) : "";
      var kicker = section && section.kicker != null ? str(section.kicker) : (pages > 1 ? "Page " + (st.page + 1) + " of " + pages : "");
      note(acc, kit.setText(slot, "gk_sh_h", heading));
      note(acc, kit.setText(slot, "gk_sh_k", kicker));
      note(acc, kit.setClass(slot, "gk_sh_k", HIDE, kicker === ""));
      hideBodies(st, body.kind, acc);
      var page = pageSlice(st, body);
      var i;
      var id;
      var item;
      // A row past the page's items: empty it if it showed something, else just keep it hidden.
      function clearRow(rowId) {
        if (st.shown[rowId]) emptyRow(st, body.kind, rowId, acc); else note(acc, kit.hide(slot, rowId));
      }
      if (body.kind === "list") {
        for (i = 0; i < SH.list; i++) {
          id = "gk_sh_li" + i;
          if (i >= page.items.length) { clearRow(id); continue; }
          item = page.items[i];
          note(acc, kit.setText(slot, id + "_n", listNumber(item, page.start + i)));
          note(acc, kit.setText(slot, id + "_t", listText(item)));
          note(acc, kit.setClass(slot, id, HL, st.highlight === page.start + i));
          showRow(st, "list", id, acc);
        }
      } else if (body.kind === "text") {
        for (i = 0; i < SH.paragraphs; i++) {
          id = "gk_sh_p" + i;
          if (i >= page.items.length) { clearRow(id); continue; }
          note(acc, kit.setText(slot, id, page.items[i]));
          showRow(st, "text", id, acc);
        }
      } else if (body.kind === "toggles") {
        for (i = 0; i < SH.toggles; i++) {
          id = "gk_sh_tg" + i;
          if (i >= page.items.length) { clearRow(id); continue; }
          item = page.items[i] || {};
          note(acc, kit.setText(slot, id + "_t", item.label));
          note(acc, kit.setText(slot, id + "_v", item.value != null ? item.value : (item.on ? "On" : "Off")));
          note(acc, kit.setClass(slot, id, ON, !!item.on));
          note(acc, kit.setClass(slot, id, DISABLED, !!item.disabled));
          note(acc, kit.P().setDisabled(slot, id, !!item.disabled));
          showRow(st, "toggles", id, acc);
        }
      } else if (body.kind === "grid") {
        for (i = 0; i < SH.grid; i++) {
          id = "gk_sh_g" + i;
          if (i >= page.items.length) { clearRow(id); continue; }
          item = page.items[i] || {};
          note(acc, kit.setText(slot, id + "_t", item.title));
          note(acc, kit.setText(slot, id + "_p", item.price == null ? "" : item.price));
          note(acc, kit.setText(slot, id + "_d", item.desc == null ? "" : item.desc));
          note(acc, kit.setClass(slot, id, SEL, !!item.selected));
          note(acc, kit.setClass(slot, id, DISABLED, !!item.disabled));
          note(acc, kit.P().setDisabled(slot, id, !!item.disabled));
          showRow(st, "grid", id, acc);
        }
      } else {
        var cols = Array.isArray(body.src.columns) ? body.src.columns : [];
        var c;
        for (c = 0; c < SH.tableCols; c++) {
          note(acc, kit.setText(slot, "gk_sh_th" + c, c < cols.length ? cols[c] : ""));
          note(acc, kit.setClass(slot, "gk_sh_th" + c, HIDE, c >= cols.length));
        }
        for (i = 0; i < SH.tableRows; i++) {
          id = "gk_sh_tr" + i;
          if (i >= page.items.length) { clearRow(id); continue; }
          item = page.items[i] || {};
          var cells = Array.isArray(item.cells) ? item.cells : [];
          for (c = 0; c < SH.tableCols; c++) {
            note(acc, kit.setText(slot, id + "_c" + c, c < cells.length ? cells[c] : ""));
            note(acc, kit.setClass(slot, id + "_c" + c, HIDE, c >= cols.length));
          }
          var acts = Array.isArray(item.actions) ? item.actions : [];
          for (var a = 0; a < SH.rowActions; a++) {
            var x = id + "_x" + a;
            var act = acts[a];
            if (!act) { note(acc, kit.hide(slot, x)); continue; }
            toneOf(st, x + "_t", "gk-t-" + U.pickTone(act.tone, act.label, "default"), acc);
            note(acc, kit.setText(slot, x + "_t", act.label));
            note(acc, kit.setClass(slot, x, DISABLED, !!act.disabled));
            note(acc, kit.P().setDisabled(slot, x, !!act.disabled));
            note(acc, kit.show(slot, x));
          }
          showRow(st, "table", id, acc);
        }
      }
      // Pager.
      note(acc, kit.setClass(slot, "gk_sh_pager", HIDE, pages <= 1));
      if (pages > 1) {
        note(acc, kit.setText(slot, "gk_sh_prev_t", "‹"));
        note(acc, kit.setText(slot, "gk_sh_next_t", "›"));
        note(acc, kit.P().setDisabled(slot, "gk_sh_prev", st.page <= 0));
        note(acc, kit.setClass(slot, "gk_sh_prev", DISABLED, st.page <= 0));
        note(acc, kit.P().setDisabled(slot, "gk_sh_next", st.page >= pages - 1));
        note(acc, kit.setClass(slot, "gk_sh_next", DISABLED, st.page >= pages - 1));
        var dots = Math.min(pages, SH.dots);
        var lit = pages <= SH.dots ? st.page : Math.floor((st.page * SH.dots) / pages);
        for (i = 0; i < SH.dots; i++) {
          note(acc, kit.setClass(slot, "gk_sh_d" + i, HIDE, i >= dots));
          note(acc, kit.setClass(slot, "gk_sh_d" + i, ON, i === lit));
        }
      }
    }

    function paintSheet(st) {
      var acc = { first: null };
      var slot = st.slot;
      note(acc, kit.setText(slot, "gk_sh_title", st.title));
      note(acc, kit.setText(slot, "gk_sh_right", st.right));
      note(acc, kit.setText(slot, "gk_sh_close_t", "Close"));
      note(acc, kit.setText(slot, "gk_sh_ft", st.footer));
      paintSide(st, acc);
      paintBody(st, acc);
      note(acc, kit.show(slot, "gk_sheet", { cursor: true }));
      st.fresh = false;
      return acc.first;
    }

    function sheetChat(st) {
      var section = st.sections[st.section];
      if (!section) return;
      var body = bodyOf(section);
      var page = pageSlice(st, body);
      var pages = pageCount(body);
      U.chat(st.slot, "{orange}" + U.plainText(st.title) + "{grey}: " + U.plainText(section.heading != null ? section.heading : section.label) +
        (pages > 1 ? " {yellow}" + (st.page + 1) + "/" + pages : ""));
      for (var i = 0; i < page.items.length; i++) {
        var it = page.items[i];
        var line;
        if (body.kind === "list") line = "{yellow}" + listNumber(it, page.start + i) + "{grey}. " + listText(it);
        else if (body.kind === "text") line = "{grey}" + str(it);
        else if (body.kind === "toggles") line = "{grey}" + str(it && it.label) + ": " + (it && it.on ? "{green}On" : "{red}Off");
        else if (body.kind === "grid") line = "{grey}" + str(it && it.title) + (it && it.price != null ? " {yellow}" + it.price : "");
        else line = "{grey}" + (it && Array.isArray(it.cells) ? it.cells.join("  ") : "");
        U.chat(st.slot, line);
      }
    }

    function repaintSheet(st) {
      var err = paintSheet(st);
      if (!kit.ok(err) && !/stale client/i.test(String(err))) sheetChat(st);
    }

    function sectionIndex(st, id) {
      for (var i = 0; i < st.sections.length; i++) if (st.sections[i].id === id) return i;
      return -1;
    }

    /** Point the sheet at a section (by id) and optionally a list number to highlight. */
    // Only an explicit section or highlight moves the sheet: a plain refresh keeps the player's
    // page and side-tab scroll.
    function focus(st, sectionId, highlight) {
      if (sectionId === undefined && highlight == null) return;
      if (sectionId !== undefined) {
        var at = sectionIndex(st, sectionId);
        if (at >= 0 && at !== st.section) { st.section = at; st.page = 0; }
      }
      st.highlight = -1;
      st.sideFollow = true;
      if (highlight != null) {
        var body = bodyOf(st.sections[st.section]);
        var idx = findListIndex(body, highlight);
        if (idx >= 0) { st.highlight = idx; st.page = Math.floor(idx / perPage(body)); }
      }
    }

    // A spec field left out keeps its current value (see the drawer).
    function applySheetSpec(st, spec) {
      if (spec.title !== undefined) st.title = str(spec.title);
      if (spec.right !== undefined) st.right = str(spec.right);
      if (spec.footer !== undefined) st.footer = str(spec.footer);
      if (spec.sections !== undefined) {
        st.sections = Array.isArray(spec.sections) ? spec.sections.map(function (s) {
          s = s || {};
          return {
            id: s.id == null ? str(s.label) : s.id, label: str(s.label == null ? s.id : s.label), group: s.group,
            meta: s.meta, heading: s.heading, kicker: s.kicker, body: s.body
          };
        }) : [];
      }
      if (spec.onSelect !== undefined) st.onSelect = spec.onSelect;
      if (spec.onSection !== undefined) st.onSection = spec.onSection;
      if (spec.onClose !== undefined) st.onClose = spec.onClose;
      if (st.section >= st.sections.length) { st.section = 0; st.page = 0; }
      focus(st, spec.section, spec.highlight);
    }

    // `gone`: the player left or the map changed, so there is no panel left to empty.
    function closeSheet(st, fromPlayer, gone) {
      if (!st.live) return;
      st.live = false;
      if (sheets[st.slot] === st) delete sheets[st.slot];
      kit.hide(st.slot, "gk_sheet");
      if (!gone) emptySheet(st);
      U.hostRelease("gamekit:sheet:" + st.slot);
      if (typeof st.onClose === "function") guard("sheet onClose", function () { st.onClose(fromPlayer); });
    }

    function openSheet(player, spec) {
      var slot = U.slotOf(player);
      if (slot < 0) throw new Error("GameHud.sheet: a player or slot is required");
      spec = spec || {};
      var existing = sheets[slot];
      if (existing && existing.live) { applySheetSpec(existing, spec); repaintSheet(existing); return existing.handle; }
      if (U.hostClaim("gamekit:sheet:" + slot) < 0) return null;
      var st = {
        slot: slot, live: true, section: 0, page: 0, highlight: -1, sideOffset: 0, sideFollow: true,
        title: "", right: "", footer: "", sections: [], shown: {}, fresh: true
      };
      applySheetSpec(st, spec);
      sheets[slot] = st;
      kit.P();
      // A fresh claim: the panel was last painted by whoever held it before (another plugin, or
      // this one before a close), from a context whose diff cache this one cannot see. Forget
      // ours so the first paint sends every value instead of skipping the ones it thinks match.
      kit.forget(slot, SHEET_ROOTS);
      st.handle = {
        isOpen: function () { return st.live; },
        update: function (next) { if (st.live) { applySheetSpec(st, next || {}); repaintSheet(st); } },
        show: function (sectionId, highlight) { if (st.live) { focus(st, sectionId, highlight); repaintSheet(st); } },
        page: function (n) {
          if (!st.live) return;
          st.page = Math.max(0, Math.floor(Number(n)) || 0);
          st.highlight = -1;
          repaintSheet(st);
        },
        close: function () { closeSheet(st, false); }
      };
      repaintSheet(st);
      return st.handle;
    }

    function liveSheet(player) {
      var st = sheets[player.slot];
      return st && st.live ? st : null;
    }

    function selectEvent(st, kind, offset, extra) {
      var section = st.sections[st.section];
      var body = bodyOf(section);
      var index = st.page * perPage(body) + offset;
      var item = itemsOf(body)[index];
      if (item === undefined) return null;
      var ev = { section: section.id, kind: kind, index: index, item: item };
      for (var k in extra) if (has(extra, k)) ev[k] = extra[k];
      return ev;
    }

    function fireSelect(st, ev) {
      if (!ev || typeof st.onSelect !== "function") return;
      guard("sheet onSelect", function () { st.onSelect(ev, st.handle); });
    }

    var i;
    for (i = 0; i < SH.side; i++) {
      (function (n) {
        kit.route("gk_sh_s" + n, function (player) {
          var st = liveSheet(player);
          if (!st || !st.side) return;
          var e = st.side[n];
          if (!e) return;
          if (e.kind === "up") { st.sideOffset = Math.max(0, st.sideOffset - (SH.side - 2)); if (st.sideOffset === 1) st.sideOffset = 0; repaintSheet(st); return; }
          if (e.kind === "down") { st.sideOffset += SH.side - 2; repaintSheet(st); return; }
          var target = e.kind === "group" ? e.first : e.index;
          if (target === st.section) return;
          st.section = target;
          st.page = 0;
          st.highlight = -1;
          st.sideFollow = true;
          var id = st.sections[target].id;
          if (typeof st.onSection === "function") guard("sheet onSection", function () { st.onSection(id, st.handle); });
          if (st.live) repaintSheet(st);
        });
      })(i);
    }
    for (i = 0; i < SH.toggles; i++) {
      (function (n) {
        kit.route("gk_sh_tg" + n, function (player) {
          var st = liveSheet(player);
          if (!st || bodyOf(st.sections[st.section]).kind !== "toggles") return;
          var ev = selectEvent(st, "toggle", n, {});
          if (!ev || (ev.item && ev.item.disabled)) return;
          ev.on = !(ev.item && ev.item.on);
          fireSelect(st, ev);
        });
      })(i);
    }
    for (i = 0; i < SH.grid; i++) {
      (function (n) {
        kit.route("gk_sh_g" + n, function (player) {
          var st = liveSheet(player);
          if (!st || bodyOf(st.sections[st.section]).kind !== "grid") return;
          var ev = selectEvent(st, "item", n, {});
          if (!ev || (ev.item && ev.item.disabled)) return;
          fireSelect(st, ev);
        });
      })(i);
    }
    for (i = 0; i < SH.tableRows; i++) {
      for (var a = 0; a < SH.rowActions; a++) {
        (function (n, ai) {
          kit.route("gk_sh_tr" + n + "_x" + ai, function (player) {
            var st = liveSheet(player);
            if (!st || bodyOf(st.sections[st.section]).kind !== "table") return;
            var ev = selectEvent(st, "action", n, {});
            if (!ev) return;
            var act = ev.item && Array.isArray(ev.item.actions) ? ev.item.actions[ai] : null;
            if (!act || act.disabled) return;
            ev.action = act.id == null ? act.label : act.id;
            fireSelect(st, ev);
          });
        })(i, a);
      }
    }
    kit.route("gk_sh_prev", function (player) {
      var st = liveSheet(player);
      if (!st || st.page <= 0) return;
      st.page--;
      st.highlight = -1;
      repaintSheet(st);
    });
    kit.route("gk_sh_next", function (player) {
      var st = liveSheet(player);
      if (!st) return;
      if (st.page >= pageCount(bodyOf(st.sections[st.section])) - 1) return;
      st.page++;
      st.highlight = -1;
      repaintSheet(st);
    });
    kit.route("gk_sh_close", function (player) {
      var st = liveSheet(player);
      if (st) closeSheet(st, true);
    });

    // ── prompt ─────────────────────────────────────────────────────────────────────────────
    function answerOf(raw) {
      if (raw && typeof raw === "object") {
        return { id: raw.id == null ? raw.label : raw.id, label: str(raw.label), tone: raw.tone, disabled: !!raw.disabled };
      }
      return { id: raw, label: str(raw), tone: undefined, disabled: false };
    }

    function paintPrompt(st) {
      var acc = { first: null };
      var slot = st.slot;
      note(acc, kit.setText(slot, "gk_pr_title", st.title));
      note(acc, kit.setText(slot, "gk_pr_right", st.remaining > 0 ? st.remaining + "s" : ""));
      note(acc, kit.setText(slot, "gk_pr_close_t", "Close"));
      note(acc, kit.setText(slot, "gk_pr_q", st.question));
      note(acc, kit.setText(slot, "gk_pr_why", st.why));
      note(acc, kit.setClass(slot, "gk_pr_why", HIDE, st.why === ""));
      for (var i = 0; i < ANSWERS; i++) {
        var id = "gk_pr_a" + i;
        var ans = st.answers[i];
        if (!ans) { note(acc, kit.hide(slot, id)); continue; }
        note(acc, kit.swap(slot, id + "_t", "text", "gk-t-" + U.pickTone(ans.tone, ans.label, "default")));
        note(acc, kit.setText(slot, id + "_t", ans.label));
        note(acc, kit.setClass(slot, id, DISABLED, ans.disabled));
        note(acc, kit.P().setDisabled(slot, id, ans.disabled));
        note(acc, kit.show(slot, id));
      }
      note(acc, kit.setText(slot, "gk_pr_ft", st.footer));
      note(acc, kit.setClass(slot, "gk_pr_ft", HIDE, st.footer === ""));
      note(acc, kit.show(slot, "gk_prompt", { cursor: true }));
      return acc.first;
    }

    function promptChat(st) {
      U.chat(st.slot, "{orange}" + U.plainText(st.title || "Question") + "{grey}: " + st.question);
      if (st.why) U.chat(st.slot, "{grey}" + st.why);
      for (var i = 0; i < st.answers.length; i++) {
        if (!st.answers[i].disabled) U.chat(st.slot, "{yellow}" + (i + 1) + "{grey}. " + st.answers[i].label);
      }
      U.chat(st.slot, "{blue}Type the number in chat to answer.");
    }

    function repaintPrompt(st) {
      var err = paintPrompt(st);
      if (!kit.ok(err) && !/stale client/i.test(String(err))) {
        if (!st.inChat) { st.inChat = true; promptChat(st); }
      }
    }

    function endPrompt(st, index, reason) {
      if (!st.live) return;
      st.live = false;
      if (prompts[st.slot] === st) delete prompts[st.slot];
      kit.hide(st.slot, "gk_prompt");
      if (reason !== "replaced") U.hostRelease("gamekit:prompt:" + st.slot);
      if (typeof st.onAnswer === "function") {
        var ans = index === null ? null : st.answers[index];
        guard("prompt onAnswer", function () {
          st.onAnswer({ index: index, id: ans ? ans.id : null, reason: reason });
        });
      }
    }

    function promptTick(st) {
      U.afterSeconds(1, function () {
        if (!st.live) return;
        st.remaining = Math.max(0, st.remaining - 1);
        if (st.remaining <= 0) { endPrompt(st, null, "timeout"); return; }
        if (!st.inChat) kit.setText(st.slot, "gk_pr_right", st.remaining + "s");
        promptTick(st);
      });
    }

    function openPrompt(player, spec) {
      var slot = U.slotOf(player);
      if (slot < 0) throw new Error("GameHud.prompt: a player or slot is required");
      spec = spec || {};
      if (spec.question == null) throw new Error("GameHud.prompt: spec.question is required");
      var raw = Array.isArray(spec.answers) ? spec.answers : [];
      if (raw.length < 1 || raw.length > ANSWERS) {
        throw new Error("GameHud.prompt: between 1 and " + ANSWERS + " answers are required (got " + raw.length + ")");
      }
      var existing = prompts[slot];
      if (existing && existing.live) endPrompt(existing, null, "replaced");
      else if (U.hostClaim("gamekit:prompt:" + slot) < 0) return null;
      var seconds = spec.seconds > 0 ? Math.floor(Number(spec.seconds)) : 0;
      var st = {
        slot: slot, live: true, inChat: false,
        title: str(spec.title), question: str(spec.question), why: str(spec.why), footer: str(spec.footer),
        answers: raw.map(answerOf), remaining: seconds, onAnswer: spec.onAnswer
      };
      prompts[slot] = st;
      kit.P();
      repaintPrompt(st);
      if (seconds > 0) promptTick(st);
      return {
        isOpen: function () { return st.live; },
        close: function () { endPrompt(st, null, "cancelled"); }
      };
    }

    for (i = 0; i < ANSWERS; i++) {
      (function (n) {
        kit.route("gk_pr_a" + n, function (player) {
          var st = prompts[player.slot];
          if (!st || !st.live || !st.answers[n] || st.answers[n].disabled) return;
          endPrompt(st, n, "answer");
        });
      })(i);
    }
    kit.route("gk_pr_close", function (player) {
      var st = prompts[player.slot];
      if (st && st.live) endPrompt(st, null, "closed");
    });

    // Typed answers only for a player whose prompt went to chat. It runs before the vote's
    // handler: a player answering a prompt in chat means the prompt, not a vote option.
    kit.onChat(true, function (slot, text) {
      var st = prompts[slot];
      if (!st || !st.live || !st.inChat) return undefined;
      var t = String(text).trim();
      if (!/^[1-9]$/.test(t)) return undefined;
      var n = parseInt(t, 10) - 1;
      if (!st.answers[n] || st.answers[n].disabled) return undefined;
      endPrompt(st, n, "answer");
      return kit.HANDLED;
    });

    kit.onForget(function (slot) {
      var sh = sheets[slot];
      if (sh) closeSheet(sh, false, true);
      var pr = prompts[slot];
      if (pr) endPrompt(pr, null, "cancelled");
    });
    kit.onReset(function () {
      var s;
      for (s in sheets) if (has(sheets, s)) closeSheet(sheets[s], false, true);
      for (s in prompts) if (has(prompts, s)) endPrompt(prompts[s], null, "cancelled");
    });

    return { sheet: openSheet, prompt: openPrompt };
  }

  var live = null;

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var base = prevUi(reg, viaId);
      var panels = base && base.gamePanels;
      if (panels && panels._kit && !panels.sheets) {
        panels.sheets = makeSheets(panels._kit);
        if (!live) live = panels.sheets;
      }
      return base;
    }
  });

  function liveOrThrow(name) {
    if (!live) throw new Error("s2script: GameHud." + name + " requires plugin context; use it in OnPluginStart or a later callback");
    return live;
  }

  var cs2 = globalThis.__s2pkg_cs2;
  if (cs2 && cs2.GameHud) {
    var merged = {};
    for (var k in cs2.GameHud) if (has(cs2.GameHud, k)) merged[k] = cs2.GameHud[k];
    merged.sheet = function (player, spec) { return liveOrThrow("sheet").sheet(player, spec); };
    merged.prompt = function (player, spec) { return liveOrThrow("prompt").prompt(player, spec); };
    cs2.GameHud = Object.freeze(merged);
  }
})();
