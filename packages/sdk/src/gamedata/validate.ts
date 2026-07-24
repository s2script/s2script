import {
  ARG_KINDS, RET_KINDS, INT_CLASS_ARGS, MAX_INT_ARGS, MAX_FLOAT_ARGS, PLATFORM,
  type ArgKind, type PluginGamedata,
} from "./types.ts";

const ALLOWED_SECTIONS = new Set(["signatures", "calls"]);

/**
 * A call name is interpolated VERBATIM into the generated `.s2script/gamedata.d.ts` as a TypeScript
 * interface member, so it must be a plain identifier. This is a security gate, not cosmetics: a name
 * like `y: (...a: any[]) => any; [k: string]: any; z` injects an index signature into `EngineCalls`,
 * after which EVERY `Engine.call("anything")` typechecks and the build gate is completely defeated —
 * while each such call degrades to `null` at runtime, so the failure is silent. Benign-but-invalid
 * names (`burn-target`, `0abc`) emit syntactically invalid TypeScript, which surfaces as a confusing
 * `tsc` error inside a generated file instead of a gamedata error here.
 */
const IDENTIFIER = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
/** Names that are legal identifiers but hazardous as interface members / map keys. */
const RESERVED_CALL_NAMES = new Set(["constructor", "prototype", "__proto__"]);

/**
 * The resolver steps the shim actually dispatches on (`engine_calls.cpp` — `direct` is the default,
 * then `ctor-body-xref` and `lea-disp`). Closed here so a typo like "drect" is a BUILD error naming
 * the valid set, rather than a descriptor that resolves to nothing at load and degrades silently.
 */
const RESOLVE_KINDS = ["direct", "ctor-body-xref", "lea-disp"] as const;

/** Permissions the runtime understands. Closed for the same reason as RESOLVE_KINDS. */
const KNOWN_PERMISSIONS = ["engine:calls"] as const;

