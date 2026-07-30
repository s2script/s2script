import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("ws_filter_keep", (cmd) => {
    cmd.reply("keep");
  });
});
