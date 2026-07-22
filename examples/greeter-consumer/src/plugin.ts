// Consumer: hard-deps @demo/greeter. ctx.use returns a proxy that throws
// InterfaceUnavailable while the producer is unloaded, so calls are wrapped —
// a producer reload degrades gracefully instead of crashing this plugin.
// (For a dependency you can live without, declare it under
// optionalPluginDependencies and use ctx.tryUse, which returns null instead.)
//
// Types come from the verified contract copy at
// .s2script/types/@demo/greeter/index.d.ts — a byte-copy of the producer's
// api.d.ts that s2s build hashes into manifest.compiledAgainst, so a drifted
// contract is refused at load rather than marshalled across. Refresh with:
//   cp examples/greeter-plugin/api.d.ts examples/greeter-consumer/.s2script/types/@demo/greeter/index.d.ts
import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "@demo/greeter";

export default plugin((ctx) => {
  console.log("[consumer] onLoad");
  const greeter = ctx.use<Greeter>("@demo/greeter");

  // A forwarded event from the producer.
  greeter.on("greeted", (p: { slot: number; tick: number }) =>
    console.log(`[consumer] event greeted: slot=${p.slot} tick=${p.tick}`));

  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 !== 0) return;
    try {
      console.log(`[consumer] greet -> ${greeter.greet(0)}`);

      // An EntityRef received ACROSS the plugin boundary. isValid() checks it
      // against the SHARED entity system: true while the pawn lives, false once
      // it dies. That flip is cross-plugin host-invalidation, and it needs no
      // schema offset on this side — pawnHealth is the producer's read.
      const ref = greeter.pawnRef(0);
      const alive = ref ? ref.isValid() : false;
      console.log(`[consumer] pawn ref valid=${alive} health=${alive ? greeter.pawnHealth(0) : "null"}`);
    } catch (e) {
      console.log(`[consumer] degraded (producer unloaded?): ${String(e)}`);
    }
  });
});
