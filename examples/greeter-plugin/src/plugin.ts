// Producer: publishes the typed inter-plugin interface @demo/greeter@1.0.0.
//
// Methods become natives the consumer calls; handle.emit() sends forwarded
// events the consumer subscribes to with on(). Arguments and payloads cross by
// STRUCTURED COPY as JSON — never a live pointer. An EntityRef is the one
// exception: it is tagged crossing the wire and revived bound to the consumer's
// own natives, so it arrives as a live, serial-gated ref.
//
// Carry 64-bit values as decimal strings: a BigInt throws and silently drops
// the whole payload.
import { plugin } from "@s2script/sdk/plugin";
import { Pawn } from "@s2script/cs2";
import type { Greeter } from "../api";

export default plugin((ctx) => {
  console.log("[greeter] onLoad — publishing @demo/greeter");

  // Typed against the contract: tsc fails the build if this drifts from api.d.ts.
  // The version is injected by the host from the manifest — never typed here.
  const impl: Greeter = {
    greet(slot: number): string {
      return `hello, player ${slot}`;
    },
    pawnRef(slot: number) {
      const pawn = Pawn.forSlot(slot);
      return pawn ? pawn.ref : null;
    },
    pawnHealth(slot: number) {
      const pawn = Pawn.forSlot(slot);
      return pawn ? pawn.health : null;
    },
  };

  const handle = ctx.publish("@demo/greeter", impl);

  // Forwarded events: the consumer's on("greeted") fires from here.
  let ticks = 0;
  ctx.server.onGameFrame(() => {
    if (ticks++ % 256 === 0) handle.emit("greeted", { slot: 0, tick: ticks });
  });
});
