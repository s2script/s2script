import { plugin } from "@s2script/sdk/plugin";
// No `.s2script/types/` copy anywhere: this resolves through npm's workspace symlink to the
// sibling's own api.d.ts (design spec 2026-07-27 §3.2).
import type { Greeter } from "@fixture/ws-greeter";

export default plugin((ctx) => {
  const g = ctx.use<Greeter>("@fixture/ws-greeter");
  ctx.commands.register("ws_greet", (cmd) => {
    cmd.reply(g.greet("world"));
  });
});
