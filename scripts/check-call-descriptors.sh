#!/usr/bin/env bash
# Fails if a shipped `calls` OR `hooks` descriptor is not WELL-FORMED against its grammar.
#
# WHY THIS GATE EXISTS. A descriptor is data, and the runtime's answer to bad data is — correctly —
# to degrade THAT descriptor with a named reason and keep the server up. That is the right runtime
# behaviour and the wrong authoring feedback: a typo'd arg kind (`"bool "`), a signature name that
# does not exist (`CCSPlayerController_Repawn`), a validator key spelled `vtable_member`, or a
# 5-arg descriptor for a 4-arg function all produce a server that boots green and a feature that is
# quietly gone. Nobody reads the boot log for a WARN they are not expecting.
#
# `hooks` (declarative-inbound-hooks slice, 2026-08-02) is folded into THIS gate rather than a
# sibling script: it is the INBOUND twin of `calls` — same per-directory scoping (a `bypassWith`
# resolves against the same scope's `calls` exactly as a `target.name` resolves against the same
# scope's `signatures`), same target-kind/validator grammar (`check_validate`, `TARGET_KINDS`,
# `RECEIVER_KINDS` are reused verbatim) — so a second copy of that machinery would be exactly the
# "third place for a closed set to drift" the vocabulary rule below already warns against. The one
# rule a hook has that a call does not — a validator is MANDATORY, never optional, because a wrong
# DETOUR address overwrites the prologue of whatever is actually there — is checked in its own
# block. (Task 5's review folded this gap into Task 6 as item ii: before this, a `hooks` typo — an
# unknown shape, a dangling `bypassWith`, a `mutable` entry absent from `params`, a missing
# `expose.ctx`, an absent `validate` — surfaced only as a boot-time WARN, never a CI failure.)
#
# Everything here is decidable WITHOUT the game binary, so it is decidable in CI. What needs the
# binary (does the pattern match? is it unique? does the validator pass?) stays at load, where it
# belongs — this gate exists so that by the time the loader runs, the only remaining question is
# about the BINARY, never about the JSON.
#
# THE VOCABULARY IS NOT HARDCODED HERE. Every closed set below is read out of the source that
# actually enforces it — the arg/return/receiver/target vocabularies and the SysV arity budget from
# core/src/gamedata_calls.rs, the hook shape vocabulary from core/src/gamedata_hooks.rs, the
# validator vocabulary from shim/src/call_validate.cpp. A gate with its own copy of a closed set is
# a third place for the set to drift; this one fails loudly if it cannot find the real one.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 - <<'PY'
import json, pathlib, re, sys

sys.path.insert(0, 'scripts/lib')
from jsonc import strip_jsonc_comments  # shared with the other gamedata gates — one stripper, one format

bad = []
notes = []

# ---------------------------------------------------------------------------------------------
# 1. Derive every closed set from the code that enforces it.
# ---------------------------------------------------------------------------------------------
CORE_SRC = pathlib.Path('core/src/gamedata_calls.rs').read_text()
VALIDATE_SRC = pathlib.Path('shim/src/call_validate.cpp').read_text()

def fn_body(src, opener, what):
    """The text of a top-level Rust fn, from its signature to the closing brace at column 0."""
    i = src.find(opener)
    if i < 0:
        bad.append(f'cannot find `{opener}` in core/src/gamedata_calls.rs — this gate cannot '
                   f'derive the {what} vocabulary')
        return ''
    j = src.find('\n}', i)
    return src[i:j] if j > 0 else src[i:]