/** Validate a plugin's gamedata. Returns [] when valid; every string is a build-blocking error. */
export function validatePluginGamedata(gd: unknown, opts: { permissions: string[] }): string[] {
  const errs: string[] = [];
  if (gd == null || typeof gd !== "object" || Array.isArray(gd)) return ["gamedata must be an object"];
  const g = gd as Record<string, unknown>;

  for (const key of Object.keys(g)) {
    if (!ALLOWED_SECTIONS.has(key)) {
      errs.push(`gamedata section '${key}' is not supported in v1 (allowed: signatures, calls)`);
    }
  }

  const sigs = (g.signatures ?? {}) as Record<string, Record<string, unknown>>;
  if (typeof sigs !== "object" || Array.isArray(sigs)) errs.push("signatures must be an object");
  for (const [name, platforms] of Object.entries(sigs)) {
    const p = (platforms as Record<string, unknown>)?.[PLATFORM] as Record<string, unknown> | undefined;
    if (!p) { errs.push(`signature '${name}': missing platform '${PLATFORM}'`); continue; }
    for (const f of ["module", "pattern", "resolve"]) {
      if (typeof p[f] !== "string" || !(p[f] as string).length) {
        errs.push(`signature '${name}': '${f}' must be a non-empty string`);
      }
    }
    if (typeof p.resolve === "string" && p.resolve.length &&
        !(RESOLVE_KINDS as readonly string[]).includes(p.resolve)) {
      errs.push(
        `signature '${name}': unknown resolve step ${JSON.stringify(p.resolve)} ` +
          `(allowed: ${RESOLVE_KINDS.join(", ")})`
      );
    }
  }

  // Declared permissions are part of the manifest contract, so a typo here silently grants nothing.
  for (const perm of opts.permissions) {
    if (!(KNOWN_PERMISSIONS as readonly string[]).includes(perm)) {
      errs.push(`unknown permission ${JSON.stringify(perm)} (allowed: ${KNOWN_PERMISSIONS.join(", ")})`);
    }
  }

  const calls = (g.calls ?? {}) as Record<string, unknown>;
  const callNames = Object.keys(calls);
  if (callNames.length && !opts.permissions.includes("engine:calls")) {
    errs.push('gamedata declares a `calls` section but the manifest does not declare permission "engine:calls"');
  }

  for (const [name, rawDecl] of Object.entries(calls)) {
    const where = `call '${name}'`;

    // Name gate FIRST — see the IDENTIFIER note above. A bad name is never emitted, so a malicious or
    // malformed key can neither inject into the generated .d.ts nor produce invalid TypeScript.
    if (!IDENTIFIER.test(name)) {
      errs.push(
        `${where}: call name must be a plain identifier matching ${IDENTIFIER.source} — it is emitted ` +
          `verbatim as a TypeScript interface member in the generated .s2script/gamedata.d.ts`
      );
      continue;
    }
    if (RESERVED_CALL_NAMES.has(name)) {
      errs.push(`${where}: call name '${name}' is reserved`);
      continue;
    }

    if (rawDecl == null || typeof rawDecl !== "object") { errs.push(`${where}: must be an object`); continue; }
    const decl = rawDecl as Record<string, unknown>;

    // receiver
    const recv = decl.receiver as Record<string, unknown> | undefined;
    if (!recv || recv.kind !== "entity") {
      errs.push(`${where}: receiver.kind must be "entity" in v1`);
    } else if (recv.via !== undefined) {
      const via = recv.via as Record<string, unknown>;
      if (typeof via?.class !== "string" || typeof via?.field !== "string") {
        errs.push(`${where}: receiver.via requires both 'class' and 'field' strings`);
      }
    }

    // target
    const target = decl.target as Record<string, unknown> | undefined;
    if (!target) {
      errs.push(`${where}: missing 'target'`);
    } else if (target.kind === "signature") {
      if (typeof target.name !== "string") errs.push(`${where}: target.name must be a string`);
      else if (!(target.name in sigs)) errs.push(`${where}: target.name '${target.name}' has no entry in 'signatures'`);
    } else if (target.kind === "vtable") {
      if (typeof target.class !== "string") errs.push(`${where}: vtable target requires 'class'`);
      const plat = target[PLATFORM] as Record<string, unknown> | undefined;
      if (!plat) {
        errs.push(`${where}: vtable target missing platform '${PLATFORM}'`);
      } else {
        if (!Number.isInteger(plat.index) || (plat.index as number) < 0) {
          errs.push(`${where}: vtable index must be a non-negative integer`);
        }
        const prologue = (plat.validate as Record<string, unknown> | undefined)?.prologue;
        if (typeof prologue !== "string" || !prologue.length) {
          errs.push(`${where}: a vtable target REQUIRES validate.prologue (a bare borrowed index is never trusted)`);
        }
      }
    } else {
      errs.push(`${where}: target.kind must be "signature" or "vtable"`);
    }

    // args
    const args = decl.args;
    if (!Array.isArray(args)) {
      errs.push(`${where}: 'args' must be an array`);
    } else {
      let ints = 0, floats = 0;
      for (const a of args) {
        if (typeof a !== "string" || !ARG_KINDS.includes(a as ArgKind)) {
          errs.push(`${where}: unknown arg kind ${JSON.stringify(a)} (allowed: ${ARG_KINDS.join(", ")})`);
          continue;
        }
        if (INT_CLASS_ARGS.has(a as ArgKind)) ints++; else floats++;
      }
      if (ints > MAX_INT_ARGS) {
        errs.push(`${where}: ${ints} integer-class args exceeds the max of ${MAX_INT_ARGS} (\`this\` occupies rdi)`);
      }
      if (floats > MAX_FLOAT_ARGS) {
        errs.push(`${where}: ${floats} float args exceeds the max of ${MAX_FLOAT_ARGS}`);
      }

      // Optional argNames — documentary, but still emitted into the generated .d.ts, so it gets the
      // same identifier discipline as the call name itself (see the IDENTIFIER note above).
      if (decl.argNames !== undefined) {
        const names = decl.argNames;
        if (!Array.isArray(names)) {
          errs.push(`${where}: 'argNames' must be an array of parameter names`);
        } else if (names.length !== args.length) {
          errs.push(
            `${where}: 'argNames' has ${names.length} entr${names.length === 1 ? "y" : "ies"} but ` +
              `'args' has ${args.length} — they are positionally matched`
          );
        } else {
          const seen = new Set<string>();
          for (const nm of names) {
            if (typeof nm !== "string" || !IDENTIFIER.test(nm)) {
              errs.push(
                `${where}: argName ${JSON.stringify(nm)} must be a plain identifier matching ` +
                  `${IDENTIFIER.source} — it is emitted as a parameter name in the generated .d.ts`
              );
              continue;
            }
            // `self` is the generated receiver parameter; reusing it would emit a duplicate name.
            if (nm === "self") {
              errs.push(`${where}: argName 'self' is reserved (it names the receiver parameter)`);
              continue;
            }
            if (seen.has(nm)) errs.push(`${where}: duplicate argName ${JSON.stringify(nm)}`);
            seen.add(nm);
          }
        }
      }
    }

    // returns
    if (typeof decl.returns !== "string" || !RET_KINDS.includes(decl.returns as never)) {
      errs.push(`${where}: 'returns' must be one of ${RET_KINDS.join(", ")}`);
    }
  }

  return errs;
}
