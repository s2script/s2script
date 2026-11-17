#!/usr/bin/env python3
"""Offline gamedata checker: replays the shim's resolver against CS2 binaries on disk.

    python3 -I tools/gamedata-offline/check.py --bin <dir with the .so files> [--old <dir>]

For every shipped signature it runs the same strategy the shim runs at boot (direct /
lea-disp / ctor-body-xref / validated-call, then the prologue / string-xref / vtable-member
validators). For every `<Iface>_vtableSize` pin it counts the implementing class's vtable
slots. With --old it also aligns each tracked class's vtable old-vs-new by a normalised
function fingerprint, so a slot that moved (an inserted virtual) shows up as old->new index.

Needs pyelftools and capstone. Reads binaries only; never executes them.
"""
import argparse, difflib, hashlib, json, pathlib, struct, sys

from elftools.elf.elffile import ELFFile
from elftools.elf.relocation import RelocationSection
import capstone

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from jsonc import strip_jsonc_comments  # noqa: E402

PLAT = 'linuxsteamrt64'
R_RELATIVE, R_64, R_GLOB_DAT = 8, 1, 6
CLASS_TI_VTABLES = {
    '_ZTVN10__cxxabiv117__class_type_infoE',
    '_ZTVN10__cxxabiv120__si_class_type_infoE',
    '_ZTVN10__cxxabiv121__vmi_class_type_infoE',
}