# The arg vocabulary. `float` is deliberately NOT in gp_kind_of (it is the float REGISTER class, not
# a GP slot kind), so it is derived separately from the acceptance test in `prepare` — if that line
# ever changes shape this gate says so rather than silently narrowing the vocabulary.
ARG_GP_KINDS = set(re.findall(r'"([a-z]+)"', fn_body(CORE_SRC, 'pub(crate) fn gp_kind_of', 'arg')))
FLOAT_KIND = re.search(r'a\s*!=\s*"(\w+)"\s*&&\s*gp_kind_of', CORE_SRC)
if not FLOAT_KIND:
    bad.append('cannot find the float acceptance test (`a != "float" && gp_kind_of(a).is_none()`) '
               'in prepare() — this gate cannot derive the float-class arg kind')
ARG_KINDS = ARG_GP_KINDS | ({FLOAT_KIND.group(1)} if FLOAT_KIND else set())

RET_KINDS = set(re.findall(r'"([a-z]+)"', fn_body(CORE_SRC, 'pub(crate) fn ret_code', 'return')))

PREPARE = fn_body(CORE_SRC, 'fn prepare(', 'receiver')
RECEIVER_KINDS = set(re.findall(r'rkind\s*!=\s*"([a-z]+)"', PREPARE))

# Platform selection split the public wrapper from the implementation that owns the closed target
# vocabulary. Derive from that implementation so adding Windows does not silently empty the gate.
FLATTEN = fn_body(CORE_SRC, 'fn flatten_decl_for_platform(', 'target')
TARGET_KINDS = set(re.findall(r'^\s{8}"([a-z]+)" =>', FLATTEN, re.M))

def rust_const(name):
    m = re.search(r'const %s: usize = (\d+)' % name, CORE_SRC)
    if not m:
        bad.append(f'cannot find `const {name}` in core/src/gamedata_calls.rs — this gate cannot '
                   f'derive the SysV arity budget')
        return None
    return int(m.group(1))

MAX_GP_ARGS = rust_const('MAX_GP_ARGS')
MAX_FP_ARGS = rust_const('MAX_FP_ARGS')

m = re.search(r'const PLATFORM: &str = "([^"]+)"', CORE_SRC)
if not m:
    bad.append('cannot find `const PLATFORM` in core/src/gamedata_calls.rs')
PLATFORM = m.group(1) if m else None

m = re.search(r'kVocabulary\[\]\s*=\s*\{([^}]*)\}', VALIDATE_SRC)
VALIDATORS = set(re.findall(r'"([^"]+)"', m.group(1))) if m else set()
if not VALIDATORS:
    bad.append('no kVocabulary[] table found in shim/src/call_validate.cpp — this gate cannot tell '
               'which validators exist')

# The hook shape vocabulary — core/src/gamedata_hooks.rs's `SHAPES`, the closed set a `hooks.<name>.
# shape` value must belong to. Mirrors the shim's `kShapes` (scripts/check-hook-shapes.sh owns THAT
# cross-language diff); this gate only needs core's names to validate gamedata authoring.
HOOK_SRC = pathlib.Path('core/src/gamedata_hooks.rs').read_text()
m = re.search(r'pub\(crate\) const SHAPES: &\[\(&str, i32\)\] =\s*&\[(.*?)\];', HOOK_SRC, re.S)
HOOK_SHAPES = set(re.findall(r'\("([A-Za-z0-9_]+)"', m.group(1))) if m else set()
if not HOOK_SHAPES:
    bad.append('no SHAPES[] table found in core/src/gamedata_hooks.rs — this gate cannot derive the '
               'hook shape vocabulary')

# The ARITY each shape implies, derived FROM THE SHAPE NAME — never from a second table here.
#
# The name IS the ABI: `this_f32_i32_i32_i32` is `void(void* self, float, int, int, int)`, and
# `this_void` is `void(void* self)`. A lookup table in this script would be a FIFTH copy of a closed
# cross-language set (core's SHAPES, the shim's kShapes, the enum, the thunks) and would need its own
# drift gate — exactly the failure mode the "vocabulary is not hardcoded here" rule at the top of
# this file exists to prevent. A shape whose name does not follow the rule fails the gate LOUDLY
# rather than silently skipping its descriptors' arity check.
SHAPE_ARITY = {}
for _shape_name in sorted(HOOK_SHAPES):
    m = re.fullmatch(r'this_(void|[a-z0-9]+(?:_[a-z0-9]+)*)', _shape_name)
    if not m:
        bad.append(f'hook shape "{_shape_name}" does not follow the `this_void` / '
                   f'`this_<param>_<param>...` naming rule, so this gate cannot derive its arity '
                   f'from its name — rename the shape, or teach this derivation the new form')
        continue
    SHAPE_ARITY[_shape_name] = 0 if m.group(1) == 'void' else len(m.group(1).split('_'))

