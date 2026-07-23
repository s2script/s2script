import { plugin } from "@s2script/sdk/plugin";
import { label } from "@fixture/util";

export default plugin((ctx) => {
  ctx.commands.register("fixture_ws", (cmd) => { cmd.reply(label()); });
});
