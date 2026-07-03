// Pure model: nav-targets config + catalog → normalized nav wrappers with flattened readable fields.
// No I/O, no Date/random — deterministic. Reuses schemagen's buildModel + flattenedFields.

import { buildModel, flattenedFields, type Catalog, type FieldDescriptor, type AccessorKind } from "../schemagen/model.ts";

export interface NavHop { cls: string; field: string; }
export interface NavConfigEntry {
  prop: string;
  wrapper: string;
  source: string;
  target: string;
  path: NavHop[];
}

export interface NavWrapper {
  wrapper: string;
  prop: string;
  source: string;
  target: string;
  path: NavHop[];
  fields: FieldDescriptor[];
  skippedKinds: { propName: string; accessorKind: AccessorKind }[];
}

export interface NavModel {
  wrappers: NavWrapper[];
}

/**
 * The set of AccessorKind values that the EntityRef.*Via surface supports.
 * Any field whose kind is NOT in this set is filtered out of NavWrapper.fields
 * (and recorded in skippedKinds) so that emit-dts and emit-js always agree.
 * Kinds absent: "f64" (no readFloat64Via) and "str" (no readStringVia).
 */
export const SUPPORTED_NAV_KINDS = new Set<AccessorKind>([
  "i8", "i16", "i32", "u8", "u16", "u32", "f32", "bool",
  "u64", "i64", "handle", "vector", "qangle",
]);

/** Build a NavModel from a config array + the schema catalog.
 *  Reuses schemagen's buildModel + flattenedFields for the inheritance walk
 *  and propName-collision→raw handling.
 *  Fields whose kind is not in SUPPORTED_NAV_KINDS are filtered out so both
 *  emitters (emit-js and emit-dts) see an identical field list. */
export function buildNavModel(config: NavConfigEntry[], catalog: Catalog): NavModel {
  // Collect all distinct target classes needed.
  const targetClasses = [...new Set(config.map(e => e.target))];
  // Build the schema model for those classes (includes ancestor chains).
  const schemaModel = buildModel(catalog, targetClasses);

  // Build one NavWrapper per config entry, sorted by wrapper name for determinism.
  const sorted = [...config].sort((a, b) => a.wrapper < b.wrapper ? -1 : a.wrapper > b.wrapper ? 1 : 0);
  const wrappers: NavWrapper[] = sorted.map(entry => {
    const all = flattenedFields(schemaModel, entry.target);
    const fields: FieldDescriptor[] = [];
    const skippedKinds: { propName: string; accessorKind: AccessorKind }[] = [];
    for (const f of all) {
      if (SUPPORTED_NAV_KINDS.has(f.accessorKind)) {
        fields.push(f);
      } else {
        skippedKinds.push({ propName: f.propName, accessorKind: f.accessorKind });
      }
    }
    return {
      wrapper: entry.wrapper,
      prop: entry.prop,
      source: entry.source,
      target: entry.target,
      path: entry.path,
      fields,
      skippedKinds,
    };
  });

  return { wrappers };
}
