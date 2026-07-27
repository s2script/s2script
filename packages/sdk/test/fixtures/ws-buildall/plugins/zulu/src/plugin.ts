import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("ws_zulu", (cmd) => {
    cmd.reply("zulu");
  });
});
