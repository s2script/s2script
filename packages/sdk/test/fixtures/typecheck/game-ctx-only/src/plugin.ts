// Deliberately imports NOTHING by name from "@s2script/cs2" — only depends on it via package.json.
// A plugin whose only use of the game package is a `ctx` hook (never a Player/Pawn/etc. import)
// must still see ctx.gameRules: the module augmentation lives in a file nothing else reaches.
import { plugin } from "@s2script/sdk/plugin";

export default plugin((ctx) => {
  ctx.gameRules.onTerminateRound(() => {});
});
