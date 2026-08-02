#!/usr/bin/env bash
# Fails if a shipped `calls` descriptor is not WELL-FORMED against the declared-engine-call grammar.
#
# WHY THIS GATE EXISTS. A descriptor is data, and the runtime's answer to bad data is — correctly —
# to degrade THAT descriptor with a named reason and keep the server up. That is the right runtime
# behaviour and the wrong authoring feedback: a typo'd arg kind (`"bool "`), a signature name that
# does not exist (`CCSPlayerController_Repawn`), a validator key spelled `vtable_member`, or a
# 5-arg descriptor for a 4-arg function all produce a server that boots green and a feature that is
# quietly gone. Nobody reads the boot log for a WARN they are not expecting.
#
# Everything here is decidable WITHOUT the game binary, so it is decidable in CI. What needs the
# binary (does the pattern match? is it unique? does the validator pass?) stays at load, where it
# belongs — this gate exists so that by the time the loader runs, the only remaining question is
# about the BINARY, never about the JSON.
#
# THE VOCABULARY IS NOT HARDCODED HERE. Every closed set below is read out of the source that
# actually enforces it — the arg/return/receiver/target vocabularies and the SysV arity budget from
# core/src/gamedata_calls.rs, the validator vocabulary from shim/src/call_validate.cpp. A gate with
# its own copy of a closed set is a third place for the set to drift; this one fails loudly if it
# cannot find the real one.
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

FLATTEN = fn_body(CORE_SRC, 'fn flatten_decl(', 'target')
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

scopes = {}   # dir -> {"calls": {(file, name): decl}, "signatures": {name: (file, spec)}}
for root in (pathlib.Path('gamedata'), pathlib.Path('examples'), pathlib.Path('plugins')):
    if not root.is_dir():
        continue
    for p in sorted(root.rglob('*.json*')):
        if p.suffix not in ('.json', '.jsonc') or is_custom(p):
            continue
        try:
            data = json.loads(strip_jsonc_comments(p.read_text()))
        except Exception as e:
            # A file this gate cannot read is only its business if it declares calls — and it
            # cannot know that without reading it. Report, don't guess.
            if '"calls"' in p.read_text():
                bad.append(f'{p}: declares `calls` but is not valid JSON — {e}')
            continue
        if not isinstance(data, dict) or not ({'calls', 'signatures'} & set(data)):
            continue
        sc = scopes.setdefault(p.parent, {'calls': {}, 'signatures': {}})
        for name, spec in (data.get('signatures') or {}).items():
            sc['signatures'][name] = (p, spec)
        for name, decl in (data.get('calls') or {}).items():
            sc['calls'][(p, name)] = decl

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
      f'validators {"/".join(sorted(VALIDATORS))}; budget {MAX_GP_ARGS} GP + {MAX_FP_ARGS} FP)')
PY
