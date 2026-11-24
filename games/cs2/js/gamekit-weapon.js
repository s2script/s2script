// @s2script/cs2 — GameHud.weapon: the weapon card, a replacement for the stock weapons HUD.
//
// A gamemode that hides the stock weapons HUD (`Hud.hide(slot, HideHud.Weapons)`, m_iHideHUD bit 6)
// loses the ammo count with it. This region of s2script_gamehud.xml (bottom right) puts it back:
// the active weapon's name, clip / reserve in large text, a low-ammo tone, and a compact slot list
// whose rows mark the item in hand (the knife and the Zeus share a row, and so do the grenades).
//
// ONE OWNER. The card is a region of the shared in-round layout, claimed like the rail and status
// card (`__s2_ui_pool_claim("gamekit:weapon", 1)`, ledgered, freed on unload). `claim()` answers
// true or false instead of returning a handle, and the other calls throw until this plugin holds it.
//
// AUTO. `auto(audience, options)` fills the card from the pawn. Each player is read once every
// POLL_EVERY frames (about 16 Hz at 64 tick), and the players are spread across those frames so a
// full server costs a quarter of its players per frame: the active weapon (m_hActiveWeapon) and its
// m_iClip1 / m_pReserveAmmo[0] on every read, the held weapons (m_hMyWeapons) and the slot list
// every INVENTORY_POLLS reads and whenever the active weapon changes. A paint only sends what
// changed, so a read where nothing moved costs reads and no HUD traffic. 16 Hz and not slower: a
// rifle fires about ten shots a second, and a slower count would skip numbers. The card hides
// while the player is dead or spectating (the spectate hub covers that), and follows a bot the
// player has taken over. `set()` and `hide()` take a player out of auto; `auto()` puts them back.
//
// The frame subscription is the plugin's own (ledgered with its context), and it is dropped on
// release or when no player is left on auto.
//
// ES5 IIFE, concatenated after gamekit.js (same context, shares its layout entity).
(function () {
  var U = globalThis.__s2pkg_gamekit_util;
  var prevUi = globalThis.__s2pkg_game_ctx && globalThis.__s2pkg_game_ctx.ui;
  if (!U || typeof prevUi !== "function") return;

  var SPEC = U.GK.spec;
  var HIDE = SPEC.hideClass;
  var SLOTS = (SPEC.weapon && SPEC.weapon.slots) || ["primary", "secondary", "knife", "grenades", "other"];
  var ROW_ITEMS = (SPEC.weapon && SPEC.weapon.items) || 4;
  var LOW_FRACTION = 0.25;
  var POLL_EVERY = 4;        // frames between reads of one player (about 16 Hz at 64 tick)
  var INVENTORY_POLLS = 4;   // reads between inventory reads (about 4 Hz)
  var MAX_WEAPONS = 64;
  var AMMO_SLOTS = 32;       // CPlayer_WeaponServices::m_iAmmo is uint16[32]

  // ── CS2 items: default names and the slot each one sits in ─────────────────────────────────
  // Keyed by item definition index. An index not listed here falls back to the weapon's VData
  // gear slot, and its name to "Weapon". Knife skins (500 and up) are all "Knife".
  var P = "primary", S2 = "secondary", K = "knife", G = "grenades", O = "other";
  var ITEMS = {
    1: ["Desert Eagle", S2], 2: ["Dual Berettas", S2], 3: ["Five-SeveN", S2], 4: ["Glock-18", S2],
    7: ["AK-47", P], 8: ["AUG", P], 9: ["AWP", P], 10: ["FAMAS", P], 11: ["G3SG1", P], 13: ["Galil AR", P],
    14: ["M249", P], 16: ["M4A4", P], 17: ["MAC-10", P], 19: ["P90", P], 23: ["MP5-SD", P], 24: ["UMP-45", P],
    25: ["XM1014", P], 26: ["PP-Bizon", P], 27: ["MAG-7", P], 28: ["Negev", P], 29: ["Sawed-Off", P],
    30: ["Tec-9", S2], 31: ["Zeus x27", K], 32: ["P2000", S2], 33: ["MP7", P], 34: ["MP9", P], 35: ["Nova", P],
    36: ["P250", S2], 38: ["SCAR-20", P], 39: ["SG 553", P], 40: ["SSG 08", P], 41: ["Knife", K], 42: ["Knife", K],
    43: ["Flashbang", G], 44: ["HE Grenade", G], 45: ["Smoke", G], 46: ["Molotov", G], 47: ["Decoy", G],
    48: ["Incendiary", G], 49: ["C4", O], 57: ["Medi-Shot", O], 59: ["Knife", K], 60: ["M4A1-S", P],
    61: ["USP-S", S2], 63: ["CZ75-Auto", S2], 64: ["R8 Revolver", S2], 68: ["TA Grenade", G], 69: ["Fists", K],
    70: ["Breach Charge", O], 72: ["Tablet", O], 74: ["Knife", K], 75: ["Axe", K], 76: ["Hammer", K],
    78: ["Wrench", K], 80: ["Spectral Shiv", K], 81: ["Fire Bomb", G], 82: ["Diversion", G], 83: ["Frag Grenade", G],
    84: ["Snowball", G], 85: ["Bump Mine", O]
  };
  // The game client's own equipment icons, by item definition index. The files ship with CS2
  // (panorama/images/icons/equipment/<name>.svg), so the workshop addon carries none. An item with
  // no entry shows its name instead.
  var ICONS = {
    1: "deagle", 2: "elite", 3: "fiveseven", 4: "glock", 7: "ak47", 8: "aug", 9: "awp", 10: "famas",
    11: "g3sg1", 13: "galilar", 14: "m249", 16: "m4a1", 17: "mac10", 19: "p90", 23: "mp5sd", 24: "ump45",
    25: "xm1014", 26: "bizon", 27: "mag7", 28: "negev", 29: "sawedoff", 30: "tec9", 31: "taser",
    32: "hkp2000", 33: "mp7", 34: "mp9", 35: "nova", 36: "p250", 38: "scar20", 39: "sg556", 40: "ssg08",
    41: "knifegg", 42: "knife", 43: "flashbang", 44: "hegrenade", 45: "smokegrenade", 46: "molotov",
    47: "decoy", 48: "incgrenade", 49: "c4", 57: "healthshot", 59: "knife_t", 60: "m4a1_silencer",
    61: "usp_silencer", 63: "cz75a", 64: "revolver"
  };
  var ICON_BASE = "file://{images}/icons/equipment/";
  // Each icon's width over its height, in eighths (the viewBox of the game's own SVG, CS2 build
  // 11259529). An Image takes no size attribute on a custom HUD, so CSS sizes it: a fixed height and
  // a width class gk-ar-<eighths> (height-percentage). An icon not listed here (a gamemode's own)
  // gets the default 2:1 box.
  var ASPECT = {
    deagle: 13, elite: 17, fiveseven: 10, glock: 11, ak47: 22, aug: 19, awp: 27, famas: 19,
    g3sg1: 23, galilar: 23, m249: 24, m4a1: 20, mac10: 11, p90: 17, mp5sd: 19, ump45: 21,
    xm1014: 24, bizon: 22, mag7: 17, negev: 20, sawedoff: 21, tec9: 13, taser: 10, hkp2000: 8,
    mp7: 12, mp9: 18, nova: 25, p250: 9, scar20: 24, sg556: 22, ssg08: 25, knifegg: 24,
    knife: 19, flashbang: 8, hegrenade: 6, smokegrenade: 4, molotov: 6, decoy: 8, incgrenade: 4, c4: 8,
    healthshot: 11, knife_t: 20, m4a1_silencer: 24, usp_silencer: 17, cz75a: 12, revolver: 13
  };
  var ASPECT_MIN = 3, ASPECT_MAX = 28;

  /** The width class for an icon path, or null for the default box. */
  function aspectClass(path) {
    if (!path || path.indexOf(ICON_BASE) !== 0) return null;
    var n = ASPECT[path.slice(ICON_BASE.length).replace(/\.svg$/, "")];
    return n === undefined ? null : "gk-ar-" + Math.max(ASPECT_MIN, Math.min(ASPECT_MAX, n));
  }

  function itemIcon(def) {
    var n = ICONS[def] || (def >= 500 && def < 600 ? "knife" : null);
    return n ? ICON_BASE + n + ".svg" : null;
  }

  // Names for the slot list, where a row is shared and space is short. The header keeps the full name.
  var SHORT = { 43: "Flash", 44: "HE", 45: "Smoke", 47: "Decoy", 48: "Incendiary", 31: "Zeus", 83: "Frag" };
  // Order inside a shared row: knife before the Zeus, grenades in buy-menu order. Unlisted items keep
  // their inventory order after these.
  var ORDER = { 31: 50, 43: 1, 45: 2, 44: 3, 46: 4, 48: 4, 47: 5 };
  // gear_slot_t: 0 rifle, 1 pistol, 2 knife, 3 grenades, 4 C4.
  var GEAR = [P, S2, K, G, O];

  function itemName(def) {
    var e = ITEMS[def];
    if (e) return e[0];
    return def >= 500 && def < 600 ? "Knife" : "Weapon";
  }

  // ── reads ──────────────────────────────────────────────────────────────────────────────────
  var offs = {};
  function off(cls, field) {
    var k = cls + "." + field;
    var o = offs[k];
    if (o === undefined) {
      o = typeof __s2_schema_offset === "function" ? __s2_schema_offset(cls, field) : -1;
      if (o >= 0) offs[k] = o;
    }
    return o;
  }

  function num(v) { return typeof v === "number" && isFinite(v) ? v : null; }
  function weaponKey(w) { return w && w.ref ? w.ref.index + ":" + w.ref.id : null; }

  function defIndexOf(w) {
    var am = w.attributeManager;
    var item = am && am.item;
    var d = item ? num(item.itemDefinitionIndex) : null;
    return d === null ? -1 : d;
  }

  /** m_pReserveAmmo is an inline int32[2] despite the name; [0] is the primary reserve. */
  function reserveOf(w) {
    var o = off("CBasePlayerWeapon", "m_pReserveAmmo");
    return o < 0 ? null : num(w.ref.readInt32(o));
  }

  /** A grenade's count lives on the owner: m_iAmmo[primaryAmmoType]. 1 when it cannot be read. */
  function grenadeCount(pawn, info) {
    var ws = off("CBasePlayerPawn", "m_pWeaponServices");
    var ammo = off("CPlayer_WeaponServices", "m_iAmmo");
    var t = info.ammoType;
    if (ws < 0 || ammo < 0 || t === null || t < 0 || t >= AMMO_SLOTS || typeof pawn.ref.readUInt16Via !== "function") return 1;
    var n = num(pawn.ref.readUInt16Via([ws], ammo + 2 * t));
    return n !== null && n > 0 ? n : 1;
  }

  /** The body a slot is playing right now, or null while dead, spectating or not in game. */
  function livePawn(slot) {
    var cs2 = globalThis.__s2pkg_cs2;
    var Player = cs2 && cs2.Player;
    if (!Player) return null;
    var p = typeof Player._fromSlotUnchecked === "function" ? Player._fromSlotUnchecked(slot) : Player.fromSlot(slot);
    if (!p) return null;
    var pawn;
    if (p.controllingBot === true) {
      pawn = p.controlledBotPawn;
      var hp = pawn ? num(pawn.health) : null;
      if (hp === null || hp <= 0) return null;
    } else {
      if (p.pawnIsAlive !== true) return null;
      pawn = p.pawn;
    }
    return pawn && pawn.ref && pawn.ref.isValid() ? pawn : null;
  }

  // ── view: what the card shows for one player ────────────────────────────────────────────────
  // { name, clip, reserve, count, tone, slots: { primary..other, active } | null }
  // slots = { active: slot name | null, rows: { <slot>: { items: [{ t: text | null, ic: icon path | null }], on: index | -1, more: n } | null } }
  function rowEqual(a, b) {
    if (a === b) return true;
    if (!a || !b || a.on !== b.on || a.more !== b.more || a.items.length !== b.items.length) return false;
    for (var i = 0; i < a.items.length; i++) {
      if (a.items[i].t !== b.items[i].t || a.items[i].ic !== b.items[i].ic) return false;
    }
    return true;
  }

  /** Fit a row to the layout: at most ROW_ITEMS labels, the item in hand always among them, then "+N". */
  function fitRow(items, on) {
    var more = 0;
    if (items.length > ROW_ITEMS) {
      more = items.length - ROW_ITEMS;
      var shown = items.slice(0, ROW_ITEMS);
      if (on >= ROW_ITEMS) { shown[ROW_ITEMS - 1] = items[on]; on = ROW_ITEMS - 1; }
      items = shown;
    }
    return { items: items, on: on, more: more };
  }

  function text(v) { return v === undefined || v === null ? null : String(v); }
  function count(v) {
    var n = num(v);
    return n === null ? null : Math.max(0, Math.floor(n));
  }

  function makeWeapon(kit) {
    var I = kit._internal;
    var handle = null;              // this plugin's region claim (gamekit claimRegion handle)
    var painted = {};               // slot -> last view painted ({ hidden: true } when hidden)
    var autoAll = false;            // auto("all") or a filter
    var autoFilter = null;
    var autoSlots = {};             // slot -> true (auto for listed players)
    var manual = {};                // slot -> true (set/hide took the player out of auto)
    var opts = { name: null, icon: null, hideSlots: false };
    var state = {};                 // slot -> { polls, activeKey, items, slots }
    var infos = {};                 // weapon key -> { def, cat, ammoType, maxClip, peak }
    var frameSub = null;
    var seen = {};                  // slot -> true once auto has read it (then it waits its turn)
    var frameNo = 0;
    var nameErrors = 0;
    var tickErrors = {};

    function held() { return !!(handle && handle.isValid()); }
    function need(name) {
      if (!held()) throw new Error("GameHud.weapon." + name + ": claim the region first (GameHud.weapon.claim())");
    }

    // ── paint ────────────────────────────────────────────────────────────────────────────────
    function paint(slot, v) {
      var last = painted[slot];
      // After a hide (or a refused drive) the client's state is unknown: send every field.
      var full = !last || last.hidden !== false;
      if (full) last = {};
      var first = null;
      function note(err) { if (!I.ok(err) && first === null) first = err; }

      if (full || v.name !== last.name) {
        var lead = U.leadTone(v.name);
        note(I.swap(slot, "gk_wpn_name", "text", lead ? "gk-t-" + lead : null));
        note(I.setText(slot, "gk_wpn_name", v.name));
      }
      if (full || v.icon !== last.icon) {
        if (v.icon) {
          note(I.swap(slot, "gk_wpn_icon", "aspect", aspectClass(v.icon)));
          note(I.setRaw(slot, "gk_wpn_icon", v.icon));
        }
        note(I.setClass(slot, "gk_wpn_icon", HIDE, !v.icon));
      }
      if (full || v.count !== last.count) {
        if (v.count !== null) note(I.setText(slot, "gk_wpn_count", "\u00d7" + v.count));
        note(I.setClass(slot, "gk_wpn_count", HIDE, v.count === null));
      }
      var hasClip = v.clip !== null;
      if (full || hasClip !== (last.clip !== null)) note(I.setClass(slot, "gk_wpn_ammo", HIDE, !hasClip));
      if (hasClip) {
        if (full || v.clip !== last.clip) note(I.setText(slot, "gk_wpn_clip", v.clip));
        if (full || v.reserve !== last.reserve) {
          if (v.reserve !== null) note(I.setText(slot, "gk_wpn_res", "/ " + v.reserve));
          note(I.setClass(slot, "gk_wpn_res", HIDE, v.reserve === null));
        }
      }
      // Outside the clip branch: a weapon with no clip must still drop the last one's tone, or it
      // would come back on the next weapon's full magazine.
      if (full || v.tone !== last.tone) note(I.swap(slot, "gk_wpn_clip", "text", v.tone ? "gk-t-" + v.tone : null));
      var prevSlots = full ? null : (last.slots || null);
      if (full || !!v.slots !== !!prevSlots) note(I.setClass(slot, "gk_wpn_slots", HIDE, !v.slots));
      if (v.slots) {
        for (var i = 0; i < SLOTS.length; i++) {
          var id = "gk_wpn_s" + i;
          var row = v.slots.rows[SLOTS[i]] || null;
          var had = prevSlots ? (prevSlots.rows[SLOTS[i]] || null) : undefined;   // undefined: unknown
          // A hidden row is never selected, so its highlight is cleared before it hides.
          var sel = !!row && v.slots.active === SLOTS[i];
          var wasSel = prevSlots ? (!!had && prevSlots.active === SLOTS[i]) : undefined;
          if (sel !== wasSel) note(I.setClass(slot, id, "gk-sel", sel));
          if (had !== undefined && rowEqual(row, had)) continue;
          if (!row) { note(I.setClass(slot, id, HIDE, true)); continue; }
          for (var j = 0; j < ROW_ITEMS; j++) {
            var cell = id + "_i" + j;
            var item = j < row.items.length ? row.items[j] : null;
            var hadItem = had ? (j < had.items.length ? had.items[j] : null) : undefined;   // undefined: unknown
            var on = item !== null && row.on === j;
            var wasOn = had ? (hadItem !== null && had.on === j) : undefined;
            if (on !== wasOn) note(I.setClass(slot, cell, "gk-on", on));
            if (hadItem === undefined || (item === null) !== (hadItem === null)) note(I.setClass(slot, cell, HIDE, item === null));
            if (item === null) continue;
            var hadT = hadItem ? hadItem.t : undefined;
            var hadIc = hadItem ? hadItem.ic : undefined;
            if (item.t !== hadT) {
              if (item.t !== null) note(I.setText(slot, cell + "_t", item.t));
              if (hadT === undefined || (item.t === null) !== (hadT === null)) note(I.setClass(slot, cell + "_t", HIDE, item.t === null));
            }
            if (item.ic !== hadIc) {
              if (item.ic !== null) {
                note(I.swap(slot, cell + "_ic", "aspect", aspectClass(item.ic)));
                note(I.setRaw(slot, cell + "_ic", item.ic));
              }
              if (hadIc === undefined || (item.ic === null) !== (hadIc === null)) note(I.setClass(slot, cell + "_ic", HIDE, item.ic === null));
            }
          }
          var hadMore = had ? had.more : undefined;
          if (row.more !== hadMore) {
            if (row.more > 0) note(I.setText(slot, id + "_more", "+" + row.more));
            note(I.setClass(slot, id + "_more", HIDE, row.more <= 0));
          }
          if (!had) note(I.setClass(slot, id, HIDE, false));
        }
      }
      if (full) note(I.show(slot, "gk_wpn"));
      v.hidden = false;
      if (I.ok(first)) painted[slot] = v; else delete painted[slot];
      return first;
    }

    function hideSlot(slot) {
      var last = painted[slot];
      if (last && last.hidden === true) return;
      var err = I.hide(slot, "gk_wpn");
      if (I.ok(err)) painted[slot] = { hidden: true }; else delete painted[slot];
    }

    // ── manual ───────────────────────────────────────────────────────────────────────────────
    // A row is a string (one item, bright when its row is the active one, as before item marks
    // existed) or { items, active? } to mark one item of several. An item is text or { text?, icon? }.
    function normItem(value) {
      if (value === undefined || value === null) return null;
      var t = null;
      var ic = null;
      if (typeof value === "object") {
        t = text(value.text);
        ic = value.icon ? String(value.icon) : null;
      } else {
        t = String(value);
      }
      t = t ? U.plainText(t) : "";
      return t || ic ? { t: t || null, ic: ic } : null;
    }

    function normRow(value, isActiveRow) {
      if (value === undefined || value === null) return null;
      if (typeof value === "object" && Array.isArray(value.items)) {
        var items = [];
        var on = -1;
        for (var i = 0; i < value.items.length; i++) {
          var item = normItem(value.items[i]);
          if (!item) continue;
          if (value.active === i) on = items.length;
          items.push(item);
        }
        return items.length ? fitRow(items, on) : null;
      }
      var one = normItem(value);
      return one ? { items: [one], on: isActiveRow ? 0 : -1, more: 0 } : null;
    }

    function normSlots(spec) {
      if (!spec || typeof spec !== "object") return null;
      var out = { active: SLOTS.indexOf(spec.active) >= 0 ? spec.active : null, rows: {} };
      var any = false;
      for (var i = 0; i < SLOTS.length; i++) {
        var row = normRow(spec[SLOTS[i]], out.active === SLOTS[i]);
        out.rows[SLOTS[i]] = row;
        if (row) any = true;
      }
      return any ? out : null;
    }

    function viewFromSpec(spec) {
      if (!spec || spec.name == null) throw new Error("GameHud.weapon.set: spec.name is required");
      var clip = count(spec.clip);
      return {
        name: String(spec.name),
        icon: spec.icon ? String(spec.icon) : null,
        clip: clip,
        reserve: clip === null ? null : count(spec.reserve),
        count: count(spec.count),
        tone: U.isTone(spec.tone) ? spec.tone : null,
        slots: normSlots(spec.slots)
      };
    }

    // ── auto ─────────────────────────────────────────────────────────────────────────────────
    function infoFor(w, key) {
      var info = infos[key];
      if (info) return info;
      var def = defIndexOf(w);
      var data = w.data || null;
      var entry = ITEMS[def];
      var gear = data ? num(data.gearSlot) : null;
      var cat = entry ? entry[1] : (def >= 500 && def < 600 ? K : (gear !== null && GEAR[gear] ? GEAR[gear] : O));
      info = infos[key] = {
        def: def,
        cat: cat,
        ammoType: data ? num(data.primaryAmmoType) : null,
        maxClip: data ? num(data.maxClip1) : null,
        peak: 0
      };
      return info;
    }

    /** The gamemode's name for an item (options.name), or null to use the default. */
    function customName(w, info) {
      if (typeof opts.name !== "function") return null;
      try {
        var custom = opts.name(info.def, w);
        if (custom !== null && custom !== undefined) return String(custom);
      } catch (e) {
        if (nameErrors++ < 5) U.log("weapon.auto name() threw: " + ((e && e.message) || e));
      }
      return null;
    }

    /** The item's icon: the gamemode's (options.icon), else the game's own for that item. */
    function iconFor(w, info) {
      if (typeof opts.icon === "function") {
        try {
          var custom = opts.icon(info.def, w);
          if (custom !== null && custom !== undefined) return custom === "" ? null : String(custom);
        } catch (e) {
          if (nameErrors++ < 5) U.log("weapon.auto icon() threw: " + ((e && e.message) || e));
        }
      }
      return itemIcon(info.def);
    }

    function nameFor(w, info) {
      var custom = customName(w, info);
      return custom !== null ? custom : itemName(info.def);
    }

    function readInventory(pawn, st) {
      var list = pawn.weapons || [];
      var items = [];
      for (var i = 0; i < list.length && i < MAX_WEAPONS; i++) {
        var w = list[i];
        var key = weaponKey(w);
        if (!key || !w.ref.isValid()) continue;
        var info = infoFor(w, key);
        var custom = customName(w, info);
        items.push({
          key: key, weapon: w, info: info, custom: custom, icon: iconFor(w, info),
          name: custom !== null ? custom : itemName(info.def)
        });
      }
      st.items = items;
    }

    /** The held grenades' counts as one string, to tell when the slot list's "×N" moved. */
    function grenadeSignature(pawn, st) {
      var sig = "";
      for (var i = 0; i < st.items.length; i++) {
        if (st.items[i].info.cat === G) sig += grenadeCount(pawn, st.items[i].info) + ",";
      }
      return sig;
    }

    function slotList(pawn, st, activeCat, activeKey) {
      var out = { active: activeCat, rows: {} };
      var byCat = {};
      for (var i = 0; i < st.items.length; i++) {
        var it = st.items[i];
        // The icon stands for the item. Text joins it for a renamed item, an item with no icon,
        // and a grenade count.
        var label = it.custom !== null ? U.plainText(it.custom) : (it.icon ? "" : (SHORT[it.info.def] || it.name));
        if (it.info.cat === G) {
          var n = grenadeCount(pawn, it.info);
          if (n > 1) label += (label ? " " : "") + "\u00d7" + n;
        }
        var rank = ORDER[it.info.def];
        (byCat[it.info.cat] || (byCat[it.info.cat] = [])).push({
          label: { t: label || null, ic: it.icon }, key: it.key, rank: rank === undefined ? (it.info.cat === K ? 0 : 99) : rank, at: i
        });
      }
      var any = false;
      for (var c = 0; c < SLOTS.length; c++) {
        var list = byCat[SLOTS[c]];
        if (!list) { out.rows[SLOTS[c]] = null; continue; }
        list.sort(function (a, b) { return a.rank - b.rank || a.at - b.at; });
        var labels = [];
        var on = -1;
        for (var k = 0; k < list.length; k++) {
          labels.push(list[k].label);
          // The entity in hand, not its name: two items with one name stay distinguishable.
          if (list[k].key === activeKey) on = k;
        }
        out.rows[SLOTS[c]] = fitRow(labels, on);
        any = true;
      }
      return any ? out : null;
    }

    function autoView(slot) {
      var pawn = livePawn(slot);
      if (!pawn) { delete state[slot]; return null; }
      var st = state[slot] || (state[slot] = { polls: 0, activeKey: null, items: null, slots: null, nades: "" });
      var w = pawn.activeWeapon;
      var key = weaponKey(w);
      if (key && !w.ref.isValid()) { w = null; key = null; }
      var refresh = !st.items || key !== st.activeKey || (st.polls++ % INVENTORY_POLLS) === 0;
      st.activeKey = key;
      if (refresh) readInventory(pawn, st);
      if (!w) return null;

      var info = infoFor(w, key);
      var name = null;
      var icon = null;
      for (var i = 0; i < st.items.length; i++) {
        if (st.items[i].key === key) { name = st.items[i].name; icon = st.items[i].icon; break; }
      }
      if (name === null) { name = nameFor(w, info); icon = iconFor(w, info); }

      var v = { name: name, icon: icon, clip: null, reserve: null, count: null, tone: null, slots: null };
      if (info.cat === G) {
        v.count = grenadeCount(pawn, info);
      } else if (info.cat !== K || info.def === 31) {
        // Knives (and C4, the medi-shot...) have no clip and show only the name. The Zeus is in the
        // knife slot but fires, so it keeps its clip.
        var clip = num(w.clip1);
        if (clip !== null && clip >= 0) {
          v.clip = clip;
          var res = reserveOf(w);
          v.reserve = res !== null && res >= 0 ? res : null;
          if (clip > info.peak) info.peak = clip;
          var max = info.maxClip !== null && info.maxClip > 0 ? info.maxClip : info.peak;
          v.tone = clip === 0 || (max > 0 && clip <= max * LOW_FRACTION) ? "red" : null;
        }
      }
      // The list changes with the inventory, the item in hand and the grenade counts (a throw or
      // a pickup moves m_iAmmo without touching the inventory), so it is rebuilt with them and the
      // same object is painted otherwise (paint's row compare stops at identity).
      if (!opts.hideSlots) {
        var nades = grenadeSignature(pawn, st);
        if (refresh || !st.slots || nades !== st.nades) st.slots = slotList(pawn, st, info.cat, key);
        st.nades = nades;
        v.slots = st.slots;
      }
      return v;
    }

    function onAuto(slot) {
      if (manual[slot]) return false;
      if (autoSlots[slot]) return true;
      if (!autoAll) return false;
      if (!autoFilter) return true;
      try { return !!autoFilter(slot); } catch (e) { return false; }
    }

    function anyAuto() {
      if (autoAll) return true;
      for (var s in autoSlots) if (Object.prototype.hasOwnProperty.call(autoSlots, s)) return true;
      return false;
    }

    function frame() {
      if (!held()) { stopFrames(); return; }
      var turn = frameNo++ % POLL_EVERY;
      var slots = U.activeSlots();
      for (var i = 0; i < slots.length; i++) {
        var s = slots[i];
        // A player's turn comes once every POLL_EVERY frames; one not seen yet is read at once.
        if (s % POLL_EVERY !== turn && seen[s]) continue;
        if (!onAuto(s)) continue;
        seen[s] = true;
        try {
          var v = autoView(s);
          if (v) paint(s, v); else hideSlot(s);
        } catch (e) {
          var msg = String((e && e.message) || e);
          if (!tickErrors[msg]) { tickErrors[msg] = true; U.log("weapon.auto slot " + s + ": " + msg); }
        }
      }
    }

    function startFrames() {
      if (frameSub) return;
      var F = globalThis.__s2pkg_frame && globalThis.__s2pkg_frame.OnGameFrame;
      if (!F || typeof F.subscribe !== "function") throw new Error("GameHud.weapon.auto: OnGameFrame is unavailable");
      frameSub = F.subscribe(frame);
    }

    function stopFrames() {
      if (frameSub && typeof frameSub.dispose === "function") frameSub.dispose();
      frameSub = null;
    }

    function resetAuto() {
      stopFrames();
      autoAll = false;
      autoFilter = null;
      autoSlots = {};
      manual = {};
      state = {};
      infos = {};
      painted = {};
      seen = {};
    }

    /** Take an audience out of auto (set/hide). With "all", auto stops for everyone. */
    function takeManual(audience) {
      if (audience === undefined || audience === null || audience === "all") {
        autoAll = false; autoFilter = null; autoSlots = {};
        if (!anyAuto()) stopFrames();
        return U.activeSlots();
      }
      var slots = U.slotsFor(audience);
      for (var i = 0; i < slots.length; i++) { manual[slots[i]] = true; delete autoSlots[slots[i]]; }
      if (!anyAuto()) stopFrames();
      return slots;
    }

    var api = {
      claim: function () {
        if (held()) return true;
        handle = I.claimRegion("weapon", "gk_wpn", function () { return {}; });
        if (!handle) return false;
        handle._onRelease = function () { resetAuto(); };
        painted = {};
        return true;
      },
      isClaimed: function () { return held(); },
      release: function () {
        if (!held()) return;
        handle.release();
        handle = null;
      },
      auto: function (audience, options) {
        need("auto");
        options = options || {};
        if (options.name !== undefined && options.name !== null && typeof options.name !== "function") {
          throw new Error("GameHud.weapon.auto: options.name must be a function");
        }
        if (options.icon !== undefined && options.icon !== null && typeof options.icon !== "function") {
          throw new Error("GameHud.weapon.auto: options.icon must be a function");
        }
        opts = { name: options.name || null, icon: options.icon || null, hideSlots: !!options.hideSlots };
        // New options: every player's next read rebuilds the names, icons and slot list.
        for (var s in state) if (Object.prototype.hasOwnProperty.call(state, s)) state[s].items = null;
        if (audience === undefined || audience === null || audience === "all" || typeof audience === "function") {
          autoAll = true;
          autoFilter = typeof audience === "function" ? audience : null;
          manual = {};
        } else {
          var slots = U.slotsFor(audience);
          for (var i = 0; i < slots.length; i++) { autoSlots[slots[i]] = true; delete manual[slots[i]]; }
        }
        startFrames();
      },
      set: function (audience, spec) {
        need("set");
        var v = viewFromSpec(spec);
        var slots = takeManual(audience);
        for (var i = 0; i < slots.length; i++) {
          var copy = { name: v.name, icon: v.icon, clip: v.clip, reserve: v.reserve, count: v.count, tone: v.tone, slots: v.slots };
          paint(slots[i], copy);
        }
      },
      hide: function (audience) {
        need("hide");
        var slots = takeManual(audience);
        for (var i = 0; i < slots.length; i++) hideSlot(slots[i]);
      },
      _forgetSlot: function (slot) {
        delete painted[slot]; delete state[slot]; delete autoSlots[slot]; delete manual[slot]; delete seen[slot];
        if (!anyAuto()) stopFrames();
      },
      _resetForMap: function () { painted = {}; state = {}; infos = {}; seen = {}; },
      _frame: frame,
      _frameNo: function () { return frameNo; }
    };
    return api;
  }

  // ── ctx binding ─────────────────────────────────────────────────────────────────────────────
  var liveWeapon = null;

  globalThis.__s2pkg_game_ctx = Object.assign({}, globalThis.__s2pkg_game_ctx, {
    ui: function (reg, viaId) {
      var base = prevUi(reg, viaId);
      var kit = base && base.gameHud;
      if (kit && kit._internal && !kit.weapon) {
        var weapon = makeWeapon(kit);
        kit.weapon = weapon;
        if (!liveWeapon) liveWeapon = weapon;
        reg(viaId(function () {
          var Clients = U.clientsApi();
          if (Clients && typeof Clients.onDisconnect === "function") {
            return Clients.onDisconnect(function (c) { if (c && typeof c.slot === "number") weapon._forgetSlot(c.slot); });
          }
        }));
        reg(viaId(function () {
          var Server = globalThis.__s2pkg_server && globalThis.__s2pkg_server.Server;
          if (Server && typeof Server.onMapStart === "function") return Server.onMapStart(function () { weapon._resetForMap(); });
        }));
      }
      return base;
    }
  });

  function weaponOrThrow(name) {
    if (!liveWeapon) throw new Error("s2script: GameHud.weapon." + name + " requires plugin context; use it in OnPluginStart or a later callback");
    return liveWeapon;
  }

  var cs2 = globalThis.__s2pkg_cs2;
  if (cs2 && cs2.GameHud) {
    var merged = {};
    for (var k in cs2.GameHud) if (Object.prototype.hasOwnProperty.call(cs2.GameHud, k)) merged[k] = cs2.GameHud[k];
    // The plugin is the calling context; claim()'s argument is accepted and not needed.
    merged.weapon = Object.freeze({
      claim: function () { return weaponOrThrow("claim").claim(); },
      isClaimed: function () { return liveWeapon ? liveWeapon.isClaimed() : false; },
      release: function () { return weaponOrThrow("release").release(); },
      auto: function (audience, options) { return weaponOrThrow("auto").auto(audience, options); },
      set: function (audience, spec) { return weaponOrThrow("set").set(audience, spec); },
      hide: function (audience) { return weaponOrThrow("hide").hide(audience); }
    });
    cs2.GameHud = Object.freeze(merged);
  }
})();
