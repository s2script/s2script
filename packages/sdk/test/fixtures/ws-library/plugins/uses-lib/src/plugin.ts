import { plugin } from "@s2script/sdk/plugin";
import { greet } from "@fixture/greetlib";

export default plugin((ctx) => {
  ctx.commands.register("ws_library_test", (cmd) => {
    cmd.reply(greet("world"));
  });
});
