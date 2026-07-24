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
}

export interface PluginGamedata {
  signatures?: Record<string, Record<string, SigSpec>>;
  calls?: Record<string, CallDecl>;
}
