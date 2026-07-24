/** Plugin-shippable gamedata. v1 accepts ONLY `signatures` + `calls` (spec §14). */
export const PLATFORM = "linuxsteamrt64" as const;

export type ArgKind = "bool" | "int" | "float" | "string" | "vector" | "entity";
export type RetKind = "void" | "bool" | "int" | "float" | "entity";

export const ARG_KINDS: readonly ArgKind[] = ["bool", "int", "float", "string", "vector", "entity"];
export const RET_KINDS: readonly RetKind[] = ["void", "bool", "int", "float", "entity"];

/** Everything except `float` occupies an integer register under SysV. */
export const INT_CLASS_ARGS: ReadonlySet<ArgKind> = new Set<ArgKind>(["bool", "int", "string", "vector", "entity"]);
/** `this` consumes rdi, leaving 5 GP argument registers. */
export const MAX_INT_ARGS = 5;
export const MAX_FLOAT_ARGS = 8;

export interface SigSpec { module: string; pattern: string; resolve: string }
export interface ViaSpec { class: string; field: string }
export interface Receiver { kind: "entity"; via?: ViaSpec }

export interface SignatureTarget { kind: "signature"; name: string }
export interface VtablePlatform { index: number; validate: { prologue: string } }
export interface VtableTarget { kind: "vtable"; class: string; [platform: string]: unknown }

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
}
