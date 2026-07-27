import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "@fixture/ws-greeter";

export default plugin((ctx) => {
  const g = ctx.use<Greeter>("@fixture/ws-greeter");
  ctx.commands.register("ws_stale_greet", (cmd) => {
    // A string argument: valid against the SIBLING's contract, TS2345 against the stale copy.
    cmd.reply(g.greet("world"));
  });
});
