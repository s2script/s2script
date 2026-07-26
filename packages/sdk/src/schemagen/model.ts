// Pure model: catalog + curated list → normalized per-class accessor descriptors.
// No I/O, no Date/random — deterministic. See the plan's Global Constraints.

export type Catalog = Record<string, { parent: string | null; fields: CatalogField[] }>;
export interface CatalogField {
  name: string;
  offset: number;
  type: { kind: string; name?: string; inner?: string };
}

export type AccessorKind = "f32" | "bool" | "i8" | "i16" | "i32" | "u8" | "u16" | "u32" | "handle" | "u64" | "i64" | "f64" | "str" | "vector" | "qangle";

export interface FieldDescriptor {
  propName: string;
  rawName: string;
  declaringClass: string;
  accessorKind: AccessorKind;
  writable: boolean;
  strLen?: number;
  /** Extra bytes added to the resolved offset — non-zero only for a flattened value wrapper,
   *  where the scalar sits at `wrapperOffset + this`. See {@link transparentValue}. */
  addOffset?: number;
}
export interface SkippedField { className: string; rawName: string; reason: string; }

/**
 * A field whose type is a struct embedded INLINE in the owner (`m_Glow`, `m_Collision`), exposed as
 * a nested accessor object rather than flattened.
 *
 * Embedding is opt-in per struct (`codegen-embedded.json`). Most `kind: "class"` fields in the
 * catalog are single-scalar wrappers such as `GameTime_t`, and surfacing all ~63 of them as nested
 * objects would bloat the generated surface for no gain — so only structs that are genuinely worth
 * addressing as a unit are listed.
 */
export interface EmbeddedFieldDescriptor {
  propName: string;
  rawName: string;
  /** The class that OWNS the embedded field — the offset base is resolved against this. */
  declaringClass: string;
  /** The struct type; its own accessors are resolved against this. */
  embeddedClass: string;
}
export interface EmbeddedClassDescriptor { className: string; fields: FieldDescriptor[]; embeddedFields: EmbeddedFieldDescriptor[]; skipped: SkippedField[]; }
export interface ClassDescriptor { className: string; parent: string | null; ownFields: FieldDescriptor[]; embeddedFields: EmbeddedFieldDescriptor[]; skipped: SkippedField[]; }
export interface SchemaModel { classes: ClassDescriptor[]; embedded: EmbeddedClassDescriptor[]; collisions: string[]; }

// AccessorKind → EntityRef method (5B.2 surface) + TS type. Writable ⇔ a WRITE entry exists.
export const READ: Record<AccessorKind, string> = {
  f32: "readFloat32", bool: "readBool", i8: "readInt8", i16: "readInt16",
  i32: "readInt32", u8: "readUInt8", u16: "readUInt16", u32: "readUInt32", handle: "readHandle",
  u64: "readUInt64", i64: "readInt64", f64: "readFloat64", str: "readString",
  vector: "readFloats", qangle: "readFloats",
};
export const WRITE: Partial<Record<AccessorKind, string>> = {
  f32: "writeFloat32", bool: "writeBool",
  i8: "writeInt8", i16: "writeInt16", i32: "writeInt32",
  u8: "writeUInt8", u16: "writeUInt16", u32: "writeUInt32",
};
export const TSTYPE: Record<AccessorKind, string> = {
  f32: "number | null", bool: "boolean | null", i8: "number | null", i16: "number | null",
  i32: "number | null", u8: "number | null", u16: "number | null", u32: "number | null", handle: "EntityRef | null",
  u64: "string | null", i64: "string | null", f64: "number | null", str: "string | null",
  vector: "Vector | null", qangle: "QAngle | null",
};

// atomic subtype → (kind, writable). Only genuine scalars; everything else falls through to skip.
const ATOMIC: Record<string, { k: AccessorKind; w: boolean }> = {
  float32: { k: "f32", w: true }, bool: { k: "bool", w: true },
  int8: { k: "i8", w: true }, int16: { k: "i16", w: true }, int32: { k: "i32", w: true },
  uint8: { k: "u8", w: true }, uint16: { k: "u16", w: true }, uint32: { k: "u32", w: true },
  uint64: { k: "u64", w: false }, int64: { k: "i64", w: false }, float64: { k: "f64", w: false },   // no narrow 64-bit writers
  // `Color` is a 4-byte RGBA struct, not a scalar, but it is laid out exactly as a little-endian
  // uint32 (R in the low byte) and every engine consumer treats it that way. Mapping it here rather
  // than adding a value class keeps the surface small and matches how packed colours are already
  // passed elsewhere in the SDK (e.g. the fade user-message).
  Color: { k: "u32", w: true },
};

