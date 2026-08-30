/**
 * Demo plugin fixture — imports @s2script/sdk/plugin and @s2script/sdk/timers so esbuild emits
 * require("@s2script/sdk/plugin") / require("@s2script/sdk/timers") in the CJS bundle (external).
 */
import { delay } from "@s2script/sdk/timers";
import { hook } from "@s2script/sdk/plugin";

export function OnPluginStart(): void {
  hook.server.onGameFrame(() => {
    console.log("frame tick");
  });
  void (async () => {
    await delay(100);
    console.log("hello from @demo/hello");
  })();
}
