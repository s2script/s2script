// extension-fastmath — a plugin that imports a native extension.
//
// A native extension is a shared library in addons/s2script/extensions/ that adds natives without
// a core change (see native/fastmath.c and packages/sdk/ext/s2ext.h). A plugin declares the ones
// it needs under `s2script.extensions` in package.json, and imports them like any module:
//
//   - the server refuses to load this plugin, with the reason, if @community/fastmath is missing
//     or not ^0.1.0;
//   - the types come from .s2script/extensions/@community/fastmath/index.d.ts, which is what
//     `sm ext types @community/fastmath` prints on a server that has the extension installed.
//
// Build the extension:  cc -shared -fPIC -O2 -I../../packages/sdk/ext -o fastmath.so native/fastmath.c
// Build the plugin:     npx @s2script/sdk build examples/extension-fastmath
import { command, HookResult } from "@s2script/sdk";
import { clamp, lerp, label, parseNumber } from "@community/fastmath";

export function OnPluginStart(): void {
  // sm_lerp <a> <b> <t>: numbers only, so optimised code calls the C function directly.
  command("sm_lerp", (cmd) => {
    const t = clamp(Math.round(cmd.argFloat(3, 0) * 100), 0, 100) / 100;
    cmd.reply(`lerp = ${lerp(cmd.argFloat(1, 0), cmd.argFloat(2, 1), t)}`);
    return HookResult.Handled;
  });

  // sm_parse <text>: a native that can fail throws an S2Error carrying the extension's code.
  command("sm_parse", (cmd) => {
    try {
      cmd.reply(label(1, parseNumber(cmd.arg(1)), "units") ?? "");
    } catch (e) {
      cmd.reply(`not a number (${(e as { code?: string }).code})`);
    }
    return HookResult.Handled;
  });
}
