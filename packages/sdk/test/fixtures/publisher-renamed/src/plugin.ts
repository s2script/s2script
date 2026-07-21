import { plugin } from "@s2script/sdk/plugin";
import type { OtherName } from "../api";

export default plugin((ctx) => {
  const impl: OtherName = {
    pong(): boolean {
      return true;
    },
  };
  ctx.publish("@demo/other-name", impl);
});