// atomic vector-type name → kind (only the fixed-3-float types this slice; 2D/4D/Color/Quaternion deferred).
const VEC: Record<string, AccessorKind> = { Vector: "vector", VectorWS: "vector", QAngle: "qangle" };
// kind → value-class + float count, for the emitters + import detection.
export const VEC_INFO: Partial<Record<AccessorKind, { cls: string; count: number }>> = {
  vector: { cls: "Vector", count: 3 },
  qangle: { cls: "QAngle", count: 3 },
};

const KNOWN_TAGS = new Set(["i","n","b","h","fl","f","u","e","p","a","v","vec","ang","q","sz","isz","ch","clr","un"]);
export function idiomaticName(raw: string): string {
  const s = raw.replace(/^m_/, "");
  const m = s.match(/^([a-z]+)([A-Z].*)$/);         // leading lowercase run, then an Uppercase-led core
  const core = (m && KNOWN_TAGS.has(m[1])) ? m[2] : s;
  return core.charAt(0).toLowerCase() + core.slice(1);
}

/**
 * A struct that exists only to name a scalar: exactly one field, called `m_Value`.
 *
 * `GameTime_t` (float32) and `GameTick_t` (int32) are these, and between them they account for 39 of
 * the 63 embedded fields in the CS2 closure — `m_flDeathTime`, `m_flCreateTime`, `m_fTimeLastHurt`
 * and so on. Exposing them as nested objects would mean writing `pawn.deathTime.value`, which is
 * strictly worse than the scalar it wraps, so they are FLATTENED: the field takes the wrapper's
 * name and the inner field's kind, read at `wrapperOffset + valueOffset`.
 *
 * The rule is deliberately narrow. `CHitboxComponent` also has a single field, but it is called
 * `m_flBoundsExpandRadius` and carries its own meaning — flattening that would produce a property
 * whose name says "hitbox component" and whose value is a radius. Only `m_Value` is anonymous
 * enough to absorb.
 */
export function transparentValue(catalog: Catalog, className: string): CatalogField | null {
  const e = catalog[className];
  if (!e || e.parent || e.fields.length !== 1) return null;
  const f = e.fields[0]!;
  return f.name === "m_Value" && f.type.kind === "atomic" ? f : null;
}

export function classifyField(type: CatalogField["type"], embeddable: ReadonlySet<string> = new Set()): { accessorKind: AccessorKind; writable: boolean; strLen?: number } | { embedded: string } | { skip: string } {
  if (type.kind === "handle") return { accessorKind: "handle", writable: false };
  if (type.kind === "atomic") {
    const vk = VEC[type.name ?? ""];
    if (vk) return { accessorKind: vk, writable: false };
    const m = ATOMIC[type.name ?? ""];
    if (m) return { accessorKind: m.k, writable: m.w };
    return { skip: `atomic '${type.name}' is not a scalar (string/vector/compound)` };
  }
  if (type.kind === "unknown") {
    const cm = (type.name ?? "").match(/^char\[(\d+)\]$/);
    if (cm) return { accessorKind: "str", writable: false, strLen: parseInt(cm[1], 10) };
    return { skip: `unmapped 'unknown' type '${type.name ?? ""}'` };
  }
  if (type.kind === "enum") return { skip: "enum byte-width absent from catalog (deferred)" };
  if (type.kind === "class") {
    const n = type.name ?? "";
    if (embeddable.has(n)) return { embedded: n };
    return { skip: `embedded class '${n}' deferred` };
  }
  if (type.kind === "ptr") return { skip: "raw pointer" };
  return { skip: `unmapped kind '${type.kind}'` };
}

export function flattenedFields(model: SchemaModel, className: string): FieldDescriptor[] {
  const byName = new Map<string, ClassDescriptor>(model.classes.map((c): [string, ClassDescriptor] => [c.className, c]));
  const chain: ClassDescriptor[] = [];
  let cur: string | null = className;
  while (cur && byName.has(cur)) { const c: ClassDescriptor = byName.get(cur)!; chain.unshift(c); cur = c.parent; }
  return chain.flatMap((c) => c.ownFields);
}

