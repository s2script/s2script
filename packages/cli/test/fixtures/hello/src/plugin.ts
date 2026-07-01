/**
 * Demo plugin fixture — imports @s2script/std so esbuild emits require("@s2script/std")
 * in the CJS bundle (the import is marked external).
 */
import { OnGameFrame, delay } from "@s2script/std";

OnGameFrame.subscribe(() => {
  console.log("frame tick");
});

export const onLoad = async (): Promise<void> => {
  await delay(100);
  console.log("hello from @demo/hello");
};
