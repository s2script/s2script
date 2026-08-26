// Pure model: gamedata `hooks` → typed hook descriptors, grouped by `expose.ctx`.
// No I/O, no Date/random — deterministic. Sort by ctx namespace, then by hook name.
//
// This is the codegen-side MIRROR of `core/src/gamedata_hooks.rs::prepare` — it reads the exact
// same descriptor shape (`target`/`shape`/`params`/`mutable`/`receiver`/`bypassWith`/`expose.ctx`)
// but only cares about the surface a plugin author sees: the ctx namespace, the method name, the
// view's fields and their mutability, and the receiver's surfaced name. Grammar enforcement (an
// unknown shape, a dangling `bypassWith`, `mutable` not a subset of `params`, a missing `validate`)
// is `scripts/check-call-descriptors.sh`'s job, not this generator's — a hook that fails that gate
// never ships, so this model does not need to re-validate it to stay correct.

/** The on-disk shape of one `hooks.<name>` entry, restricted to what the generator reads. */
export interface RawHookDecl {
  /** The ABI shape name. Codegen needs it because a param's TYPE comes from the shape, not from the
   *  `params` list (which carries only names) — see STRING_PARAMS in emit-dts.ts. */
  shape?: string;
  params?: string[];
  mutable?: string[];
  receiver?: { kind?: string; as?: string };
  expose?: { ctx?: string; handwritten?: boolean };
  // target/bypassWith/validate are read by the runtime and the grammar gate, not by codegen.
}

export type GamedataHooks = Record<string, RawHookDecl>;

/** One positional param in a hook's view, in author order. */
export interface HookParamDescriptor {
  name: string;
  /** May a handler write this one back to the engine? Mirrors `HookPlan.writable`. */
  mutable: boolean;
}

/** One `hooks` entry, ready to emit: its view's shape and the ctx method it hangs off. */
export interface HookDescriptor {
  /** The hook's gamedata key (e.g. "onTerminateRound") — also the generated ctx method name. */
  name: string;
  /** The ABI shape name, verbatim. Decides each param's emitted TYPE. */
  shape: string;
  /** The exported view interface name (e.g. "OnTerminateRoundView"). */
  viewIface: string;
  /** Positional params, in author order. Empty for a nullary shape (e.g. `this_void`). */
  params: HookParamDescriptor[];
  /** The property name a books-gated EntityRef receiver is surfaced under, or null for
   *  `receiver.kind: "none"` (the default — most detour receivers are not entities at all). */
  receiverAs: string | null;
}

/** One `ctx.<ns>` namespace and every hook declared under it. */
export interface CtxNamespace {
  /** The `PluginContext` member name (e.g. "gameRules") — `expose.ctx` verbatim. */
  ns: string;
  /** The exported namespace interface name (e.g. "CtxGameRules"), mirroring the hand-written
   *  `CtxEvents`/`CtxClients`/… interfaces in `packages/sdk/plugin.d.ts`. */
  ctxIface: string;
  hooks: HookDescriptor[];
}

/** PascalCase("onTerminateRound") → "OnTerminateRound" (the caller appends "View"/uses it as-is).
 *  Hook names are already camelCase (unlike event names, which are snake_case), so this is just a
 *  first-letter capitalize — not the eventgen `pascalCase`'s underscore split. */
function capitalize(name: string): string {
  return name.length === 0 ? name : name.charAt(0).toUpperCase() + name.slice(1);
}

function hookDescriptor(name: string, decl: RawHookDecl): HookDescriptor {
  const paramNames = decl.params ?? [];
  const mutableSet = new Set(decl.mutable ?? []);
  const params: HookParamDescriptor[] = paramNames.map((p) => ({ name: p, mutable: mutableSet.has(p) }));
  const receiverAs = decl.receiver?.kind === "entity" ? decl.receiver.as ?? null : null;
  return { name, shape: decl.shape ?? "", viewIface: `${capitalize(name)}View`, params, receiverAs };
}

/** Flat list of every declared hook, for the plugin-side `Engine.hook` types. Unlike
 *  {@link buildHookModel} this does not group by `expose.ctx` and does not skip an unexposed hook —
 *  the plugin validator already refused those, and `Engine.hook` is keyed by the gamedata name. */
export function buildPluginHookList(hooks: GamedataHooks): HookDescriptor[] {
  return Object.keys(hooks).sort().map((name) => hookDescriptor(name, hooks[name] ?? {}));
}

/**
 * Build a sorted, deterministic model from a gamedata `hooks` section.
 *
 * A hook with no (or a blank) `expose.ctx` is skipped: core's `prepare()` refuses to register it at
 * all ("nothing could subscribe to it"), so it never reaches the shim and there is nothing for a
 * plugin to bind against — generating a dangling method for it would be worse than omitting it. That
 * grammar failure is `check-call-descriptors.sh`'s job to catch at CI time, not this generator's.
 */
export function buildHookModel(hooks: GamedataHooks): CtxNamespace[] {
  const byNs = new Map<string, HookDescriptor[]>();
  for (const name of Object.keys(hooks).sort()) {
    const decl = hooks[name] ?? {};
    const ns = decl.expose?.ctx;
    if (!ns) continue;
    // First-class views (CanAcquire) are hand-written in packages/cs2/items.d.ts.
    if ((decl.expose as { handwritten?: boolean } | undefined)?.handwritten) continue;

    const list = byNs.get(ns) ?? [];
    list.push(hookDescriptor(name, decl));
    byNs.set(ns, list);
  }
  return Array.from(byNs.keys())
    .sort()
    .map((ns) => ({ ns, ctxIface: `Ctx${capitalize(ns)}`, hooks: byNs.get(ns)! }));
}
