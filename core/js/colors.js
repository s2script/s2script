// Color-tag expansion. ENGINE-GENERIC: this file never names a colour or a game — each game
// package defines its own colour codes and hands the {name} -> control-byte table over at
// runtime via setTable(). Core only ever does a map lookup.
//
// Concatenated AHEAD of prelude.js by the include_str!/concat! in core/src/v8host.rs, so
// globalThis.__s2_colors exists before prelude.js runs. Dual-mode export (see
// games/cs2/js/activity.js) so the pure logic is node:test-able without a V8 context.
(function () {
  var table = Object.create(null);
  var warned = Object.create(null);

  function setTable(obj) {
    table = Object.create(null);
    if (!obj || typeof obj !== "object") return;          // null/garbage -> empty table, tags just vanish
    for (var k in obj) {
      if (!Object.prototype.hasOwnProperty.call(obj, k) || k === "__proto__") continue;
      if (typeof obj[k] === "string") table[String(k).toLowerCase()] = obj[k];
    }
  }

  // {name} where name is LETTERS ONLY, so positional {1}/{2} slots can never collide with a
  // colour tag. An unrecognised tag is DELETED (players must never see stray braces) and warned
  // ONCE per distinct name, so a typo is diagnosable from the server console without spamming it.
  function expand(text) {
    return String(text).replace(/\{([A-Za-z]+)\}/g, function (_m, name) {
      var key = name.toLowerCase();
      var v = table[key];
      if (typeof v === "string") return v;
      if (!warned[key]) {
        warned[key] = true;
        console.log("[s2script] WARN: unknown colour tag {" + name + "} — removed from output");
      }
      return "";
    });
  }

  var ZWSP = "\u200B";
  // Expand BEFORE the lead-byte test: a line written "{green}hi" must present as "\x04hi" when we
  // decide, or we would read "{" , skip the ZWSP, and the chat box would swallow the colour byte.
  function chatLine(prefix, msg) {
    var body = expand(String(prefix) + String(msg));
    var lead = body.charAt(0);
    return (lead === ZWSP || lead === " ") ? body : ZWSP + body;
  }

  // Console output: expand first (an unexpanded "{green}" is not a control byte and would survive
  // the strip as literal text), then drop every control byte — colour bytes included.
  function consoleLine(text) {
    return expand(text).replace(/[\x00-\x1F\x7F]/g, "");
  }

  var api = {
    setTable: setTable, expand: expand, chatLine: chatLine, consoleLine: consoleLine,
    _resetWarnings: function () { warned = Object.create(null); },
  };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  if (typeof globalThis !== "undefined") globalThis.__s2_colors = api;
})();