# `expect`'s byte cap, read from the validator that enforces it.
m = re.search(r'kMaxExpect\s*=\s*(\d+)', VALIDATE_SRC)
MAX_EXPECT = int(m.group(1)) if m else 256

for missing, what in ((ARG_GP_KINDS, 'arg'), (RET_KINDS, 'return'),
                      (RECEIVER_KINDS, 'receiver'), (TARGET_KINDS, 'target')):
    if not missing:
        bad.append(f'derived an EMPTY {what} vocabulary — the source shape this gate parses changed')

if bad:
    print('CALL DESCRIPTOR GATE COULD NOT RUN:', file=sys.stderr)
    for b in bad:
        print(f'  {b}', file=sys.stderr)
    sys.exit(1)

# ---------------------------------------------------------------------------------------------
# 2. Collect every shipped descriptor, scoped the way the runtime merges them.
#
# A `signature` target names an entry in the SAME merged view: for gamedata/<owner>/ that is the
# whole owner directory (shim/src/gamedata.cpp merges every listed file into one GameConfig); for a
# plugin it is that plugin's own gamedata dir. Scoping by containing directory covers both, and a
# name that only resolves across a scope boundary is exactly the bug this catches.
#
# custom/ is the operator override channel — hand-written, gitignored, never shipped — and the other
# gamedata gates skip it for the same reason.
# ---------------------------------------------------------------------------------------------
def is_custom(p):
    return 'custom' in p.parts[:-1]

scopes = {}   # dir -> {"calls": {(file, name): decl}, "signatures": {name: (file, spec)}, "hooks": {(file, name): decl}}
for root in (pathlib.Path('gamedata'), pathlib.Path('examples'), pathlib.Path('plugins')):
    if not root.is_dir():
        continue
    for p in sorted(root.rglob('*.json*')):
        if p.suffix not in ('.json', '.jsonc') or is_custom(p):
            continue
        try:
            data = json.loads(strip_jsonc_comments(p.read_text()))
        except Exception as e:
            # A file this gate cannot read is only its business if it declares calls/hooks — and it
            # cannot know that without reading it. Report, don't guess.
            if '"calls"' in p.read_text() or '"hooks"' in p.read_text():
                bad.append(f'{p}: declares `calls`/`hooks` but is not valid JSON — {e}')
            continue
        if not isinstance(data, dict) or not ({'calls', 'signatures', 'hooks'} & set(data)):
            continue
        sc = scopes.setdefault(p.parent, {'calls': {}, 'signatures': {}, 'hooks': {}})
        for name, spec in (data.get('signatures') or {}).items():
            sc['signatures'][name] = (p, spec)
        for name, decl in (data.get('calls') or {}).items():
            sc['calls'][(p, name)] = decl
        for name, decl in (data.get('hooks') or {}).items():
            sc['hooks'][(p, name)] = decl

# ---------------------------------------------------------------------------------------------
# 3. The grammar.
# ---------------------------------------------------------------------------------------------
def pattern_tokens(spec):
    return (spec or {}).get('pattern', '').split()

