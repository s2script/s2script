// Pure model: nav-targets config + catalog → normalized nav wrappers with flattened readable fields.
// No I/O, no Date/random — deterministic. Reuses schemagen's buildModel + flattenedFields.

import { buildModel, flattenedFields, type Catalog, type FieldDescriptor } from "../schemagen/model.ts";

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
}

export interface NavModel {
  wrappers: NavWrapper[];
}

/** Build a NavModel from a config array + the schema catalog.
 *  Reuses schemagen's buildModel + flattenedFields for the inheritance walk
 *  and propName-collision→raw handling. */
export function buildNavModel(config: NavConfigEntry[], catalog: Catalog): NavModel {
  // Collect all distinct target classes needed.
  const targetClasses = [...new Set(config.map(e => e.target))];
  // Build the schema model for those classes (includes ancestor chains).
  const schemaModel = buildModel(catalog, targetClasses);

  // Build one NavWrapper per config entry, sorted by wrapper name for determinism.
  const sorted = [...config].sort((a, b) => a.wrapper < b.wrapper ? -1 : a.wrapper > b.wrapper ? 1 : 0);
  const wrappers: NavWrapper[] = sorted.map(entry => {
    const fields = flattenedFields(schemaModel, entry.target);
    return {
      wrapper: entry.wrapper,
      prop: entry.prop,
      source: entry.source,
      target: entry.target,
      path: entry.path,
      fields,
    };
  });

  return { wrappers };
}