export function buildModel(catalog: Catalog, requested: string[], embeddable: string[] = []): SchemaModel {
  for (const n of embeddable) if (!catalog[n]) throw new Error(`gen-schema: embedded class '${n}' is not in the catalog`);
  // An explicit list, when given, restricts which structs embed; empty/absent means "every struct
  // the catalog describes". Curation stopped being necessary once transparent wrappers flatten —
  // those were the only reason the naive expansion was noisy.
  const restrict = embeddable.length > 0 ? new Set(embeddable) : null;
  const canEmbed = (n: string): boolean =>
    !!catalog[n] && transparentValue(catalog, n) === null && (restrict === null || restrict.has(n));
  const embedSet = new Set(Object.keys(catalog).filter(canEmbed));

  // 1. Closure: requested + ancestor chains (stop at null parent or a parent absent from the catalog).
  const inClosure = new Set<string>();
  for (const start of requested) {
    if (!catalog[start]) throw new Error(`gen-schema: requested class '${start}' is not in the catalog`);
    let cur: string | null = start;
    while (cur && catalog[cur] && !inClosure.has(cur)) { inClosure.add(cur); cur = catalog[cur].parent; }
  }
  // 2. Stable topological order: by depth-to-root, ties by name.
  const depth = (c: string): number => { let d = 0, cur: string | null = c; while (cur && catalog[cur]?.parent && inClosure.has(catalog[cur]!.parent!)) { d++; cur = catalog[cur]!.parent; } return d; };
  const ordered = [...inClosure].sort((a, b) => depth(a) - depth(b) || (a < b ? -1 : a > b ? 1 : 0));

  const usedEmbeds = new Set<string>();
  /** Classify one catalog field, resolving a transparent wrapper to the scalar it wraps. */
  const classify = (f: CatalogField): ReturnType<typeof classifyField> & { addOffset?: number } => {
    if (f.type.kind === "class") {
      const inner = transparentValue(catalog, f.type.name ?? "");
      if (inner) {
        const c = classifyField(inner.type);
        if (!("skip" in c) && !("embedded" in c)) return { ...c, addOffset: inner.offset };
      }
    }
    return classifyField(f.type, embedSet);
  };

  const fieldsOf = (owner: string, out: FieldDescriptor[], emb: EmbeddedFieldDescriptor[], skipped: SkippedField[]): void => {
    for (const f of catalog[owner]!.fields) {
      const c = classify(f);
      if ("skip" in c) { skipped.push({ className: owner, rawName: f.name, reason: c.skip }); continue; }
      if ("embedded" in c) {
        usedEmbeds.add(c.embedded);
        emb.push({ propName: idiomaticName(f.name), rawName: f.name, declaringClass: owner, embeddedClass: c.embedded });
        continue;
      }
      out.push({ propName: idiomaticName(f.name), rawName: f.name, declaringClass: owner, accessorKind: c.accessorKind, writable: c.writable, strLen: c.strLen, addOffset: c.addOffset });
    }
  };

  // 3. Per class: classify own fields.
  const classes: ClassDescriptor[] = ordered.map((className) => {
    const parent = catalog[className].parent;
    const ownFields: FieldDescriptor[] = [];
    const embeddedFields: EmbeddedFieldDescriptor[] = [];
    const skipped: SkippedField[] = [];
    fieldsOf(className, ownFields, embeddedFields, skipped);
    return { className, parent: parent && inClosure.has(parent) ? parent : null, ownFields, embeddedFields, skipped };
  });

  // 3b. Descriptors for every embedded struct reached, transitively. A struct may itself embed one
  //     (CAttributeContainer -> CEconItemView, CCollisionProperty -> VPhysicsCollisionAttribute_t);
  //     the accessor mechanism is identical at any depth, since a nested wrapper is just another
  //     (ref, base) pair with base = outerBase + fieldOffset. `done` both dedupes and breaks the
  //     cycle a self-referential struct would otherwise send this into.
  const embedded: EmbeddedClassDescriptor[] = [];
  const done = new Set<string>();
  for (;;) {
    const next = [...usedEmbeds].filter((n) => !done.has(n)).sort();
    if (next.length === 0) break;
    for (const className of next) {
      done.add(className);
      const fields: FieldDescriptor[] = [];
      const embeddedFields: EmbeddedFieldDescriptor[] = [];
      const skipped: SkippedField[] = [];
      const chain: string[] = [];
      for (let cur: string | null = className; cur && catalog[cur]; cur = catalog[cur].parent) chain.unshift(cur);
      for (const owner of chain) fieldsOf(owner, fields, embeddedFields, skipped);
      embedded.push({ className, fields, embeddedFields, skipped });
    }
  }
  embedded.sort((a, b) => (a.className < b.className ? -1 : a.className > b.className ? 1 : 0));

  // 4. Collision pass — RESOLVED PER INHERITANCE CHAIN, not globally.
  //
  // A property name is only ambiguous when two fields can land on the SAME wrapped object, and
  // `flattenedFields` builds an object from one inheritance chain. So `CCSPlayerController.m_iScore`
  // and `CTeam.m_iScore` are both `score` with no ambiguity whatsoever — they never coexist.
  //
  // Resolving globally instead made the surface unstable in the worst possible way: adding an
  // unrelated class silently RENAMED a property on an existing one. Adding CTeam alone turned
  // `controller.score` into `controller.m_iScore` for every plugin already using it, with no error
  // anywhere. That is a breaking change disguised as codegen coverage, and it is why this is
  // per-chain.
  //
  // Where a conflict IS real, the ANCESTOR keeps the idiomatic name and descendants fall back to the
  // raw one. `CBeam.m_fSpeed` genuinely collides with `CBaseEntity.m_flSpeed` on a CBeam wrapper, but
  // resolving it must not cost `speed` on every other entity in the game. Ancestor-wins also makes
  // the outcome independent of WHICH descendants happen to be generated — the same property keeps
  // the same name however the class list grows.
  const ancestors = (cls: string): Set<string> => {
    const out = new Set<string>();
    for (let cur: string | null = catalog[cls]?.parent ?? null; cur && catalog[cur]; cur = catalog[cur].parent) out.add(cur);
    return out;
  };
  const byProp = new Map<string, { propName: string; rawName: string; declaringClass: string }[]>();
  for (const c of classes) for (const f of [...c.ownFields, ...c.embeddedFields]) { (byProp.get(f.propName) ?? byProp.set(f.propName, []).get(f.propName)!).push(f); }
  const collisions: string[] = [];
  for (const [prop, fields] of byProp) {
    // Distinct declarations only: the same field reached through several generated subclasses is one
    // declaration, not a conflict with itself.
    const decls = new Map<string, { propName: string; rawName: string; declaringClass: string }[]>();
    for (const f of fields) (decls.get(`${f.declaringClass}.${f.rawName}`) ?? decls.set(`${f.declaringClass}.${f.rawName}`, []).get(`${f.declaringClass}.${f.rawName}`)!).push(f);
    if (decls.size < 2) continue;

    const keys = [...decls.keys()].sort();
    const clsOf = (k: string): string => decls.get(k)![0]!.declaringClass;
    const conflicting = new Set<string>();
    for (let i = 0; i < keys.length; i++) {
      for (let j = i + 1; j < keys.length; j++) {
        const a = clsOf(keys[i]!), b = clsOf(keys[j]!);
        // Same class, or one derives from the other -> they meet on one object.
        if (a === b || ancestors(a).has(b) || ancestors(b).has(a)) { conflicting.add(keys[i]!); conflicting.add(keys[j]!); }
      }
    }
    if (conflicting.size === 0) continue;

    // The shallowest class in the conflict keeps `prop`; everything else takes its raw name. Two
    // fields on the SAME class have no ancestor to break the tie, so both fall back.
    const depthOf = (k: string): number => ancestors(clsOf(k)).size;
    const inConflict = [...conflicting].sort((x, y) => depthOf(x) - depthOf(y) || (x < y ? -1 : 1));
    const winnerCls = clsOf(inConflict[0]!);
    const soleWinner = inConflict.filter((k) => clsOf(k) === winnerCls).length === 1;
    const renamed: string[] = [];
    for (const k of inConflict) {
      if (soleWinner && k === inConflict[0]) continue;
      for (const f of decls.get(k)!) f.propName = f.rawName;
      renamed.push(k);
    }
    collisions.push(`${prop} <- ${renamed.sort().join(", ")}${soleWinner ? ` (kept by ${winnerCls})` : ""}`);
  }
  collisions.sort();
  return { classes, embedded, collisions };
}
