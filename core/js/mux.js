// The JS multiplexer (core/src/jsmux.rs). Evaluated once in its own context, which holds no
// plugin code and no natives beyond the two passed in here, and returns the functions core calls.
//
// One Rust->JS crossing per event replaces a crossing per subscriber. Every guarantee of the Rust
// fan-out (core/src/dispatch.rs) still holds, and each has its home stated beside the code below.
//
//   enter(slot, generation, line) -> 1 | 0
//                                      Liveness by registry slot. On 1 it also names the plugin in
//                                      the crash breadcrumb (with the handler's source line when
//                                      `line` > 0) and records it as the dispatch owner.
//   thrown(slot, message, stack)       A handler threw: WARN and, where the dispatch reports, file
//                                      it with the crash reporter.
//   range(n)                           An OnGameFrame handler returned a number outside 0..3.
(function (enter, thrown, range) {
  "use strict";

  // kind -> (name -> subscriber array). Core replaces a whole array whenever its channel changed,
  // so a dispatch walks the array it started with: subscribing or unsubscribing from inside a
  // handler cannot change who this event reaches. That is today's snapshot-before-invoke.
  var kinds = [];
  function channels(kind) {
    var m = kinds[kind];
    if (m === undefined) kinds[kind] = m = new Map();
    return m;
  }

  // rows: [slot, generation, handler, make, id, monitor, line] per subscriber, in dispatch order.
  // `make` builds the subscriber's payload in its own realm (a GameEvent class, or the frame ctx
  // factory), or is null for channels whose subscribers share plain arguments.
  function set(kind, name, rows) {
    var subs = new Array(rows.length);
    for (var i = 0; i < rows.length; i++) {
      var r = rows[i];
      subs[i] = { slot: r[0], generation: r[1], handler: r[2], ctor: r[3], id: r[4], monitor: r[5], line: r[6] };
    }
    channels(kind).set(name, subs);
  }

  function drop(kind, names) {
    for (var i = 0; i < names.length; i++) channels(kind).delete(names[i]);
  }

  function describe(e) {
    try {
      return String(e);
    } catch (_) {
      return "handler threw";
    }
  }

  function stackOf(e) {
    try {
      var s = e !== null && typeof e === "object" ? e.stack : undefined;
      return typeof s === "string" ? s : "";
    } catch (_) {
      return "";
    }
  }

  // stopAt: 0 = never (notify: return values are not read), 1 = truncate after a Stop (3),
  // 2 = truncate after Handled (2) or more. Results collapse to their maximum, as run_chain does.
  // A channel with payload classes (game events) hands each subscriber `new ctor(name)`; any other
  // channel hands every subscriber the same `argc` arguments, exactly that many.
  function dispatch(kind, name, stopAt, argc, a0, a1, a2) {
    var subs = channels(kind).get(name);
    if (subs === undefined) return 0;
    var result = 0;
    for (var i = 0; i < subs.length; i++) {
      if (stopAt === 1 ? result === 3 : stopAt === 2 && result >= 2) break;
      var s = subs[i];
      if (enter(s.slot, s.generation, 0) === 0) continue;
      var ret;
      try {
        // Called through a local so the receiver is `undefined`, never the subscriber record.
        var handler = s.handler;
        if (s.ctor !== null) {
          // A game event is built in the subscriber's own realm, fresh for each subscriber, as the
          // Rust fan-out built it. A constructor that throws still delivers, with `undefined`.
          var ev = undefined;
          try {
            ev = new s.ctor(name);
          } catch (_) {
            ev = undefined;
          }
          ret = handler(ev);
        } else if (argc === 2) {
          ret = handler(a0, a1);
        } else if (argc === 3) {
          ret = handler(a0, a1, a2);
        } else if (argc === 1) {
          ret = handler(a0);
        } else {
          ret = handler();
        }
      } catch (e) {
        thrown(s.slot, describe(e), stackOf(e));
        continue;
      }
      if (stopAt !== 0 && typeof ret === "number") {
        // ToUint32, then 1..3; anything else is Continue, so a stray number can never truncate.
        var r = ret >>> 0;
        if (r > 3) r = 0;
        if (r > result) result = r;
      }
    }
    return result;
  }

  // OnGameFrame's chain (multiplexer::run_chain): priority order with Monitor last; after a Stop
  // only Monitor handlers run; a Monitor's return is ignored; `undefined` is Continue; any other
  // return is ToUint32'd and a value above 3 WARNs and counts as Continue. Returns the collapsed
  // result, or `[result, ...ids]` naming the handlers that threw, for auto-disable.
  function frame(kind, name, simulating, firstTick, lastTick) {
    var subs = channels(kind).get(name);
    if (subs === undefined) return 0;
    var result = 0, stopped = false, errored = null;
    for (var i = 0; i < subs.length; i++) {
      var s = subs[i];
      if (stopped && !s.monitor) continue;
      // No ctx factory means the owner has no context: skip it as Continue, as run_chain did.
      if (s.ctor === null) continue;
      if (enter(s.slot, s.generation, s.line) === 0) continue;
      var ret;
      try {
        var make = s.ctor, handler = s.handler;
        ret = handler(make(simulating, firstTick, lastTick, name));
      } catch (e) {
        thrown(s.slot, describe(e), stackOf(e));
        if (errored === null) errored = [];
        errored.push(s.id);
        continue;
      }
      var r = 0;
      if (ret !== undefined) {
        try {
          r = ret >>> 0;
        } catch (_) {
          r = 0;
        }
        if (r > 3) {
          range(r);
          r = 0;
        }
      }
      if (s.monitor) continue;
      if (r > result) result = r;
      if (r === 3) stopped = true;
    }
    return errored === null ? result : [result].concat(errored);
  }

  return { set: set, drop: drop, dispatch: dispatch, frame: frame };
});
