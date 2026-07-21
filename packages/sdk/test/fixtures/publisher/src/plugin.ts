import { plugin } from "@s2script/sdk/plugin";
import type { Publisher } from "../api";

export default plugin((ctx) => {
  const impl: Publisher = {
    ping(): boolean {
      return true;
    },
  };
  ctx.publish("@demo/publisher", impl);
});
