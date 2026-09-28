import type { PLATFORM, RECEIVERS, ATOM_WIDTHS } from './abi.generated.ts';

export type NativeAtom = keyof typeof ATOM_WIDTHS;
export type NativeReturn = NativeAtom | 'void';
export type Receiver = (typeof RECEIVERS)[number];
export type Platform = typeof PLATFORM;
export type Surface = 'call' | 'pre' | 'post';
export type AuthorType = 'bool' | 'i32' | 'u32' | 'i64' | 'u64' | 'f32' | 'f64' | 'entity' | 'entity?' | 'string' | 'vector';
export type ParameterCopyOwnership = 'callee-borrowed' | 'callee-retained' | 'native-observed';
export type ReturnCopyOwnership = 'caller-borrowed' | 'native-observed';

export interface ValidatorSpec {
  prologue?: string;
  'string-xref'?: { at: number; dispOff: number; instrLen: number; expect: string };
  'vtable-member'?: string;
}

export interface AuthorSignatureTarget {
  kind?: 'signature';
  module: string;
  pattern: string;
  validate: ValidatorSpec;
  /** Only meaningful for validated-call: validators run on the derived callee. */
  targetValidate?: ValidatorSpec;
}

export interface AuthorVirtualTarget {
  kind: 'virtual';
  module: string;
  class: string;
  index: number;
  validate: ValidatorSpec;
}

export type AuthorTarget = AuthorSignatureTarget | AuthorVirtualTarget;

export interface AuthorFunction {
  target: AuthorTarget | { ref: string };
  receiver?: { type: Receiver };
  parameters?: Array<{ name: string; type: AuthorType; ownership?: ParameterCopyOwnership; mutable?: 'pre' }>;
  returns?: AuthorType | 'void' | { type: 'string' | 'vector'; ownership: ReturnCopyOwnership };
  surfaces?: Surface[];
  suppression?: 'generic' | 'none';
  requirement?: 'optional' | 'required';
  resolve?: 'direct' | 'ctor-body-xref' | 'lea-disp' | 'validated-call';
}

export interface FunctionFileV2 {
  schemaVersion: 2;
  targets?: Record<string, AuthorTarget>;
  functions: Record<string, AuthorFunction>;
}

/** Projection ids remain open so host-owned Task 6 codecs can be represented. Community v2
 * authoring is deliberately closed to the safe built-ins until a registration authority exists. */
export interface ProjectionSpec { id: string; version: number }

export interface NormalizedSignatureTarget {
  kind: 'signature';
  module: string;
  pattern: string;
  resolve: AuthorFunction['resolve'] & string;
  derivation: 'identity' | 'ctor-body-xref' | 'lea-disp' | 'e8-rel32';
  candidateValidate: ValidatorSpec;
  targetValidate: ValidatorSpec;
}

export interface NormalizedVirtualTarget {
  kind: 'virtual';
  module: string;
  class: string;
  index: number;
  resolve: 'direct';
  derivation: 'virtual-slot';
  candidateValidate: ValidatorSpec;
  targetValidate: ValidatorSpec;
}

export type NormalizedTarget = NormalizedSignatureTarget | NormalizedVirtualTarget;

export interface NormalizedFunction {
  localName: string;
  canonicalId: string;
  contractHash: string;
  target: NormalizedTarget;
  abi: {
    platform: Platform;
    receiver: Receiver;
    fingerprint: string;
    stackCopyBytes: number;
    parameters: Array<{ name: string; native: NativeAtom; projection: ProjectionSpec; ownership?: ParameterCopyOwnership; mutable: ('pre')[] }>;
    returns: { native: NativeReturn; projection: ProjectionSpec; ownership?: ReturnCopyOwnership };
  };
  policy: {
    /** Host-owned compatibility adapters may use a named id; community normalization emits generic.v2 only. */
    id: string; version: 1; contractHash: string;
    surfaces: Surface[]; selfCall: 'bypass-own-hooks'; suppression: 'generic' | 'none' | 'adapter';
  };
  requirement: 'optional' | 'required';
}

export interface NormalizedBundle {
  schemaVersion: 2;
  ownerId: string;
  bundleHash: string;
  functions: NormalizedFunction[];
}

export interface EngineFunctionsManifestSummary {
  schemaVersion: 2;
  bundleHash: string;
  permissions: ('engine:calls' | 'engine:hooks')[];
  functions: Array<{ canonicalId: string; contractHash: string; surfaces: Surface[]; mutates: boolean; suppresses: boolean; requirement: 'optional' | 'required' }>;
}
