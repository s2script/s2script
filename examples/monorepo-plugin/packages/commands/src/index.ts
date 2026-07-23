import type { PluginContext } from "@s2script/sdk/plugin";
import { GreetingLog } from "@monorepo-example/core";

/**
 * A feature package: it receives the plugin context and the shared store, and
 * owns one slice of behaviour. Feature packages import @monorepo-example/core
 * — never each other — so the dependency graph stays a tree.
 */
export function registerCommands(ctx: PluginContext, log: GreetingLog): void {
  let frames = 0;
  ctx.server.onGameFrame(() => { frames += 1; });

  ctx.commands.register("sm_greet", (cmd) => {
    log.add("hello from a workspace package", frames);
    cmd.reply(`logged greeting #${log.count}`);
  });

  ctx.commands.register("sm_latest", (cmd) => {
    const latest = log.latest();
    cmd.reply(latest ? `#${log.count} "${latest.text}" at frame ${latest.at}` : "nothing logged yet");
  });
}
