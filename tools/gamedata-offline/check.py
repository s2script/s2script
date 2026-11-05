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


def load_gamedata():
    files = {
        'core': ROOT / 'gamedata/core/game.cs2.jsonc',
        'sdkhooks': ROOT / 'gamedata/sdkhooks/game.cs2.jsonc',
        'cs2': ROOT / 'games/cs2/gamedata/game.cs2.jsonc',
    }
    return {k: json.loads(strip_jsonc_comments(p.read_text())) for k, p in files.items()}


# Interface pins: (pin key, module, class, offset_to_top). See the comment block above the pins in
# gamedata/core/game.cs2.jsonc.
PINS = [
    ('ISource2Server_vtableSize', 'libserver.so', 'CSource2Server', 0),
    ('ISource2ServerConfig_vtableSize', 'libserver.so', 'CCSGameConfiguration', -8),
    ('ISource2GameClients_vtableSize', 'libserver.so', 'CSource2GameClients', 0),
    ('ISource2GameEntities_vtableSize', 'libserver.so', 'CSource2GameEntities', 0),
    ('IGameEventManager2_vtableSize', 'libserver.so', 'CGameEventManager', 0),
    ('IGameEventSystem_vtableSize', 'libengine2.so', 'CGameEventSystem', 0),
    ('INetworkServerService_vtableSize', 'libengine2.so', 'CNetworkServerService', 0),
    ('ICvar_vtableSize', 'libtier0.so', 'CCvar', 0),
    ('IVEngineServer2_vtableSize', 'libengine2.so', 'CEngineServer', 0),
    ('INetChannelInfo_vtableSize', 'libnetworksystem.so', 'CNetChan', 0),
    ('ISoundEventManager_vtableSize', 'libsoundsystem.so', 'CSoundOpSystem', -8),
    ('INetworkStringTableContainer_vtableSize', 'libengine2.so', 'CNetworkStringTableContainer', 0),
    ('INetworkStringTable_vtableSize', 'libengine2.so', 'CNetworkStringTable', 0),
]
PIN_CLASS = {k.removesuffix('_vtableSize'): (m, c, o) for k, m, c, o in PINS}
# Bare vtable indices that are not behind an interface pin.
EXTRA_INDEX = {
    'CNavPhysicsInterface_TraceShape': ('libserver.so', 'CNavPhysicsInterface', 0),
    'CBaseEntity_Teleport': ('libserver.so', 'CBaseEntity', 0),
    'CGameRulesGameSystem_OnPrecacheResource': ('libserver.so', 'CGameRulesGameSystem', 0),
    'CServerSideClient_ProcessRespondCvarValue': ('libengine2.so', 'CServerSideClient', 0),
    'CCSPlayer_ItemServices_RemoveWeapons': ('libserver.so', 'CCSPlayer_ItemServices', 0),
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--bin', required=True)
    ap.add_argument('--old')
    ap.add_argument('--json')
    a = ap.parse_args()
    gd = load_gamedata()
    imgs, old = {}, {}

    def img(d, cache, mod):
        if mod not in cache:
            cache[mod] = Image(str(pathlib.Path(d) / mod))
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
    if a.json:
        pathlib.Path(a.json).write_text(json.dumps(report, indent=1))


if __name__ == '__main__':
    main()
