/** Plugin-shippable gamedata. v1 accepts `signatures` + `calls` + `hooks`. */
export const SUPPORTED_PLATFORMS = ["linuxsteamrt64", "windows64"] as const;
export type SupportedPlatform = (typeof SUPPORTED_PLATFORMS)[number];

export type ArgKind = "bool" | "int" | "float" | "string" | "vector" | "entity";
export type RetKind = "void" | "bool" | "int" | "float" | "entity";

export const ARG_KINDS: readonly ArgKind[] = ["bool", "int", "float", "string", "vector", "entity"];
export const RET_KINDS: readonly RetKind[] = ["void", "bool", "int", "float", "entity"];

/** Everything except `float` occupies an integer register under SysV. */
export const INT_CLASS_ARGS: ReadonlySet<ArgKind> = new Set<ArgKind>(["bool", "int", "string", "vector", "entity"]);
/** `this` consumes rdi, leaving 5 GP argument registers. */
/**
 * Declared integer-class args. Mirrors `kMaxGpArgs` (shim) and `MAX_GP_ARGS` (core) — an ABI.
 *
 * Six is the SysV *register* count, not a limit on the call: further integer args spill to the
 * stack, which the shim's prototypes cover. The budget is 9 declared args, plus the receiver when
 * the descriptor has one.
 */
export const MAX_INT_ARGS = 9;
export const MAX_FLOAT_ARGS = 8;

export interface SigSpec { module: string; pattern: string; resolve: string }
export interface ViaSpec { class: string; field: string }
/**
 * How the call obtains its `this`.
 *
 * `"entity"` — a books-gated entity ref supplies the receiver, optionally hopping through ONE
 * schema-named sub-object pointer (`via`).
 * `"none"`   — a STATIC/free engine function with no receiver at all. The generated callable takes
 * no leading `self`, and `via` is rejected: there is no receiver to hop from.
 */
export type Receiver = { kind: "entity"; via?: ViaSpec } | { kind: "none" };

export interface SignatureTarget { kind: "signature"; name: string }
export interface VtablePlatform { index: number; validate: { prologue: string } }
export interface VtableTarget { kind: "vtable"; class: string; [platform: string]: unknown }

/**
 * The shim's closed inbound-thunk vocabulary. MUST match `SHAPES` in
 * `core/src/gamedata_hooks.rs` (kept in sync with the shim by `scripts/check-hook-shapes.sh`).
 * A new shape is a core change; this list is the build-time echo so a typo fails here, not at load.
 */
export const HOOK_SHAPES = ["this_void", "this_f32_i32_i32_i32", "this_f32_i32_i64_i64", "this_i64_i32_i64"] as const;
export type HookShape = (typeof HOOK_SHAPES)[number];

/** Positional arity implied by the shape name (`this_void` → 0; otherwise one slot per `_`-token). */
export function hookShapeArity(shape: string): number | undefined {
  if (!(HOOK_SHAPES as readonly string[]).includes(shape)) return undefined;
  if (shape === "this_void") return 0;
  return shape.slice("this_".length).split("_").length;
}

export interface HookDecl {
  target: (SignatureTarget & { validate?: Record<string, unknown> }) | VtableTarget;
  shape: HookShape;
  params?: string[];
  mutable?: string[];
  receiver?: { kind?: "none" | "entity"; as?: string };
  bypassWith?: string;
  /** Required by the runtime: a hook with none is refused as "nothing could subscribe to it". */
  expose: { ctx: string };
}

export interface CallDecl {
  receiver: Receiver;
  target: SignatureTarget | VtableTarget;
  args: ArgKind[];
  returns: RetKind;
  /**
   * OPTIONAL author-facing parameter names, positionally matched to `args`. Purely documentary: the
   * generated `.d.ts` uses them instead of `a0…aN`, which matters because on an `unsafe` FFI surface
   * the parameter names ARE the documentation — `ignite(pawn.ref, 10.0, 4, null, 0.0)` is unreadable
   * without them.
   *
   * Deliberately a parallel array rather than turning `args` into objects: the runtime only ever needs
   * the kinds, so keeping `args` a flat string array means core parses the packed gamedata unchanged
   * (it ignores this field entirely) and there is no ABI or marshalling implication.
   */
  argNames?: string[];
}

export interface PluginGamedata {
  signatures?: Record<string, Record<string, SigSpec>>;
  calls?: Record<string, CallDecl>;
  hooks?: Record<string, HookDecl>;
}
