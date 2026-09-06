import type { HookResultValue } from "./events";
/**
 * @s2script/interfaces — author-time type stubs for typed inter-plugin interfaces.
 * NO runtime code: the engine injects the implementation at load time.
 */

/**
 * Handle returned by `ctx.publish(name, impl)`: lets the producer emit forwarded
 * events to every plugin subscribed to this interface via its `on(event, …)`.
 */
export interface PublishHandle {
  /**
   * Emit a forwarded event to all consumers subscribed via `interface.on(event, …)`. The payload is
   * structured-copied across the context boundary (JSON, EntityRef-aware — never a live reference).
   * @example
   * import type { PublishHandle } from "@s2script/sdk/interfaces";
   * // plugins/zones/src/plugin.ts:30 — notify consumers a zone was created
   * iface.emit("created", { zone: z.name, min: z.min, max: z.max, tags: z.tags });
   */
  emit(event: string, payload: unknown): void;
}

/** Type-only protocol 2 descriptor. Payloads must belong to the supported wire algebra. */
export interface Notification<P> {
  readonly __notificationPayload: P;
}
/** Synchronous decision forward. Changed does not imply payload mutation. */
export interface Hook<P> {
  readonly __hookPayload: P;
}
/** Synchronous shallow transformation; only W fields may be patched. */
export interface Transform<P extends object, W extends keyof P> {
  readonly __transformPayload: P;
  readonly __transformWritable: W;
}
export type TransformResponse<P, W extends keyof P> =
  | { result: 1; patch?: Partial<Pick<P, W>> }
  | { result: Exclude<HookResultValue, 1>; patch?: never };
/** CLI-generated association; an authored augmentation cannot authorize a build. */
export interface InterfaceContracts {}
export type ContractMethods<C> = C extends { methods: infer M extends object }
  ? M
  : never;
export type ContractForwards<C> = C extends { forwards: infer F } ? F : never;
export type NotificationPayload<D> = D extends Notification<infer P>
  ? P
  : never;
export type ForwardPayload<D> = D extends Notification<infer P>
  ? P
  : D extends Hook<infer P>
  ? P
  : D extends Transform<infer P, infer W>
  ? P
  : never;
export type ForwardResponse<D> = D extends Notification<infer P>
  ? void
  : D extends Hook<infer P>
  ? HookResultValue
  : D extends Transform<infer P, infer W>
  ? TransformResponse<P, W>
  : never;
type ForwardNames<C, Mode> = {
  [K in keyof ContractForwards<C>]: ContractForwards<C>[K] extends Mode
    ? K
    : never;
}[keyof ContractForwards<C>] &
  string;
export type DispatchResponse<D> = D extends Hook<unknown>
  ? HookResultValue
  : D extends { readonly __transformPayload: object }
  ? { result: HookResultValue; payload: ForwardPayload<D> }
  : never;
export type TypedPublishHandle<C> = {
  emit<K extends ForwardNames<C, Notification<unknown>>>(
    event: K,
    payload: ForwardPayload<ContractForwards<C>[K]>
  ): void;
  dispatch<
    K extends ForwardNames<
      C,
      Hook<unknown> | { readonly __transformPayload: object }
    >
  >(
    event: K,
    payload: ForwardPayload<ContractForwards<C>[K]>
  ): DispatchResponse<ContractForwards<C>[K]>;
};
type ExtraResponseKeys<R, W> = R extends unknown
  ?
      | Exclude<keyof R, "result" | "patch">
      | (R extends { patch?: infer Patch } ? Exclude<keyof Patch, W> : never)
  : never;
type ExactForwardHandler<D, R> = D extends Transform<infer P, infer W>
  ? ExtraResponseKeys<R, W> extends never
    ? unknown
    : never
  : unknown;
export type TypedInterfaceHandle<C> = ContractMethods<C> & {
  on<
    K extends keyof ContractForwards<C> & string,
    H extends (
      payload: ForwardPayload<ContractForwards<C>[K]>
    ) => ForwardResponse<ContractForwards<C>[K]>
  >(
    event: K,
    handler: H & ExactForwardHandler<ContractForwards<C>[K], ReturnType<H>>
  ): void;
};
