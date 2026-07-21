import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "@demo/greeter";

export default plugin((ctx) => {
  const g = ctx.use<Greeter>("@demo/greeter");
  ctx.commands.register("greet_me", (cmd) => {
    cmd.reply(g.greet("world"));
  });
});