def check_validate(where, v, sig_spec=None):
    """`v` is a `validate` object (from a signature or an inline target). sig_spec, when given, is
    the platform spec whose byte pattern the validator's offsets are measured against."""
    if v is None:
        return
    if not isinstance(v, dict):
        bad.append(f'{where}: `validate` must be an object')
        return
    for key, payload in v.items():
        if key not in VALIDATORS:
            bad.append(f'{where}: unknown validator "{key}" — the closed vocabulary is '
                       f'{", ".join(sorted(VALIDATORS))}')
            continue
        if key == 'prologue':
            if not isinstance(payload, str) or not payload.strip():
                bad.append(f'{where}: validate.prologue must be a non-empty byte-pattern string')
            else:
                for t in payload.split():
                    if t != '?' and not re.fullmatch(r'[0-9A-Fa-f]{2}', t):
                        bad.append(f'{where}: validate.prologue token "{t}" is neither a hex byte '
                                   f'nor a `?` wildcard')
                        break
        elif key == 'vtable-member':
            if not isinstance(payload, str) or not payload.strip():
                bad.append(f'{where}: validate.vtable-member must be a non-empty class-name string')
            elif '\0' in payload:
                bad.append(f'{where}: validate.vtable-member class name contains an interior NUL')
        elif key == 'string-xref':
            if not isinstance(payload, dict):
                bad.append(f'{where}: validate.string-xref must be an object '
                           f'{{at, dispOff, instrLen, expect}}')
                continue
            nums = {}
            for k in ('at', 'dispOff', 'instrLen'):
                if not isinstance(payload.get(k), int) or isinstance(payload.get(k), bool):
                    bad.append(f'{where}: validate.string-xref.{k} must be an integer')
                else:
                    nums[k] = payload[k]
            exp = payload.get('expect')
            if not isinstance(exp, str) or not exp:
                bad.append(f'{where}: validate.string-xref.expect must be a non-empty string')
            elif len(exp.encode()) > MAX_EXPECT:
                bad.append(f'{where}: validate.string-xref.expect exceeds {MAX_EXPECT} bytes')
            elif '\0' in exp:
                bad.append(f'{where}: validate.string-xref.expect contains an interior NUL')
            if len(nums) == 3:
                at, disp, ilen = nums['at'], nums['dispOff'], nums['instrLen']
                if at < 0 or disp < 0 or ilen <= 0:
                    bad.append(f'{where}: validate.string-xref has a negative/zero offset')
                elif disp + 4 > ilen:
                    bad.append(f'{where}: validate.string-xref displacement (dispOff {disp} + 4) '
                               f'lies outside its own {ilen}-byte instruction')
                # THE SOUNDNESS INVARIANT, and the only place it can be checked. call_validate.cpp
                # reads four bytes at fn+at+dispOff and follows them; it cannot verify an
                # instruction even begins at `at`. What makes that sound is that the SIGNATURE
                # PATTERN pins the opcode bytes and wildcards only the displacement. That is a
                # static property of the JSON, so it is checked here, before shipping.
                elif sig_spec is not None and sig_spec.get('resolve') == 'direct':
                    toks = pattern_tokens(sig_spec)
                    if at + ilen > len(toks):
                        bad.append(f'{where}: validate.string-xref covers bytes {at}..{at + ilen} '
                                   f'but the signature pattern is only {len(toks)} bytes — the '
                                   f'instruction it reads is not pinned by the pattern')
                    else:
                        opcode = toks[at:at + disp]
                        dispb = toks[at + disp:at + disp + 4]
                        if any(t == '?' for t in opcode):
                            bad.append(f'{where}: validate.string-xref reads an instruction whose '
                                       f'opcode bytes ({" ".join(opcode)}) are wildcarded in the '
                                       f'pattern — the xref is not pinned to a known instruction')
                        if any(t != '?' for t in dispb):
                            bad.append(f'{where}: validate.string-xref\'s displacement bytes '
                                       f'({" ".join(dispb)}) are baked into the pattern — a '
                                       f'build-specific operand (see check-gamedata-sigs.sh)')

