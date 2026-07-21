import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.commands.register("late", (cmd) => {
    ctx.events.on("player_death", () => {
      cmd.reply("someone died");
    });
  });
});
