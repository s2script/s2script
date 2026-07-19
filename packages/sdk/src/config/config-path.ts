/** Sanitize a plugin id to its `configs/` filename — byte-for-byte identical to the runtime's
 *  ConfigPath (shim/src/s2script_mm.cpp) and the CLI's .s2sp id sanitization: every char NOT in
 *  [A-Za-z0-9._-] becomes '_', then '.json' is appended.
 *
 *  e.g. "@s2script/funvotes" -> "_s2script_funvotes.json", "a/b c" -> "a_b_c.json".
 *
 *  This MUST match the shim exactly — a mismatch means the generated default file lands at a name the
 *  runtime never reads, so the operator sees no config. */
export function configFileName(id: string): string {
  return id.replace(/[^A-Za-z0-9._-]/g, "_") + ".json";
}