checked = 0
for scope, sc in sorted(scopes.items()):
    for (path, name), decl in sorted(sc['calls'].items(), key=lambda kv: (str(kv[0][0]), kv[0][1])):
        checked += 1
        where = f'{path}: calls.{name}'
        if not isinstance(decl, dict):
            bad.append(f'{where}: descriptor must be an object')
            continue

        # --- receiver ---------------------------------------------------------------------
        recv = decl.get('receiver')
        if recv is not None and not isinstance(recv, dict):
            bad.append(f'{where}: `receiver` must be an object')
            recv = None
        rkind = (recv or {}).get('kind', 'entity')
        if rkind not in RECEIVER_KINDS:
            bad.append(f'{where}: unsupported receiver kind "{rkind}" — expected one of '
                       f'{", ".join(sorted(RECEIVER_KINDS))}')
        via = (recv or {}).get('via')
        if via is not None:
            if rkind == 'none':
                bad.append(f'{where}: receiver.kind "none" cannot carry a `via` sub-object hop')
            if not isinstance(via, dict) or not isinstance(via.get('class'), str) \
                    or not isinstance(via.get('field'), str) or not via['class'] or not via['field']:
                bad.append(f'{where}: receiver.via must be {{class, field}} with two non-empty '
                           f'strings — a partial `via` is silently ignored by core')

        # --- args / arity -----------------------------------------------------------------
        args = decl.get('args', [])
        if not isinstance(args, list):
            bad.append(f'{where}: `args` must be an array')
            args = []
        gp = fp = 0
        for i, a in enumerate(args):
            if not isinstance(a, str) or a not in ARG_KINDS:
                bad.append(f'{where}: args[{i}] = {a!r} is not in the arg vocabulary '
                           f'({", ".join(sorted(ARG_KINDS))})')
                continue
            if a == (FLOAT_KIND.group(1) if FLOAT_KIND else 'float'):
                fp += 1
            else:
                gp += 1
        if gp > MAX_GP_ARGS:
            bad.append(f'{where}: {gp} integer-class args exceeds the budget of {MAX_GP_ARGS}')
        if fp > MAX_FP_ARGS:
            bad.append(f'{where}: {fp} float args exceeds the budget of {MAX_FP_ARGS}')

        # `argNames` is documentary — the runtime never reads it — which is exactly why a wrong one
        # is never caught at runtime. A mismatched length makes every call site read as a lie.
        anames = decl.get('argNames')
        if anames is not None:
            if not isinstance(anames, list) or not all(isinstance(x, str) and x for x in anames):
                bad.append(f'{where}: `argNames` must be an array of non-empty strings')
            elif len(anames) != len(args):
                bad.append(f'{where}: argNames has {len(anames)} entries for {len(args)} args — '
                           f'every call site reads the wrong name')

        # --- returns ----------------------------------------------------------------------
        ret = decl.get('returns', 'void')
        if not isinstance(ret, str) or ret not in RET_KINDS:
            bad.append(f'{where}: unknown return kind {ret!r} — expected one of '
                       f'{", ".join(sorted(RET_KINDS))}')

        # --- target -----------------------------------------------------------------------
        target = decl.get('target')
        if not isinstance(target, dict):
            bad.append(f'{where}: descriptor has no `target` object')
            continue
        tkind = target.get('kind')
        if tkind not in TARGET_KINDS:
            bad.append(f'{where}: unknown target kind {tkind!r} — expected one of '
                       f'{", ".join(sorted(TARGET_KINDS))}')
            continue

        if tkind == 'signature':
            if isinstance(target.get('pattern'), str) and target['pattern']:
                check_validate(f'{where}.target', target.get('validate'), target)
                continue
            sname = target.get('name')
            if not isinstance(sname, str) or not sname:
                bad.append(f'{where}: signature target has no `name`')
                continue
            if sname not in sc['signatures']:
                bad.append(f'{where}: names signature "{sname}", which no file in {scope}/ '
                           f'declares — the descriptor would degrade at every boot')
                continue
            spath, plats = sc['signatures'][sname]
            if not isinstance(plats, dict) or PLATFORM not in plats:
                bad.append(f'{where}: signature "{sname}" ({spath}) has no "{PLATFORM}" entry')
                continue
            spec = plats[PLATFORM]
            if not isinstance(spec, dict) or not isinstance(spec.get('pattern'), str) \
                    or not spec['pattern'].strip():
                bad.append(f'{where}: signature "{sname}" ({spath}) has no byte pattern')
                continue
            if not isinstance(spec.get('module'), str) or not spec['module']:
                bad.append(f'{where}: signature "{sname}" ({spath}) names no module')
            # Only the INLINE override is checked here — the signature's own `validate` is covered
            # once, for every signature, by the sweep below (a validator is worth checking whether
            # or not a descriptor happens to target it yet). Checking it in both places would
            # report the same typo twice under two different labels.
            if 'validate' in target:
                check_validate(f'{where}.target', target['validate'], spec)

        elif tkind == 'vtable':
            plat = target.get(PLATFORM)
            if not isinstance(plat, dict):
                bad.append(f'{where}: vtable target has no "{PLATFORM}" object')
                continue
            idx = plat.get('index')
            if not isinstance(idx, int) or isinstance(idx, bool) or idx < 0:
                bad.append(f'{where}: vtable target needs a non-negative integer `index`')
            if not isinstance(target.get('class'), str) or not target['class']:
                bad.append(f'{where}: vtable target needs a `class` name to walk')
            v = plat.get('validate')
            # engine_calls.cpp REFUSES a vtable target with no prologue: a borrowed index can land
            # on a real, in-range thunk, so the .text guard alone passes it.
            if not isinstance(v, dict) or not v.get('prologue'):
                bad.append(f'{where}: a vtable target REQUIRES validate.prologue — a borrowed index '
                           f'that lands on an in-range thunk passes every other gate')
            check_validate(f'{where}.target', v)

