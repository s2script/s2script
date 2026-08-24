import {
  ARG_KINDS, RET_KINDS, INT_CLASS_ARGS, MAX_INT_ARGS, MAX_FLOAT_ARGS, SUPPORTED_PLATFORMS,
  HOOK_SHAPES, hookShapeArity,
  type ArgKind, type PluginGamedata, type SupportedPlatform,
} from "./types.ts";

const ALLOWED_SECTIONS = new Set(["signatures", "calls", "hooks"]);

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

/**
 * Permissions the runtime understands. Closed for the same reason as RESOLVE_KINDS.
 *
 * `engine:hooks` is separate from `engine:calls`: an operator who granted outbound calls has not
 * granted inbound detours. Core already enforces this; the build list must match so a typo is a
 * build error rather than a permission the runtime silently ignores.
 */
const KNOWN_PERMISSIONS = ["engine:calls", "engine:hooks"] as const;

function supportedEntries(
  value: Record<string, unknown> | undefined,
): Array<[SupportedPlatform, Record<string, unknown>]> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  const entries: Array<[SupportedPlatform, Record<string, unknown>]> = [];
  for (const platform of SUPPORTED_PLATFORMS) {
    const candidate = value[platform];
    if (candidate != null && typeof candidate === "object" && !Array.isArray(candidate)) {
      entries.push([platform, candidate as Record<string, unknown>]);
    }
  }
  return entries;
}

