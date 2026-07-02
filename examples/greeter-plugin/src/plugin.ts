import { OnGameFrame } from "@s2script/frame";
import { publishInterface, PublishHandle } from "@s2script/interfaces";

// Producer: publishes the typed inter-plugin interface @demo/greeter@1.0.0 with a
// single native `greet(slot) -> string`, and emits a forwarded `greeted` event every
// ~256 frames so a consumer's on("greeted", …) subscription fires live. Built to a
// .s2sp by `npx s2script build`; dropped into addons/s2script/plugins/ to load.
let handle: PublishHandle | null = null;
let ticks = 0;

export function onLoad(): void {
  console.log("[greeter] onLoad — publishing @demo/greeter@1.0.0");
  handle = publishInterface("@demo/greeter", "1.0.0", {
    greet(slot: number): string {
      return `hello, player ${slot}`;
    },
  });
  // Emit a forwarded event every ~256 frames so the consumer's on("greeted") fires live.
  OnGameFrame.subscribe(() => {
    if (ticks++ % 256 === 0 && handle) handle.emit("greeted", { slot: 0, tick: ticks });
  });
}

export function onUnload(): void {
  console.log("[greeter] onUnload");
}
