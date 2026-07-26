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
  /**
   * Hops whose offsets are SUMMED into the final field offset instead of being dereferenced —
   * i.e. structs embedded inline in the target rather than pointed to from it.
   *
   * `CCSPlayerController_ActionTrackingServices::m_matchStats` is one: the match stats live INSIDE
   * the services object, so reaching `m_iKills` is one pointer hop (to the services) plus a base
   * offset (to the stats block) plus the field. Without this a target could only expose fields
   * declared on itself or an ancestor, and anything behind an embedded struct was unreachable.
   */
  base?: NavHop[];
  /**
   * Raw field names (`m_flMaxspeed`) that get a SETTER as well as a getter. Opt-in per field,
   * deliberately: which byte a field lives at is regenerable layout, but whether writing it is
   * SAFE is a behavioural fact that belongs in reviewed config. Blanket-honouring the catalog's
   * `writable` flag would expose engine bookkeeping (`m_nTraceCount`, `m_bInStuckTest`) whose
   * mutation is undefined behaviour. Absent ⇒ the wrapper stays entirely read-only.
   */
  writable?: string[];
}

/** A nav field plus whether THIS wrapper opted it into write-through (see NavConfigEntry.writable). */
export type NavField = FieldDescriptor & { navWritable: boolean };

export interface NavWrapper {
  wrapper: string;
  prop: string;
  source: string;
  target: string;
  path: NavHop[];
  base: NavHop[];
  fields: NavField[];
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

/**
 * Kinds a nav SETTER can be emitted for — i.e. the `EntityRef.write*Via` chain-write surface that
 * actually exists (`core/src/v8host.rs`). Deliberately narrower than the read surface: the other
 * writers were never added. An allowlisted field of any other kind is a GENERATION-TIME ERROR
 * rather than a silently-dropped setter, so a missing native is discovered at build time by
 * whoever asked for it, not at runtime by a plugin author whose assignment did nothing.
 */
export const WRITABLE_NAV_KINDS = new Set<AccessorKind>(["f32", "bool", "i32"]);

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
    const wantWritable = new Set(entry.writable ?? []);
    const fields: NavField[] = [];
    const skippedKinds: { propName: string; accessorKind: AccessorKind }[] = [];
    for (const f of all) {
      if (SUPPORTED_NAV_KINDS.has(f.accessorKind)) {
        fields.push({ ...f, navWritable: wantWritable.has(f.rawName) });
      } else {
        skippedKinds.push({ propName: f.propName, accessorKind: f.accessorKind });
      }
    }

    // Fail generation on a bad allowlist rather than emitting nothing. Both of these are silent
    // no-ops otherwise, and both are exactly what a CS2 update produces: a renamed field, or a
    // field whose type changed to one with no chain writer.
    for (const raw of wantWritable) {
      const f = fields.find(x => x.rawName === raw);
      if (!f) {
        const known = all.some(x => x.rawName === raw);
        throw new Error(
          `nav-targets: ${entry.wrapper}.writable lists ${JSON.stringify(raw)}, which ` +
          (known
            ? `exists on ${entry.target} but has an unsupported accessor kind, so it has no nav accessor at all.`
            : `does not exist on ${entry.target} or any of its ancestors (renamed or removed?).`));
      }
      if (!WRITABLE_NAV_KINDS.has(f.accessorKind)) {
        throw new Error(
          `nav-targets: ${entry.wrapper}.writable lists ${JSON.stringify(raw)} (kind ` +
          `'${f.accessorKind}'), but there is no EntityRef.write*Via for that kind. Add the ` +
          `native and extend WRITABLE_NAV_KINDS, or drop the field from the allowlist.`);
      }
    }
    return {
      wrapper: entry.wrapper,
      prop: entry.prop,
      source: entry.source,
      target: entry.target,
      path: entry.path,
      base: entry.base ?? [],
      fields,
      skippedKinds,
    };
  });

  return { wrappers };
}
