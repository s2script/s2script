declare module "@s2script/sdk/plugin" {
  import type { UserCmdView } from "@s2script/sdk/usercmd";
  import type { PublishHandle } from "@s2script/sdk/interfaces";
  export interface CtxEvents { on(name: string, h: (ev: unknown) => void): void; }
  export interface CtxClients {
    onRunCmd(h: (cmd: UserCmdView, info: { slot: number }) => number | void): void;
  }
  export interface CtxCommands { register(name: string, h: (cmd: unknown) => void): void; }
  export type InterfaceHandle<T extends object> = T & {
    on(event: string, handler: (payload: any) => void): void;
  };
  export interface Scope { clear(): void; dispose(): void; }
  export interface PluginContext {
    readonly events: CtxEvents;
    readonly clients: CtxClients;
    readonly commands: CtxCommands;
    publish<T extends object>(name: string, impl: T): PublishHandle;
    use<T extends object>(name: string): InterfaceHandle<T>;
    tryUse<T extends object>(name: string): InterfaceHandle<T> | null;
    createScope(): Scope;
  }
  export interface PluginDefinition { readonly __s2plugin: 1; }
  export function plugin(factory: (ctx: PluginContext) => unknown): PluginDefinition;
}
declare module "@s2script/sdk/usercmd" {
  export interface UserCmdView { buttons: bigint; forwardMove: number; }
}
declare module "@s2script/sdk/interfaces" {
  export interface PublishHandle { emit(event: string, payload: unknown): void; }
}
declare module "@s2script/sdk/db" {
  export const Database: {
    open(name: string): Promise<{ query(sql: string): Promise<unknown> }>;
  };
}
