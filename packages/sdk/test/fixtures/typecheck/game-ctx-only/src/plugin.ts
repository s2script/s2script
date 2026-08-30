import type { PluginContext } from "@s2script/sdk/plugin";

// Deliberately imports NOTHING by name from "@s2script/cs2" — only depends on it via package.json.
// A plugin whose only use of the game package is a PluginContext hook (never a Player/Pawn/etc.
// import) must still see ctx.gameRules: the module augmentation lives in a file nothing else reaches.
function usesGameRules(ctx: PluginContext): void {
  ctx.gameRules.onTerminateRound(() => {});
}

export function OnPluginStart(): void {
  void usesGameRules;
}
