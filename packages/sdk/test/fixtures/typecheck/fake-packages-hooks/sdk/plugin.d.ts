// Deliberately MINIMAL fake of packages/sdk/plugin.d.ts — only what this isolated tree's one
// fixture (game-ctx-only) needs. See globals.d.ts for why this lives in its own packagesDir rather
// than the shared fake-packages/.
export interface PluginContext {
  readonly id: string;
}
export type PluginFactory = (ctx: PluginContext) => void;
export interface PluginDefinition { readonly __s2plugin: 1; }
export declare function plugin(factory: PluginFactory): PluginDefinition;
