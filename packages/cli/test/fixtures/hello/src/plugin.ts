/**
 * Demo plugin fixture — imports @s2script/frame and @s2script/timers so esbuild emits
 * require("@s2script/frame") / require("@s2script/timers") in the CJS bundle (marked external).
 */
import { OnGameFrame } from "@s2script/frame";
import { delay } from "@s2script/timers";

OnGameFrame.subscribe(() => {
  console.log("frame tick");
});

export const onLoad = async (): Promise<void> => {
  await delay(100);
  console.log("hello from @demo/hello");
};