/** Validate a plugin's gamedata. Returns [] when valid; every string is a build-blocking error. */
export function validatePluginGamedata(gd: unknown, opts: { permissions: string[] }): string[] {
  const errs: string[] = [];
  if (gd == null || typeof gd !== "object" || Array.isArray(gd)) return ["gamedata must be an object"];
  const g = gd as Record<string, unknown>;

  for (const key of Object.keys(g)) {
    if (!ALLOWED_SECTIONS.has(key)) {
      errs.push(`gamedata section '${key}' is not supported in v1 (allowed: signatures, calls, hooks)`);
    }
  }

  const sigs = (g.signatures ?? {}) as Record<string, Record<string, unknown>>;
  if (typeof sigs !== "object" || Array.isArray(sigs)) errs.push("signatures must be an object");
  for (const [name, platforms] of Object.entries(sigs)) {
    const entries = supportedEntries(platforms as Record<string, unknown>);
    if (!entries.length) {
      errs.push(
        `signature '${name}': requires at least one supported platform ` +
          `(${SUPPORTED_PLATFORMS.join(", ")})`,
      );
      continue;
    }
    for (const [platform, p] of entries) {
      for (const f of ["module", "pattern", "resolve"]) {
        if (typeof p[f] !== "string" || !(p[f] as string).length) {
          errs.push(`signature '${name}' platform '${platform}': '${f}' must be a non-empty string`);
        }
      }
      if (typeof p.resolve === "string" && p.resolve.length &&
          !(RESOLVE_KINDS as readonly string[]).includes(p.resolve)) {
        errs.push(
          `signature '${name}' platform '${platform}': unknown resolve step ${JSON.stringify(p.resolve)} ` +
            `(allowed: ${RESOLVE_KINDS.join(", ")})`
        );
      }
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
    if (!recv || (recv.kind !== "entity" && recv.kind !== "none")) {
      errs.push(`${where}: receiver.kind must be "entity" or "none"`);
    } else if (recv.kind === "none") {
      // A static/free function has no `this` to hop from, so `via` is contradictory rather than
      // merely redundant — reject it instead of ignoring it.
      if (recv.via !== undefined) {
        errs.push(`${where}: receiver.kind "none" cannot carry a 'via' sub-object hop`);
      }
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
      const entries = supportedEntries(target);
      if (!entries.length) {
        errs.push(
          `${where}: vtable target requires at least one supported platform ` +
            `(${SUPPORTED_PLATFORMS.join(", ")})`,
        );
      }
      for (const [platform, plat] of entries) {
        if (!Number.isInteger(plat.index) || (plat.index as number) < 0) {
          errs.push(`${where} platform '${platform}': vtable index must be a non-negative integer`);
        }
        const prologue = (plat.validate as Record<string, unknown> | undefined)?.prologue;
        if (typeof prologue !== "string" || !prologue.length) {
          errs.push(
            `${where} platform '${platform}': a vtable target REQUIRES validate.prologue ` +
              `(a bare borrowed index is never trusted)`,
          );
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
        errs.push(`${where}: ${ints} integer-class args exceeds the max of ${MAX_INT_ARGS}`);
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

  validateHooks(g, sigs, callNames, opts.permissions, errs);
  return errs;
}

function isNonEmptyObject(v: unknown): v is Record<string, unknown> {
  return v != null && typeof v === "object" && !Array.isArray(v) && Object.keys(v as object).length > 0;
}

/** Microsoft x64 positions 0..3 have paired GP/XMM registers; position 4+ is stack-only. */
function hookHasWindowsStackInteger(shape: string): boolean {
  if (!(HOOK_SHAPES as readonly string[]).includes(shape) || shape === "this_void") return false;
  const args = shape.slice("this_".length).split("_");
  return args.slice(3).some((kind) => kind !== "f32");
}

function hasPrologue(validate: unknown): boolean {
  const prologue = (validate as Record<string, unknown> | undefined)?.prologue;
  return typeof prologue === "string" && prologue.length > 0;
}

/** Grammar for `hooks` — mirrors `scripts/check-call-descriptors.sh` §3b and core `prepare()`. */
function validateHooks(
  g: Record<string, unknown>,
  sigs: Record<string, Record<string, unknown>>,
  callNames: string[],
  permissions: string[],
  errs: string[],
): void {
  if (g.hooks === undefined) return;
  if (g.hooks == null || typeof g.hooks !== "object" || Array.isArray(g.hooks)) {
    errs.push("hooks must be an object");
    return;
  }
  const hooks = g.hooks as Record<string, unknown>;
  const hookNames = Object.keys(hooks);
  if (hookNames.length && !permissions.includes("engine:hooks")) {
    errs.push('gamedata declares a `hooks` section but the manifest does not declare permission "engine:hooks"');
  }

  for (const [name, rawDecl] of Object.entries(hooks)) {
    const where = `hook '${name}'`;

    if (!IDENTIFIER.test(name)) {
      errs.push(
        `${where}: hook name must be a plain identifier matching ${IDENTIFIER.source} — it is emitted ` +
          `verbatim as a TypeScript interface member in the generated .s2script/hooks.d.ts`
      );
      continue;
    }
    if (RESERVED_CALL_NAMES.has(name)) {
      errs.push(`${where}: hook name '${name}' is reserved`);
      continue;
    }

    if (rawDecl == null || typeof rawDecl !== "object" || Array.isArray(rawDecl)) {
      errs.push(`${where}: must be an object`);
      continue;
    }
    const decl = rawDecl as Record<string, unknown>;

    const ctx = (decl.expose as Record<string, unknown> | undefined)?.ctx;
    if (typeof ctx !== "string" || !ctx.length) {
      errs.push(`${where}: missing 'expose.ctx' — nothing could subscribe to this hook`);
    } else if (!IDENTIFIER.test(ctx)) {
      errs.push(`${where}: 'expose.ctx' must be a plain identifier, got ${JSON.stringify(ctx)}`);
    }

    const shape = decl.shape;
    if (typeof shape !== "string" || !(HOOK_SHAPES as readonly string[]).includes(shape)) {
      errs.push(
        `${where}: unknown hook shape ${JSON.stringify(shape)} (this build compiles thunks for: ${HOOK_SHAPES.join(", ")})`
      );
    }

    const params = decl.params ?? [];
    if (!Array.isArray(params)) {
      errs.push(`${where}: 'params' must be an array`);
    } else {
      const seen = new Set<string>();
      for (let i = 0; i < params.length; i++) {
        const p = params[i];
        if (typeof p !== "string" || !IDENTIFIER.test(p)) {
          errs.push(`${where}: params[${i}] = ${JSON.stringify(p)} must be a plain identifier`);
          continue;
        }
        if (seen.has(p)) errs.push(`${where}: param name ${JSON.stringify(p)} is declared more than once`);
        seen.add(p);
      }
      const arity = typeof shape === "string" ? hookShapeArity(shape) : undefined;
      if (arity !== undefined && params.length > arity) {
        errs.push(
          `${where}: declares ${params.length} 'params' but shape ${JSON.stringify(shape)} passes only ${arity}`
        );
      }

      if (decl.mutable !== undefined) {
        const mutable = decl.mutable;
        if (!Array.isArray(mutable)) {
          errs.push(`${where}: 'mutable' must be an array`);
        } else {
          for (const m of mutable) {
            if (typeof m !== "string" || !params.includes(m)) {
              errs.push(`${where}: 'mutable' names ${JSON.stringify(m)}, which is not one of this hook's params`);
            }
          }
        }
      }
    }

    const recv = decl.receiver as Record<string, unknown> | undefined;
    if (recv !== undefined) {
      if (recv == null || typeof recv !== "object") {
        errs.push(`${where}: 'receiver' must be an object`);
      } else {
        const rkind = recv.kind ?? "none";
        if (rkind !== "none" && rkind !== "entity") {
          errs.push(`${where}: receiver.kind must be "none" or "entity"`);
        } else if (rkind === "entity") {
          if (typeof recv.as !== "string" || !IDENTIFIER.test(recv.as)) {
            errs.push(`${where}: receiver.kind "entity" needs a plain-identifier 'as' name`);
          } else if (Array.isArray(params) && params.includes(recv.as)) {
            errs.push(`${where}: receiver 'as' name ${JSON.stringify(recv.as)} collides with a param name`);
          }
        }
      }
    }

    if (decl.bypassWith !== undefined) {
      if (typeof decl.bypassWith !== "string" || !decl.bypassWith.length) {
        errs.push(`${where}: 'bypassWith' must be a non-empty call name`);
      } else if (!callNames.includes(decl.bypassWith)) {
        errs.push(
          `${where}: 'bypassWith' names ${JSON.stringify(decl.bypassWith)}, which is not a 'calls' descriptor in this owner's gamedata`
        );
      }
    }

    const target = decl.target as Record<string, unknown> | undefined;
    if (!target) {
      errs.push(`${where}: missing 'target'`);
      continue;
    }
    if (target.kind === "signature") {
      if (typeof target.name !== "string") {
        errs.push(`${where}: target.name must be a string`);
      } else if (!(target.name in sigs)) {
        errs.push(`${where}: target.name '${target.name}' has no entry in 'signatures'`);
      }
      const signaturePlatforms = typeof target.name === "string"
        ? supportedEntries(sigs[target.name] as Record<string, unknown> | undefined)
        : [];
      const missingValidators = signaturePlatforms
        .filter(([, spec]) => !isNonEmptyObject(spec.validate))
        .map(([platform]) => platform);
      if (!isNonEmptyObject(target.validate) && missingValidators.length) {
        errs.push(
          `${where}: a hook target MUST carry a non-empty 'validate' (inline, or inherited for every ` +
            `platform; missing: ${missingValidators.join(", ")}) — a wrong detour address overwrites the ` +
            `prologue of whatever is actually there`,
        );
      }
      const windowsSpec = typeof target.name === "string"
        ? (sigs[target.name]?.windows64 as Record<string, unknown> | undefined)
        : undefined;
      // flatten_decl treats an inline target.validate as a whole-object override. Do not combine an
      // inherited prologue with an inline non-prologue validator here: runtime will not combine them.
      const effectiveWindowsValidate = Object.hasOwn(target, "validate")
        ? target.validate
        : windowsSpec?.validate;
      if (windowsSpec && typeof shape === "string" && hookHasWindowsStackInteger(shape) &&
          !hasPrologue(effectiveWindowsValidate)) {
        errs.push(
          `${where} platform 'windows64': shape ${JSON.stringify(shape)} has a stack-position ` +
            `integer argument whose width cannot be inferred from entry registers; it REQUIRES ` +
            `validate.prologue to identify the exact target before installing the hook`,
        );
      }
    } else if (target.kind === "vtable") {
      if (typeof target.class !== "string") errs.push(`${where}: vtable target requires 'class'`);
      const entries = supportedEntries(target);
      if (!entries.length) {
        errs.push(
          `${where}: vtable target requires at least one supported platform ` +
            `(${SUPPORTED_PLATFORMS.join(", ")})`,
        );
      }
      for (const [platform, plat] of entries) {
        if (!Number.isInteger(plat.index) || (plat.index as number) < 0) {
          errs.push(`${where} platform '${platform}': vtable index must be a non-negative integer`);
        }
        const prologue = (plat.validate as Record<string, unknown> | undefined)?.prologue;
        if (typeof prologue !== "string" || !prologue.length) {
          errs.push(
            `${where} platform '${platform}': a vtable target REQUIRES validate.prologue ` +
              `(a bare borrowed index is never trusted)`,
          );
        }
      }
    } else {
      errs.push(`${where}: target.kind must be "signature" or "vtable"`);
    }
  }
}