class Image:
    def __init__(self, path):
        self.path = path
        data = pathlib.Path(path).read_bytes()
        elf = ELFFile(open(path, 'rb'))
        self.segs = []  # (vaddr, bytearray, exec)
        for s in elf.iter_segments():
            if s['p_type'] != 'PT_LOAD':
                continue
            buf = bytearray(s['p_memsz'])
            buf[:s['p_filesz']] = data[s['p_offset']:s['p_offset'] + s['p_filesz']]
            self.segs.append((s['p_vaddr'], buf, bool(s['p_flags'] & 1)))
        self.sections = {sec.name: (sec['sh_addr'], sec['sh_size']) for sec in elf.iter_sections() if sec['sh_addr']}
        self.extfuncs = set()  # addresses of slots holding a symbol reloc to a FUNC (e.g. __cxa_pure_virtual)
        self.typeinfos = set()  # addresses of class typeinfo objects (their vptr is a reloc to a __cxxabiv1 vtable)
        for sec in elf.iter_sections():
            if not isinstance(sec, RelocationSection):
                continue
            symtab = elf.get_section(sec['sh_link'])
            for r in sec.iter_relocations():
                t = r['r_info_type']
                if t == R_RELATIVE:
                    self.w64(r['r_offset'], r['r_addend'])
                elif t in (R_64, R_GLOB_DAT):
                    sym = symtab.get_symbol(r['r_info_sym'])
                    if t == R_64 and sym.name in CLASS_TI_VTABLES:
                        self.typeinfos.add(r['r_offset'])
                    if sym['st_shndx'] != 'SHN_UNDEF':
                        self.w64(r['r_offset'], sym['st_value'] + r['r_addend'])
                    else:
                        if sym['st_info']['type'] == 'STT_FUNC' or sym.name.startswith('__cxa_'):
                            self.extfuncs.add(r['r_offset'])
                        self.w64(r['r_offset'], 0)
        self.exec_segs = [(v, b) for v, b, x in self.segs if x]

    def seg(self, at, n=1):
        for v, b, x in self.segs:
            if v <= at and at + n <= v + len(b):
                return v, b, x
        return None

    def read(self, at, n):
        s = self.seg(at, n)
        if not s:
            return None
        v, b, _ = s
        return bytes(b[at - v:at - v + n])

    def r64(self, at):
        b = self.read(at, 8)
        return None if b is None else struct.unpack('<q', b)[0]

    def w64(self, at, val):
        s = self.seg(at, 8)
        if s:
            v, b, _ = s
            b[at - v:at - v + 8] = struct.pack('<Q', val & 0xFFFFFFFFFFFFFFFF)

    def executable(self, at, n=1):
        s = self.seg(at, n)
        return bool(s and s[2])

    def sec_bytes(self, name):
        if name not in self.sections:
            return None, None
        a, n = self.sections[name]
        return a, self.read(a, n)

    # ---- pattern scan (exec segments only, like s2sig::FindPatterns over executable_segments)
    def find(self, pattern):
        first = next((i for i, p in enumerate(pattern) if p >= 0), None)
        out = []
        for v, b in self.exec_segs:
            mb = bytes(b)
            if first is None:
                continue
            # anchor on the longest fixed run
            best = (first, 1)
            i = 0
            while i < len(pattern):
                if pattern[i] < 0:
                    i += 1
                    continue
                j = i
                while j < len(pattern) and pattern[j] >= 0:
                    j += 1
                if j - i > best[1]:
                    best = (i, j - i)
                i = j
            off, ln = best
            anchor = bytes(pattern[off:off + ln])
            pos = mb.find(anchor)
            while pos != -1:
                st = pos - off
                if st >= 0 and st + len(pattern) <= len(mb):
                    if all(p < 0 or mb[st + k] == p for k, p in enumerate(pattern)):
                        out.append(v + st)
                pos = mb.find(anchor, pos + 1)
        return sorted(set(out))

    # ---- RTTI vtable lookup (mirrors s2vtable::GetVTableByName; offset_to_top selectable)
    def vtable(self, cls, offset_to_top=0):
        key = (cls, offset_to_top)
        cache = self.__dict__.setdefault('_vtc', {})
        if key in cache:
            return cache[key]
        cache[key] = None
        ra, rb = self.sec_bytes('.rodata')
        if rb is None:
            return None
        name = (str(len(cls)) + cls).encode() + b'\0'
        i = rb.find(name)
        if i == -1:
            return None
        name_addr = ra + i
        ti = None
        for sname in ('.data.rel.ro', '.data.rel.ro.local'):
            sa, sb = self.sec_bytes(sname)
            if sb is None:
                continue
            p = sb.find(struct.pack('<Q', name_addr))
            while p != -1:
                if p % 8 == 0:
                    ti = sa + p - 8
                    break
                p = sb.find(struct.pack('<Q', name_addr), p + 1)
            if ti:
                break
        if not ti:
            return None
        for sname in ('.data.rel.ro', '.data.rel.ro.local'):
            sa, sb = self.sec_bytes(sname)
            if sb is None:
                continue
            pat = struct.pack('<Q', ti)
            p = sb.find(pat)
            while p != -1:
                if p % 8 == 0 and p >= 8 and struct.unpack('<q', sb[p - 8:p])[0] == offset_to_top:
                    cache[key] = sa + p + 8
                    return cache[key]
                p = sb.find(pat, p + 1)
        return None

    def all_vtables(self):
        """Every RTTI vtable in the module: {(class name, offset_to_top): slot count}.

        A vtable is an 8-aligned qword in .data.rel.ro that points at a class typeinfo, preceded by
        a non-positive offset_to_top and followed by at least one function slot. Same-named
        template instantiations from different modules never meet, since this is per module.
        """
        names = {}
        for ti in self.typeinfos:
            np = self.r64(ti + 8)
            raw = self.read(np, 256) if np else None
            if raw:
                names[ti] = raw.split(b'\0')[0].decode('latin-1')
        out = {}
        for sname in ('.data.rel.ro', '.data.rel.ro.local'):
            sa, sb = self.sec_bytes(sname)
            if sb is None:
                continue
            for p in range(8, len(sb) - 8, 8):
                ti = struct.unpack_from('<q', sb, p)[0]
                if ti not in names:
                    continue
                ott = struct.unpack_from('<q', sb, p - 8)[0]
                if not -0x1000000 <= ott <= 0:
                    continue
                n = len(self.slots(sa + p + 8))
                if n:
                    key = (names[ti], ott)
                    out[key] = max(n, out.get(key, 0))
        return out

    def slots(self, vt):
        out = []
        for i in range(512):
            at = vt + 8 * i
            if at in self.extfuncs:
                out.append(('ext', at))
                continue
            p = self.r64(at)
            if p is None or not self.executable(p):
                break
            out.append(p)
        return out


