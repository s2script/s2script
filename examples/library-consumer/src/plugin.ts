// Consumes @example/base64 the way `s2s add` sets a plugin up to: a vendored copy at
// .s2script/libs/@example/base64/, committed in this repo so the example builds offline with no
// registry involved (see README.md for why, and examples/library-package for the producer side).
//
// `encode`/`decode` are BUNDLED here, not loaded at runtime — esbuild inlines the library's code
// straight into this plugin's plugin.js at `s2s build` time. There is no `require("@example/
// base64")` anywhere in the output for the host to resolve; by the time this plugin loads, the
// codec is just more of this file.
import { encode, decode } from "@example/base64";
import { command } from "@s2script/sdk/commands";

export function OnPluginStart(): void {
  command("sm_b64", (cmd) => {
    const input = cmd.argsFrom(0);
    cmd.reply(input ? `${input} -> ${encode(input)}` : "usage: sm_b64 <text>");
  });

  command("sm_unb64", (cmd) => {
    const input = cmd.argsFrom(0);
    try {
      cmd.reply(decode(input));
    } catch (e) {
      cmd.reply(`not base64: ${(e as Error).message}`);
    }
  });
}
