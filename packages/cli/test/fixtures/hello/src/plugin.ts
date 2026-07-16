/**
 * Demo plugin fixture — imports @s2script/sdk/frame and @s2script/sdk/timers so esbuild emits
 * require("@s2script/sdk/frame") / require("@s2script/sdk/timers") in the CJS bundle (external).
 */
import { OnGameFrame } from "@s2script/sdk/frame";
import { delay } from "@s2script/sdk/timers";

OnGameFrame.subscribe(() => {
  console.log("frame tick");
});

export const onLoad = async (): Promise<void> => {
  await delay(100);
  console.log("hello from @demo/hello");
};