def parse_pattern(s):
    return [-1 if t in ('?', '??') else int(t, 16) for t in s.split()]


def rel(img, pc, off, ln):
    b = img.read(pc + off, 4)
    if b is None:
        return None
    return pc + ln + struct.unpack('<i', b)[0]


def validate(img, v, fn):
    if not v:
        return None
    if not img.executable(fn):
        return 'validator input outside .text'
    if 'prologue' in v:
        pat = parse_pattern(v['prologue'])
        b = img.read(fn, len(pat))
        if b is None or any(p >= 0 and b[i] != p for i, p in enumerate(pat)):
            return 'prologue mismatch'
    if 'string-xref' in v:
        x = v['string-xref']
        tgt = rel(img, fn + x['at'], x['dispOff'], x['instrLen'])
        exp = x['expect'].encode() + b'\0'
        got = img.read(tgt, len(exp)) if tgt else None
        if got != exp:
            shown = img.read(tgt, 40) if tgt else None
            return f"string-xref expected {x['expect']!r}, got {shown.split(b'\0')[0] if shown else None!r}"
    if 'vtable-member' in v:
        vt = img.vtable(v['vtable-member'])
        if not vt:
            return f"vtable {v['vtable-member']} not found"
        if fn not in img.slots(vt):
            return f"not a member of {v['vtable-member']} primary vtable"
    return None


def resolve(img, e):
    strat = e.get('resolve', 'direct')
    v = e.get('validate') or {}
    cands = img.find(parse_pattern(e['pattern']))
    if strat == 'validated-call':
        ok = [c for c in cands if img.read(c, 1) == b'\xe8' and validate(img, v, c) is None]
        if len(ok) != 1:
            return None, f'validated-call: {len(cands)} matches, {len(ok)} validated'
        return rel(img, ok[0], 1, 5), None
    if len(cands) > 1 and strat == 'direct' and v:
        passed = [c for c in cands if validate(img, v, c) is None]
        if len(passed) != 1:
            return None, f'ambiguous ({len(cands)} matches, {len(passed)} passed validators)'
        cands = passed
    if len(cands) != 1:
        return None, 'NOT FOUND' if not cands else f'ambiguous ({len(cands)} matches)'
    t = cands[0]
    if strat == 'lea-disp':
        return rel(img, t, 3, 7), None
    if strat == 'ctor-body-xref':
        callers = [c for c in img.find([0xe8, -1, -1, -1, -1]) if rel(img, c, 1, 5) == t]
        if len(callers) != 1:
            return None, f'ctor-body-xref: {len(callers)} ctor callers'
        c = callers[0]
        for back in range(7, 33):
            b = img.read(c - back, 3)
            if b and b[0] == 0x4c and b[1] == 0x8d and b[2] in (0x35, 0x2d):
                return rel(img, c - back, 3, 7), None
        return None, 'ctor-body-xref: preceding LEA not found'
    err = validate(img, v, t)
    return (None, err) if err else (t, None)


# ---- vtable fingerprinting for old/new alignment
_md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
_md.detail = False


def fingerprint(img, fn):
    if isinstance(fn, tuple):
        return 'ext'
    code = img.read(fn, 160) or b''
    toks = []
    for ins in _md.disasm(code, fn):
        op = ins.op_str
        if 'rip' in op:
            op = 'RIP'
        if ins.mnemonic in ('call', 'jmp') or ins.mnemonic.startswith('j'):
            op = 'REL' if op.startswith('0x') else op
        toks.append(ins.mnemonic + ' ' + op)
        if ins.mnemonic in ('ret', 'jmp', 'ud2') or len(toks) >= 24:
            break
    return hashlib.sha1('\n'.join(toks).encode()).hexdigest()[:12]


def load_targets():
    """tools/gamedata-offline/targets.jsonc: the gamedata files, pins and bare indices (shared
    with gdgen, github.com/s2script/s2-gamedatagen)."""
    return json.loads(strip_jsonc_comments((pathlib.Path(__file__).parent / 'targets.jsonc').read_text()))


TARGETS = load_targets()


def load_gamedata():
    return {k: json.loads(strip_jsonc_comments((ROOT / p).read_text())) for k, p in TARGETS['gamedata'].items()}


