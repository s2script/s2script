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
export interface EmbeddedClassDescriptor { className: string; fields: FieldDescriptor[]; skipped: SkippedField[]; }
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
  const embedSet = new Set(embeddable.filter((n) => catalog[n]));
  for (const n of embeddable) if (!catalog[n]) throw new Error(`gen-schema: embedded class '${n}' is not in the catalog`);
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
  // 3. Per class: classify own fields.
  const usedEmbeds = new Set<string>();
  const classes: ClassDescriptor[] = ordered.map((className) => {
    const parent = catalog[className].parent;
    const ownFields: FieldDescriptor[] = [];
    const embeddedFields: EmbeddedFieldDescriptor[] = [];
    const skipped: SkippedField[] = [];
    for (const f of catalog[className].fields) {
      const c = classifyField(f.type, embedSet);
      if ("skip" in c) { skipped.push({ className, rawName: f.name, reason: c.skip }); continue; }
      if ("embedded" in c) {
        usedEmbeds.add(c.embedded);
        embeddedFields.push({ propName: idiomaticName(f.name), rawName: f.name, declaringClass: className, embeddedClass: c.embedded });
        continue;
      }
      ownFields.push({ propName: idiomaticName(f.name), rawName: f.name, declaringClass: className, accessorKind: c.accessorKind, writable: c.writable, strLen: c.strLen });
    }
    return { className, parent: parent && inClosure.has(parent) ? parent : null, ownFields, embeddedFields, skipped };
  });
  // 3b. Descriptors for every embedded struct actually referenced. Only structs reached from a
  //     generated class are emitted, so listing one that nothing embeds costs nothing.
  //     Embedded structs are flattened across their own ancestry: unlike entities they are not
  //     emitted as an `extends` chain, so a parent's fields have to be folded in here.
  const embedded: EmbeddedClassDescriptor[] = [...usedEmbeds].sort().map((className) => {
    const fields: FieldDescriptor[] = [];
    const skipped: SkippedField[] = [];
    const chain: string[] = [];
    for (let cur: string | null = className; cur && catalog[cur]; cur = catalog[cur].parent) chain.unshift(cur);
    for (const owner of chain) {
      for (const f of catalog[owner].fields) {
        // No nesting-within-nesting: one level keeps the accessor a plain (ref, base) pair.
        const c = classifyField(f.type);
        if ("skip" in c) { skipped.push({ className: owner, rawName: f.name, reason: c.skip }); continue; }
        if ("embedded" in c) continue;
        fields.push({ propName: idiomaticName(f.name), rawName: f.name, declaringClass: owner, accessorKind: c.accessorKind, writable: c.writable, strLen: c.strLen });
      }
    }
    return { className, fields, skipped };
  });
  // 4. Collision pass: an idiomatic propName shared by >=2 distinct fields (by declaringClass+rawName) -> raw fallback for all.
  //    Embedded field names share the namespace (they become properties on the same object), so they
  //    take part too — otherwise an embedded `m_Glow` could silently shadow a scalar named `glow`.
  const byProp = new Map<string, { propName: string; rawName: string; declaringClass: string }[]>();
  for (const c of classes) for (const f of [...c.ownFields, ...c.embeddedFields]) { (byProp.get(f.propName) ?? byProp.set(f.propName, []).get(f.propName)!).push(f); }
  const collisions: string[] = [];
  for (const [prop, fields] of byProp) {
    const distinct = new Set(fields.map((f) => `${f.declaringClass}.${f.rawName}`));
    if (distinct.size >= 2) { for (const f of fields) f.propName = f.rawName; collisions.push(`${prop} <- ${[...distinct].sort().join(", ")}`); }
  }
  collisions.sort();
  return { classes, embedded, collisions };
}
