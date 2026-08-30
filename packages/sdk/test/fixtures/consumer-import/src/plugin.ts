import { plugin } from "@s2script/sdk/plugin";
import { command } from "@s2script/sdk/commands";
import { greet } from "@demo/greeter";

/** Producer-as-import (B): the host proxy is `require("@demo/greeter")`, not ctx.use. */
export default plugin(() => {
  command("greet_me", (cmd) => {
    cmd.reply(greet("world"));
  });
});
