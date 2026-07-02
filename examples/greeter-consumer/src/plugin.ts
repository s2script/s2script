import { OnGameFrame } from "@s2script/frame";
import greeter = require("@demo/greeter"); // hard dep → producer-backed proxy

// Consumer: hard-deps @demo/greeter (a proxy that throws InterfaceUnavailable while the
// producer is unloaded), subscribes to the producer's forwarded `greeted` event, and
// calls greet(0) every ~256 frames. The greet call is wrapped in try/catch so a
// producer-absent throw degrades gracefully (logs + keeps ticking) instead of crashing.
let ticks = 0;

export function onLoad(): void {
  console.log("[consumer] onLoad");
  greeter.on("greeted", (p) => console.log(`[consumer] event greeted: slot=${p.slot} tick=${p.tick}`));
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 === 0) {
      try {
        console.log(`[consumer] greet -> ${greeter.greet(0)}`);
      } catch (e) {
        console.log(`[consumer] greet failed (degraded): ${String(e)}`);
      }
    }
  });
}

export function onUnload(): void {
  console.log("[consumer] onUnload");
}
