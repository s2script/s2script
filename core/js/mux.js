// The JS multiplexer (core/src/jsmux.rs). Evaluated once in its own context, which holds no
// plugin code and no natives beyond the two passed in here, and returns the functions core calls.
//
// One Rust->JS crossing per event replaces a crossing per subscriber. Every guarantee of the Rust
// fan-out (core/src/dispatch.rs) still holds, and each has its home stated beside the code below.
//
//   enter(slot, generation) -> 1 | 0   Liveness by registry slot. On 1 it also names the plugin in
//                                      the crash breadcrumb and records it as the dispatch owner.
//   thrown(slot, generation, message, stack)
//                                      A handler threw: WARN and, where the dispatch reports, file
//                                      it with the crash reporter.
(function (enter, thrown) {
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

  function set(kind, name, slots, generations, handlers, ctors) {
    var subs = new Array(handlers.length);
    for (var i = 0; i < handlers.length; i++) {
      subs[i] = { slot: slots[i], generation: generations[i], handler: handlers[i], ctor: ctors[i] };
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
      if (enter(s.slot, s.generation) === 0) continue;
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
        thrown(s.slot, s.generation, describe(e), stackOf(e));
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

  return { set: set, drop: drop, dispatch: dispatch };
});
