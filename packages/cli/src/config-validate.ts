/** Validate an s2script.config block: each entry is { type, default, description? } and `default`
 *  matches `type`. Returns human error messages (empty = valid). Types: string/int/float/bool. */
export function validateConfigBlock(config: unknown): string[] {
  const errs: string[] = [];
  if (config == null) return errs;
  if (typeof config !== "object" || Array.isArray(config)) return ["s2script.config must be an object"];
  for (const [key, raw] of Object.entries(config as Record<string, unknown>)) {
    if (typeof raw !== "object" || raw === null) { errs.push(`config '${key}': must be { type, default }`); continue; }
    const decl = raw as { type?: unknown; default?: unknown };
    const t = decl.type, d = decl.default;
    if (t !== "string" && t !== "int" && t !== "float" && t !== "bool") {
      errs.push(`config '${key}': unknown type ${JSON.stringify(t)} (want string|int|float|bool)`); continue;
    }
    const ok =
      (t === "string" && typeof d === "string") ||
      (t === "int" && typeof d === "number" && Number.isInteger(d)) ||
      (t === "float" && typeof d === "number") ||
      (t === "bool" && typeof d === "boolean");
    if (!ok) errs.push(`config '${key}': default ${JSON.stringify(d)} does not match type '${t}'`);
  }
  return errs;
}
