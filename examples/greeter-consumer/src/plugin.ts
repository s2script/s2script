import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "../../greeter-plugin/api";

// Consumer: hard-deps @demo/greeter (a proxy that throws InterfaceUnavailable while the
// producer is unloaded), subscribes to the producer's forwarded `greeted` event, and
// calls greet(0) every ~256 frames. The greet call is wrapped in try/catch so a
// producer-absent throw degrades gracefully (logs + keeps ticking) instead of crashing.
export default plugin((ctx) => {
  console.log("[consumer] onLoad");
  const greeter = ctx.use<Greeter>("@demo/greeter");
  greeter.on("greeted", (p: { slot: number; tick: number }) =>
    console.log(`[consumer] event greeted: slot=${p.slot} tick=${p.tick}`));
  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 === 0) {
      try {
        console.log(`[consumer] greet -> ${greeter.greet(0)}`);
      } catch (e) {
        console.log(`[consumer] greet failed (degraded): ${String(e)}`);
      }
    }
  });
});
