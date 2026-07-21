import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.publish("@demo/other-name", {
    ping: () => 1,
  });
});
