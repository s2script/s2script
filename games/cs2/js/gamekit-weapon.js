// @s2script/cs2 — GameHud.weapon: the weapon card, a replacement for the stock weapons HUD.
//
// A gamemode that hides the stock weapons HUD (`Hud.hide(slot, HideHud.Weapons)`, m_iHideHUD bit 6)
// loses the ammo count with it. This region of s2script_gamehud.xml (bottom right) puts it back:
// the active weapon's name, clip / reserve in large text, a low-ammo tone, and a compact slot list.
//
// ONE OWNER. The card is a region of the shared in-round layout, claimed like the rail and status
// card (`__s2_ui_pool_claim("gamekit:weapon", 1)`, ledgered, freed on unload). `claim()` answers
// true or false instead of returning a handle, and the other calls throw until this plugin holds it.
//
// AUTO. `auto(audience, options)` fills the card from the pawn on every game frame: the active
// weapon (m_hActiveWeapon) and its m_iClip1 / m_pReserveAmmo[0] each frame, the held weapons
// (m_hMyWeapons) every INVENTORY_EVERY frames and whenever the active weapon changes. A paint only
// sends what changed, so a frame where nothing moved costs reads and no HUD traffic. The card hides
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
  var LOW_FRACTION = 0.25;
  var INVENTORY_EVERY = 8;   // frames between inventory reads (about 8 Hz at 64 tick)
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
  function slotsEqual(a, b) {
    if (a === b) return true;
    if (!a || !b) return false;
    if (a.active !== b.active) return false;
    for (var i = 0; i < SLOTS.length; i++) if ((a[SLOTS[i]] || "") !== (b[SLOTS[i]] || "")) return false;
    return true;
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
    var opts = { name: null, hideSlots: false };
    var state = {};                 // slot -> { frame, activeKey, items }
    var infos = {};                 // weapon key -> { def, cat, ammoType, maxClip, peak }
    var frameSub = null;
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
        if (full || v.tone !== last.tone) note(I.swap(slot, "gk_wpn_clip", "text", v.tone ? "gk-t-" + v.tone : null));
      }
      if (full || !slotsEqual(v.slots, last.slots)) {
        note(I.setClass(slot, "gk_wpn_slots", HIDE, !v.slots));
        if (v.slots) {
          var prev = full ? null : last.slots;
          for (var i = 0; i < SLOTS.length; i++) {
            var row = "gk_wpn_s" + i;
            var label = v.slots[SLOTS[i]];
            var had = prev ? prev[SLOTS[i]] : undefined;
            var wasSel = prev ? prev.active === SLOTS[i] : undefined;
            var sel = v.slots.active === SLOTS[i];
            if (!label) { if (!prev || had) note(I.setClass(slot, row, HIDE, true)); continue; }
            if (label !== had) note(I.setText(slot, row + "_t", label));
            if (!prev || !had) note(I.setClass(slot, row, HIDE, false));
            if (sel !== wasSel) note(I.setClass(slot, row, "gk-sel", sel));
          }
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
    function normSlots(spec) {
      if (!spec || typeof spec !== "object") return null;
      var out = { active: SLOTS.indexOf(spec.active) >= 0 ? spec.active : null };
      var any = false;
      for (var i = 0; i < SLOTS.length; i++) {
        var t = text(spec[SLOTS[i]]);
        out[SLOTS[i]] = t ? U.plainText(t) : null;
        if (out[SLOTS[i]]) any = true;
      }
      return any ? out : null;
    }

    function viewFromSpec(spec) {
      if (!spec || spec.name == null) throw new Error("GameHud.weapon.set: spec.name is required");
      var clip = count(spec.clip);
      return {
        name: String(spec.name),
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

    function nameFor(w, info) {
      if (typeof opts.name === "function") {
        try {
          var custom = opts.name(info.def, w);
          if (custom !== null && custom !== undefined) return String(custom);
        } catch (e) {
          if (nameErrors++ < 5) U.log("weapon.auto name() threw: " + ((e && e.message) || e));
        }
      }
      return itemName(info.def);
    }

    function readInventory(pawn, st) {
      var list = pawn.weapons || [];
      var items = [];
      for (var i = 0; i < list.length && i < MAX_WEAPONS; i++) {
        var w = list[i];
        var key = weaponKey(w);
        if (!key || !w.ref.isValid()) continue;
        var info = infoFor(w, key);
        items.push({ key: key, weapon: w, info: info, name: nameFor(w, info) });
      }
      st.items = items;
    }

    function slotList(pawn, st, activeCat) {
      var out = { active: activeCat };
      var byCat = {};
      for (var i = 0; i < st.items.length; i++) {
        var it = st.items[i];
        var label = U.plainText(it.name);
        if (it.info.cat === G) {
          var n = grenadeCount(pawn, it.info);
          if (n > 1) label += " ×" + n;
        }
        (byCat[it.info.cat] || (byCat[it.info.cat] = [])).push(label);
      }
      var any = false;
      for (var c = 0; c < SLOTS.length; c++) {
        var names = byCat[SLOTS[c]];
        out[SLOTS[c]] = names ? names.join(" · ") : null;
        if (names) any = true;
      }
      return any ? out : null;
    }

    function autoView(slot) {
      var pawn = livePawn(slot);
      if (!pawn) { delete state[slot]; return null; }
      var st = state[slot] || (state[slot] = { frame: 0, activeKey: null, items: null });
      var w = pawn.activeWeapon;
      var key = weaponKey(w);
      if (key && !w.ref.isValid()) { w = null; key = null; }
      var refresh = !st.items || key !== st.activeKey || (st.frame++ % INVENTORY_EVERY) === 0;
      st.activeKey = key;
      if (refresh) readInventory(pawn, st);
      if (!w) return null;

      var info = infoFor(w, key);
      var name = null;
      for (var i = 0; i < st.items.length; i++) if (st.items[i].key === key) { name = st.items[i].name; break; }
      if (name === null) name = nameFor(w, info);

      var v = { name: name, clip: null, reserve: null, count: null, tone: null, slots: null };
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
      if (!opts.hideSlots) v.slots = slotList(pawn, st, info.cat);
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
      frameNo++;
      var slots = U.activeSlots();
      for (var i = 0; i < slots.length; i++) {
        var s = slots[i];
        if (!onAuto(s)) continue;
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
        var nameChanged = opts.name !== (options.name || null);
        opts = { name: options.name || null, hideSlots: !!options.hideSlots };
        if (nameChanged) { for (var s in state) if (Object.prototype.hasOwnProperty.call(state, s)) state[s].items = null; }
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
          var copy = { name: v.name, clip: v.clip, reserve: v.reserve, count: v.count, tone: v.tone, slots: v.slots };
          paint(slots[i], copy);
        }
      },
      hide: function (audience) {
        need("hide");
        var slots = takeManual(audience);
        for (var i = 0; i < slots.length; i++) hideSlot(slots[i]);
      },
      _forgetSlot: function (slot) {
        delete painted[slot]; delete state[slot]; delete autoSlots[slot]; delete manual[slot];
        if (!anyAuto()) stopFrames();
      },
      _resetForMap: function () { painted = {}; state = {}; infos = {}; },
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
