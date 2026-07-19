/** Validate an s2script.config block.
 *
 *  Each entry is either a value DECL or a SECTION. Classification (shared verbatim with the core's
 *  `#[serde(untagged)] ConfigEntry`): an object with a string-valued `type` key is a DECL; any other
 *  object is a SECTION (a nested map of further entries, recursed). A DECL's `type` must then be one
 *  of string|int|float|bool (an unknown type string is a decl with an error, matching the core, which
 *  reads it as a decl and degrades at materialize — never a section).
 *
 *  Decl rules:
 *   - a decl (or section) key may not contain '.' — dotted names are reserved for a section walk;
 *   - `type` must be one of string|int|float|bool;
 *   - `default` must be present and match `type`;
 *   - `min`/`max` apply to int|float only and are mutually exclusive with `enum`;
 *   - `enum` applies to string|int only, must be a non-empty array of type-matching values, and the
 *     `default` must be one of them;
 *   - a numeric `default` must sit within `min`/`max`;
 *   - `sensitive`, if present, must be a boolean (masked in registry display, still written to file);
 *   - `group`/`label`/`description`, if present, must be strings.
 *
 *  Returns human error messages (empty = valid). */

const TYPES = ["string", "int", "float", "bool"] as const;
type DeclType = (typeof TYPES)[number];

function isDeclType(t: unknown): t is DeclType {
  return t === "string" || t === "int" || t === "float" || t === "bool";
}

function matchesType(v: unknown, t: DeclType): boolean {
  switch (t) {
    case "string": return typeof v === "string";
    case "int": return typeof v === "number" && Number.isInteger(v);
    case "float": return typeof v === "number";
    case "bool": return typeof v === "boolean";
  }
}

export function validateConfigBlock(config: unknown): string[] {
  const errs: string[] = [];
  if (config == null) return errs;
  validateEntries(config, "", errs);
  return errs;
}

function validateEntries(node: unknown, prefix: string, errs: string[]): void {
  if (typeof node !== "object" || node === null || Array.isArray(node)) {
    errs.push(`${prefix ? `config '${prefix}'` : "s2script.config"} must be an object`);
    return;
  }
  for (const [key, raw] of Object.entries(node as Record<string, unknown>)) {
    const full = prefix ? `${prefix}.${key}` : key;
    if (key.includes(".")) {
      errs.push(`config '${full}': key must not contain '.' (dotted names are reserved for section access)`);
      continue;
    }
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
      errs.push(`config '${full}': must be a declaration { type, default } or a section object`);
      continue;
    }
    const obj = raw as Record<string, unknown>;
    // A string-valued `type` key ⇒ a value decl; anything else ⇒ a section (recurse). Mirrors the
    // core's untagged Decl-vs-Section discrimination (Decl requires a string `type`).
    if (typeof obj.type === "string") {
      validateDecl(full, obj, obj.type, errs);
    } else {
      validateEntries(obj, full, errs);
    }
  }
}

function validateDecl(full: string, decl: Record<string, unknown>, typeStr: string, errs: string[]): void {
  if (!isDeclType(typeStr)) {
    errs.push(`config '${full}': unknown type ${JSON.stringify(typeStr)} (want string|int|float|bool)`);
    return; // no known type → the remaining checks are meaningless
  }
  const t: DeclType = typeStr;

  // default present + matches type
  if (!("default" in decl)) {
    errs.push(`config '${full}': missing 'default'`);
  } else if (!matchesType(decl.default, t)) {
    errs.push(`config '${full}': default ${JSON.stringify(decl.default)} does not match type '${t}'`);
  }

  const hasMin = decl.min !== undefined;
  const hasMax = decl.max !== undefined;
  const hasEnum = decl.enum !== undefined;

  // enum XOR min/max
  if (hasEnum && (hasMin || hasMax)) {
    errs.push(`config '${full}': 'enum' is mutually exclusive with 'min'/'max'`);
  }

  // min/max: numeric types only, numeric bounds, default within range
  if (hasMin || hasMax) {
    if (t !== "int" && t !== "float") {
      errs.push(`config '${full}': 'min'/'max' apply to int|float only (type '${t}')`);
    }
    if (hasMin && typeof decl.min !== "number") errs.push(`config '${full}': 'min' must be a number`);
    if (hasMax && typeof decl.max !== "number") errs.push(`config '${full}': 'max' must be a number`);
    if (typeof decl.min === "number" && typeof decl.max === "number" && decl.min > decl.max) {
      errs.push(`config '${full}': 'min' (${decl.min}) exceeds 'max' (${decl.max})`);
    }
    if (typeof decl.default === "number") {
      if (typeof decl.min === "number" && decl.default < decl.min) errs.push(`config '${full}': default ${decl.default} is below min ${decl.min}`);
      if (typeof decl.max === "number" && decl.default > decl.max) errs.push(`config '${full}': default ${decl.default} is above max ${decl.max}`);
    }
  }

  // enum: string|int only, non-empty array of type-matching values, default in it
  if (hasEnum) {
    if (t !== "string" && t !== "int") {
      errs.push(`config '${full}': 'enum' applies to string|int only (type '${t}')`);
    }
    if (!Array.isArray(decl.enum) || decl.enum.length === 0) {
      errs.push(`config '${full}': 'enum' must be a non-empty array`);
    } else {
      for (const e of decl.enum) {
        if (!matchesType(e, t)) errs.push(`config '${full}': enum value ${JSON.stringify(e)} does not match type '${t}'`);
      }
      if ("default" in decl && matchesType(decl.default, t) && !decl.enum.some((e) => e === decl.default)) {
        errs.push(`config '${full}': default ${JSON.stringify(decl.default)} is not one of enum`);
      }
    }
  }

  if (decl.sensitive !== undefined && typeof decl.sensitive !== "boolean") errs.push(`config '${full}': 'sensitive' must be a boolean`);
  if (decl.group !== undefined && typeof decl.group !== "string") errs.push(`config '${full}': 'group' must be a string`);
  if (decl.label !== undefined && typeof decl.label !== "string") errs.push(`config '${full}': 'label' must be a string`);
  if (decl.description !== undefined && typeof decl.description !== "string") errs.push(`config '${full}': 'description' must be a string`);
}