# Interface pins: (pin key, module, class, offset_to_top). See the comment block above the pins in
# gamedata/core/game.cs2.jsonc.
PINS = [(p['key'], p['module'], p['class'], p.get('offsetToTop', 0)) for p in TARGETS['pins']]
PIN_CLASS = {k.removesuffix('_vtableSize'): (m, c, o) for k, m, c, o in PINS}
# Bare vtable indices that are not behind an interface pin.
EXTRA_INDEX = {k: (v['module'], v['class'], v.get('offsetToTop', 0)) for k, v in TARGETS['indices'].items()}


class Missing:
    """Stand-in for a module that is not in the directory: nothing resolves in it."""

    def find(self, pattern):
        return []

    def vtable(self, cls, offset_to_top=0):
        return None

    def slots(self, vt):
        return []

    def all_vtables(self):
        return {}


def tracked_classes(gd):
    """Class names s2script names somewhere (pins, bare indices, vtable calls, validators)."""
    out = {c for _, _, c, _ in PINS} | {c for _, c, _ in EXTRA_INDEX.values()}
    for d in gd.values():
        for c in d.get('calls', {}).values():
            t = c.get('target', {})
            if t.get('kind') == 'vtable':
                out.add(t['class'])
        for s in d.get('signatures', {}).values():
            vm = ((s.get(PLAT) or {}).get('validate') or {}).get('vtable-member')
            if vm:
                out.add(vm)
    return out


def demangle_name(n):
    """'16CNetworkMessages' -> 'CNetworkMessages'; nested/template names are left mangled."""
    i = 0
    while i < len(n) and n[i].isdigit():
        i += 1
    if i and int(n[:i]) == len(n) - i:
        return n[i:]
    return n


VTABLE_MODULES = ['libserver.so', 'libengine2.so', 'libtier0.so', 'libnetworksystem.so',
                  'libmatchmaking.so', 'libsoundsystem.so', 'libfilesystem_stdio.so']


def vtable_diff(old_dir, new_dir, tracked):
    """Per module: every vtable whose slot count changed, plus vtables that appeared or vanished."""
    out = []
    for mod in VTABLE_MODULES:
        op, np_ = pathlib.Path(old_dir) / mod, pathlib.Path(new_dir) / mod
        if not (op.exists() and np_.exists()):
            continue
        o, n = Image(str(op)).all_vtables(), Image(str(np_)).all_vtables()
        for key in sorted(set(o) | set(n)):
            if o.get(key) == n.get(key):
                continue
            name = demangle_name(key[0])
            out.append({'module': mod, 'class': name, 'offset_to_top': key[1], 'old': o.get(key),
                        'new': n.get(key), 'tracked': name in tracked})
    return out


def short(name, n=70):
    return name if len(name) <= n else name[:n - 1] + '…'