# ---------------------------------------------------------------------------------------------
# 3b. `hooks` — the INBOUND sibling of `calls`. Reuses TARGET_KINDS/RECEIVER_KINDS/check_validate
# (the closed sets a hook's grammar shares with a call's) rather than a second copy; the one rule a
# hook has that a call does not — a validator is MANDATORY, never optional (spec §5) — is its own
# check below, mirroring the vtable-target `validate.prologue` requirement just above.
# ---------------------------------------------------------------------------------------------
for scope, sc in sorted(scopes.items()):
    call_names_in_scope = {name for (_p, name) in sc['calls']}
    for (path, name), decl in sorted(sc['hooks'].items(), key=lambda kv: (str(kv[0][0]), kv[0][1])):
        checked += 1
        where = f'{path}: hooks.{name}'
        if not isinstance(decl, dict):
            bad.append(f'{where}: descriptor must be an object')
            continue

        # --- expose.ctx (mandatory: nothing could subscribe to a hook with none) -----------
        expose = decl.get('expose')
        ctx_ns = (expose or {}).get('ctx') if isinstance(expose, dict) else None
        if not isinstance(ctx_ns, str) or not ctx_ns:
            bad.append(f'{where}: missing `expose.ctx` — nothing could subscribe to this hook')
        elif not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*', ctx_ns):
            bad.append(f'{where}: `expose.ctx` = {ctx_ns!r} is not a plain identifier')

        # --- shape: the closed thunk-ABI vocabulary the shim compiles for -------------------
        shape = decl.get('shape')
        if not isinstance(shape, str) or shape not in HOOK_SHAPES:
            bad.append(f'{where}: unknown hook shape {shape!r} — expected one of '
                       f'{", ".join(sorted(HOOK_SHAPES))}')

        # --- params / mutable: params are POSITIONAL identifiers; mutable must be a subset --
        params = decl.get('params', [])
        if not isinstance(params, list):
            bad.append(f'{where}: `params` must be an array')
            params = []
        seen_params = set()
        for i, p in enumerate(params):
            if not isinstance(p, str) or not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*', p):
                bad.append(f'{where}: params[{i}] = {p!r} is not a plain identifier')
            elif p in seen_params:
                bad.append(f'{where}: param name {p!r} is declared more than once')
            if isinstance(p, str):
                seen_params.add(p)

        # THE ONE PLACE THE "shape is compile-time, location is data" SPLIT LEAKS. `params` is data,
        # but it must agree with a COMPILE-TIME fact — how many arguments the thunk for this shape
        # actually receives. Nothing else connects them: core marshals by position and simply never
        # finds a param the shape does not have, `hookgen` emits one generated `.d.ts` field per
        # entry regardless, and check-hooks-generated.sh is satisfied by any freshly regenerated
        # file. So a fifth param on a 4-arg shape ships as a `readonly x: number` that is
        # `undefined` at every runtime read — a typed lie, behind no error anywhere.
        arity = SHAPE_ARITY.get(shape) if isinstance(shape, str) else None
        if arity is not None and len(params) > arity:
            bad.append(f'{where}: declares {len(params)} `params` but shape {shape!r} passes only '
                       f'{arity} — param(s) {", ".join(repr(p) for p in params[arity:])} name '
                       f'arguments the thunk never receives. Every read of them is `undefined` at '
                       f'runtime behind a generated `number` type, with no error anywhere')

        mutable = decl.get('mutable', [])
        if not isinstance(mutable, list):
            bad.append(f'{where}: `mutable` must be an array')
            mutable = []
        for mname in mutable:
            if not isinstance(mname, str) or mname not in params:
                bad.append(f"{where}: `mutable` names {mname!r}, which is not one of this hook's params")

        # --- receiver: {kind: none|entity}, "entity" needs a plain-identifier `as` ----------
        recv = decl.get('receiver')
        if recv is not None and not isinstance(recv, dict):
            bad.append(f'{where}: `receiver` must be an object')
            recv = None
        rkind = (recv or {}).get('kind', 'none')
        if rkind not in RECEIVER_KINDS:
            bad.append(f'{where}: unsupported receiver kind {rkind!r} — expected one of '
                       f'{", ".join(sorted(RECEIVER_KINDS))}')
        elif rkind == 'entity':
            as_name = (recv or {}).get('as')
            if not isinstance(as_name, str) or not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*', as_name):
                bad.append(f'{where}: receiver.kind "entity" needs a plain-identifier `as` name')
            elif as_name in params:
                bad.append(f'{where}: receiver `as` name {as_name!r} collides with a param name')

        # --- bypassWith: must name a `calls` descriptor in THIS SAME scope ------------------
        bypass = decl.get('bypassWith')
        if bypass is not None:
            if not isinstance(bypass, str) or not bypass:
                bad.append(f'{where}: `bypassWith` must be a non-empty call name')
            elif bypass not in call_names_in_scope:
                bad.append(f'{where}: `bypassWith` names {bypass!r}, which no `calls` descriptor in '
                           f'{scope}/ declares — the hook would degrade at every boot')

        # --- target + THE MANDATORY VALIDATOR (spec §5) -------------------------------------
        # Unlike a `calls` signature target, uniqueness alone is never enough for a hook: a wrong
        # CALL address misbehaves, a wrong DETOUR address overwrites the prologue of whatever is
        # actually there. flatten_decl (core/src/gamedata_calls.rs) LIFTS a named signature's own
        # co-located validator onto the hook's target when the hook declares no inline one of its
        # own, so the EFFECTIVE validator is the hook's inline one if present, else the signature's
        # — either being absent/empty must fail here exactly as prepare() fails it at load: named,
        # not silent.
        target = decl.get('target')
        if not isinstance(target, dict):
            bad.append(f'{where}: descriptor has no `target` object')
            continue
        tkind = target.get('kind')
        if tkind not in TARGET_KINDS:
            bad.append(f'{where}: unknown target kind {tkind!r} — expected one of '
                       f'{", ".join(sorted(TARGET_KINDS))}')
            continue

        if tkind == 'signature':
            if isinstance(target.get('pattern'), str) and target['pattern']:
                check_validate(f'{where}.target', target.get('validate'), target)
                validate_sources = [target.get('validate')]
            else:
                sname = target.get('name')
                if not isinstance(sname, str) or not sname:
                    bad.append(f'{where}: signature target has no `name`')
                    continue
                if sname not in sc['signatures']:
                    bad.append(f'{where}: names signature "{sname}", which no file in {scope}/ '
                               f'declares — the descriptor would degrade at every boot')
                    continue
                spath, plats = sc['signatures'][sname]
                if not isinstance(plats, dict) or PLATFORM not in plats:
                    bad.append(f'{where}: signature "{sname}" ({spath}) has no "{PLATFORM}" entry')
                    continue
                spec = plats[PLATFORM]
                if not isinstance(spec, dict) or not isinstance(spec.get('pattern'), str) \
                        or not spec['pattern'].strip():
                    bad.append(f'{where}: signature "{sname}" ({spath}) has no byte pattern')
                    continue
                if 'validate' in target:
                    check_validate(f'{where}.target', target['validate'], spec)
                validate_sources = [target.get('validate'), spec.get('validate')]

            if not any(isinstance(v, dict) and len(v) > 0 for v in validate_sources):
                bad.append(f'{where}: a hook target MUST carry a non-empty `validate` (inline, or '
                           f"inherited from its named signature) — a wrong DETOUR address overwrites "
                           f'the prologue of whatever is actually there')

        elif tkind == 'vtable':
            plat = target.get(PLATFORM)
            if not isinstance(plat, dict):
                bad.append(f'{where}: vtable target has no "{PLATFORM}" object')
                continue
            idx = plat.get('index')
            if not isinstance(idx, int) or isinstance(idx, bool) or idx < 0:
                bad.append(f'{where}: vtable target needs a non-negative integer `index`')
            if not isinstance(target.get('class'), str) or not target['class']:
                bad.append(f'{where}: vtable target needs a `class` name to walk')
            v = plat.get('validate')
            # Already mandatory for a CALL's vtable target (the block above) — a hook inherits the
            # same requirement, so no separate hook-only rule is needed here.
            if not isinstance(v, dict) or not v.get('prologue'):
                bad.append(f'{where}: a vtable target REQUIRES validate.prologue — a borrowed index '
                           f'that lands on an in-range thunk passes every other gate')
            check_validate(f'{where}.target', v)

# Every signature's validator is checked even when no descriptor targets it yet: an unshipped
# validator is still a live treadmill recipe, and a typo in one is invisible until the day it runs.
for scope, sc in sorted(scopes.items()):
    for sname, (spath, plats) in sorted(sc['signatures'].items()):
        if not isinstance(plats, dict):
            continue
        for plat, spec in plats.items():
            if isinstance(spec, dict) and 'validate' in spec:
                check_validate(f'{spath}: signatures.{sname}[{plat}]', spec['validate'], spec)

if bad:
    print('CALL DESCRIPTOR VIOLATIONS:', file=sys.stderr)
    for b in dict.fromkeys(bad):
        print(f'  {b}', file=sys.stderr)
    sys.exit(1)

print(f'check-call-descriptors: {checked} descriptor(s) well-formed '
      f'(args {"/".join(sorted(ARG_KINDS))}; returns {"/".join(sorted(RET_KINDS))}; '
      f'validators {"/".join(sorted(VALIDATORS))}; budget {MAX_GP_ARGS} GP + {MAX_FP_ARGS} FP; '
      f'hook shapes {"/".join(sorted(HOOK_SHAPES))})')
PY
