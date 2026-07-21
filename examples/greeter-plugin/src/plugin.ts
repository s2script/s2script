import { plugin } from "@s2script/sdk/plugin";
import type { Greeter } from "../api";

// Producer: publishes the typed inter-plugin interface @demo/greeter@1.0.0 with a
// single native `greet(slot) -> string`, and emits a forwarded `greeted` event every
// ~256 frames so a consumer's on("greeted", …) subscription fires live. Built to a
// .s2sp by `npx s2script build`; dropped into addons/s2script/plugins/ to load.
export default plugin((ctx) => {
  console.log("[greeter] onLoad — publishing @demo/greeter");
  // Typed against the contract: tsc fails the build if this drifts from api.d.ts.
  // The version is injected by the host from the manifest's `publishes` — never typed here.
  const impl: Greeter = {
    greet(slot: number): string {
      return `hello, player ${slot}`;
    },
  };
  const handle = ctx.publish("@demo/greeter", impl);
  // Emit a forwarded event every ~256 frames so the consumer's on("greeted") fires live.
  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 === 0) handle.emit("greeted", { slot: 0, tick: ticks });
  });
});