def markdown(report, a):
    L = []
    sigs = report['signatures']
    broke = [x for x in sigs if not x['ok'] and x.get('old_ok', True)]
    still = [x for x in sigs if not x['ok'] and not x.get('old_ok', True)]
    pins = [x for x in report['pins'] if not x['ok']]
    moved = [x for x in report['indices'] if index_moved(x)]
    rebodied = [x for x in report['indices'] if x['new'] is None and not index_moved(x)]
    vt = report.get('vtables', [])
    vt_tracked = [x for x in vt if x['tracked']]
    L.append('### Summary\n')
    L.append(f"- Signatures: {len(sigs) - len(broke) - len(still)} ok, **{len(broke)} newly failing**, {len(still)} already failing")
    L.append(f"- Vtable pins: {len(report['pins']) - len(pins)} ok, **{len(pins)} mismatched**")
    if a.old:
        L.append(f"- Tracked vtable indices: {len(report['indices']) - len(moved) - len(rebodied)} unchanged, "
                 f"**{len(moved)} moved**, {len(rebodied)} recompiled in place (same table size, new body)")
    if a.all_vtables:
        L.append(f"- All-vtable diff: {len(vt)} vtables changed, **{len(vt_tracked)} of them in classes s2script names**")
    L.append('')
    if broke or still:
        L.append('### Failing signatures\n')
        L.append('| | Owner | Name | Reason |\n|---|---|---|---|')
        for x in broke + still:
            L.append(f"| {'NEW' if x in broke else 'old'} | {x['owner']} | `{x['name']}` | {x['reason']} |")
        L.append('')
    if pins:
        L.append('### Vtable pin mismatches\n')
        L.append('| Pin | Class | Pin value | New build | Old build |\n|---|---|---|---|---|')
        for x in pins:
            L.append(f"| `{x['key']}` | {x['class']} | {x['pin']} | {x['live']} | {x['old'] if x['old'] is not None else '?'} |")
        L.append('')
    if moved or rebodied:
        L.append('### Tracked vtable indices that changed\n')
        L.append('| | Key | Class | Old index | New index | Slots |\n|---|---|---|---|---|---|')
        for x in moved + rebodied:
            new = x['new'] if x['new'] is not None else 'not found'
            L.append(f"| {'MOVED' if x in moved else 'new body?'} | `{x['key']}` | {x['class']} | {x['old']} | {new} | {x['old_slots']} -> {x['new_slots']} |")
        L.append('')
    if vt:
        L.append('### Vtable slot counts that changed\n')
        L.append('Classes s2script names come first. Message classes that all changed the same way are grouped.\n')
        L.append('| Module | Class | Old | New |\n|---|---|---|---|')
        rest = []
        for x in vt:
            (L if x['tracked'] else rest).append(
                f"| {x['module']} | **{x['class']}**{'' if not x['offset_to_top'] else ' (secondary ' + str(x['offset_to_top']) + ')'} | {x['old'] or '—'} | {x['new'] or '—'} |"
                if x['tracked'] else x)
        groups = {}
        for x in rest:
            groups.setdefault((x['module'], x['old'], x['new']), []).append(x)
        for (mod, o, n), xs in sorted(groups.items(), key=lambda kv: (kv[0][0], -len(kv[1]))):
            names = [short(x['class']) + ('' if not x['offset_to_top'] else f" ({x['offset_to_top']})") for x in xs]
            shown = ', '.join(f'`{c}`' for c in names[:3]) + (f' and {len(names) - 3} more' if len(names) > 3 else '')
            L.append(f"| {mod} | {shown} | {o or '—'} | {n or '—'} |")
        L.append('')
    return '\n'.join(L)


def index_moved(x):
    """A tracked index needs a look when it maps to another slot, or vanished from a table whose
    size changed. A vanished fingerprint in a same-size table is a recompiled body at the same
    index (CheckTransmit, PostEventAbstract on Nov 11), shown but not flagged."""
    if x['new'] is None:
        return x['old_slots'] != x['new_slots']
    return x['new'] != x['old']


