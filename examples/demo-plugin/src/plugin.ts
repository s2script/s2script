import { OnGameFrame, delay } from "@s2script/std";
import { Pawn } from "@s2script/cs2";

// A minimal @s2script/cs2-targeting plugin exercising Slices 1–3:
//   OnGameFrame.subscribe (Slice 1), await delay (Slice 2), Pawn.forSlot(...).health (Slice 3).
// Built to a .s2sp by `npx s2script build`; dropped into addons/s2script/plugins/ to load.
let ticks = 0;

export function onLoad(): void {
  console.log("[demo] onLoad");
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 === 0) {
      const p = Pawn.forSlot(0);
      console.log("[demo] tick " + ticks + " hp=" + (p ? p.health : "none"));
    }
  });
  (async () => {
    console.log("[demo] before delay");
    await delay(1000);
    console.log("[demo] after delay(1000)");
  })();
}

export function onUnload(): void {
  console.log("[demo] onUnload");
}
