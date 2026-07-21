import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.publish("@demo/derived-self", {
    ping: () => 1,
  });
});