def needs_attention(report):
    return (any(not x['ok'] and x.get('old_ok', True) for x in report['signatures'])
            or any(not x['ok'] for x in report['pins'])
            or any(index_moved(x) for x in report['indices'])
            or any(x['tracked'] for x in report.get('vtables', [])))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--bin', required=True)
    ap.add_argument('--old')
    ap.add_argument('--json', help='write the full report as JSON')
    ap.add_argument('--markdown', help='write a Markdown summary (for an issue or PR body)')
    ap.add_argument('--all-vtables', action='store_true',
                    help='with --old: diff the slot count of every RTTI vtable in every module')
    ap.add_argument('--strict', action='store_true',
                    help='exit 1 when anything needs attention (see the summary)')
    a = ap.parse_args()
    gd = load_gamedata()
    imgs, old = {}, {}

    def img(d, cache, mod):
        if mod not in cache:
            p = pathlib.Path(d) / mod
            if p.exists():
                cache[mod] = Image(str(p))
            else:
                print(f'  (missing module {p})', file=sys.stderr)
                cache[mod] = Missing()
        return cache[mod]

    report = {'signatures': [], 'pins': [], 'indices': []}
    print('== signatures')
    for owner, d in gd.items():
        for name, s in d.get('signatures', {}).items():
            e = s.get(PLAT)
            if not e:
                continue
            addr, err = resolve(img(a.bin, imgs, e['module']), e)
            line = {'owner': owner, 'name': name, 'ok': err is None, 'addr': addr, 'reason': err}
            if a.old:
                oa, oe = resolve(img(a.old, old, e['module']), e)
                line['old_ok'] = oe is None
            report['signatures'].append(line)
            mark = 'ok  ' if err is None else 'FAIL'
            was = '' if not a.old else ('  (was ok)' if line['old_ok'] else '  (was already failing)')
            print(f"  {mark} {owner}/{name}" + (f" @0x{addr:x}" if addr else f" — {err}") + (was if err else ''))

    def table(d, cache, mod, cls, ott):
        im = img(d, cache, mod)
        vt = im.vtable(cls, ott)
        return (im, vt, im.slots(vt)) if vt else (im, None, [])

    print('== vtable pins')
    offs = gd['core']['offsets']
    for key, mod, cls, ott in PINS:
        pin = offs[key][PLAT]
        _, vt, sl = table(a.bin, imgs, mod, cls, ott)
        o = len(table(a.old, old, mod, cls, ott)[2]) if a.old else None
        ok = len(sl) == pin
        report['pins'].append({'key': key, 'class': cls, 'pin': pin, 'live': len(sl), 'old': o, 'ok': ok})
        print(f"  {'ok  ' if ok else 'FAIL'} {key}: {cls} has {len(sl)} slots, pin {pin}" + (f" (old build {o})" if o is not None else ''))

    if a.old:
        print('== index alignment (old -> new by function fingerprint)')
        idx = {}
        for k, v in offs.items():
            iface = k.split('_')[0]
            if iface in PIN_CLASS and not k.endswith('_vtableSize'):
                idx[k] = (PIN_CLASS[iface], v[PLAT])
        for k, cls in EXTRA_INDEX.items():
            src = offs if k in offs else gd['cs2']['offsets']
            idx[k] = (cls, src[k][PLAT])
        for c in gd['cs2'].get('calls', {}).values():
            t = c.get('target', {})
            if t.get('kind') == 'vtable':
                idx[f"call:{t['class']}"] = ((t.get('module', 'libserver.so'), t['class'], 0), t[PLAT]['index'])
        for k, ((mod, cls, ott), i) in idx.items():
            oi, _, osl = table(a.old, old, mod, cls, ott)
            ni, _, nsl = table(a.bin, imgs, mod, cls, ott)
            if not osl or not nsl:
                print(f"  ?    {k}: {cls} vtable not found")
                continue
            of = [fingerprint(oi, x) for x in osl]
            nf = [fingerprint(ni, x) for x in nsl]
            sm = difflib.SequenceMatcher(None, of, nf, autojunk=False)
            mapping = {}
            for blk in sm.get_matching_blocks():
                for j in range(blk.size):
                    mapping[blk.a + j] = blk.b + j
            new = mapping.get(i)
            uniq = of.count(of[i]) == 1 if i < len(of) else False
            status = 'ok  ' if new == i else ('MOVE' if new is not None else 'GONE?')
            note = '' if uniq else ' (fingerprint not unique in table)'
            report['indices'].append({'key': k, 'class': cls, 'old': i, 'new': new, 'old_slots': len(osl), 'new_slots': len(nsl)})
            print(f"  {status} {k}: {cls}[{i}] -> {new}  ({len(osl)} -> {len(nsl)} slots){note}")
    if a.old and a.all_vtables:
        print('== all-vtable slot counts (old -> new)')
        report['vtables'] = vtable_diff(a.old, a.bin, tracked_classes(gd))
        for x in report['vtables']:
            if x['tracked']:
                print(f"  {'TRACKED' if x['tracked'] else '       '} {x['module']} {x['class']} ({x['offset_to_top']}): {x['old']} -> {x['new']}")
        print(f"  {len(report['vtables'])} vtables changed, {sum(x['tracked'] for x in report['vtables'])} tracked")
    report['needs_attention'] = needs_attention(report)
    if a.json:
        pathlib.Path(a.json).write_text(json.dumps(report, indent=1))
    if a.markdown:
        pathlib.Path(a.markdown).write_text(markdown(report, a))
    if a.strict and report['needs_attention']:
        sys.exit(1)


if __name__ == '__main__':
    main()
