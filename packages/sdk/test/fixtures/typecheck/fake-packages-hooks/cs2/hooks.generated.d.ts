// Fixture stand-in for packages/cs2/hooks.generated.d.ts — proves gamePackageDeclarationFiles
// forces this file (and its PluginContext augmentation) into the program even when nothing in the
// plugin under test imports a NAME from "@s2script/cs2".
export interface CtxGameRules {
  onTerminateRound(handler: () => void): void;
}
declare module "@s2script/sdk/plugin" {
  interface PluginContext {
    readonly gameRules: CtxGameRules;
  }
}
