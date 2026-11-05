#!/usr/bin/env python3
"""Offline schema dump: rebuild games/cs2/gamedata/schema-catalog.json from depot binaries.

    python3 -I tools/gamedata-offline/schema.py --bin <dir> [--out <catalog.json>] [--enums <enums.json>]
                                                [--prev <previous catalog.json>]

Every module that registers schema classes carries them as static SchemaClassInfoData_t
records: name, size, base classes and a field array of {name, type, offset, metadata}. Builtin
field types are tagged constants in the file. Every other type is filled in at install time by a
per-class callback (the record's install function), which asks the type scope for each type by
name ("FindDeclaredClass(\"GameTime_t\")", "Pointer(...)", "AtomicT(\"CHandle\", ...)") and
stores the result into the field array. This script decodes the static records and replays
those callbacks symbolically to recover each field's type, then writes the catalog in the same
shape the live `__s2_schema_dump` produces.

Reads the files and never runs them. Needs pyelftools and capstone.
"""
import argparse, json, pathlib, re, struct, sys

import capstone
from capstone import x86

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from check import Image  # noqa: E402

IDENT = re.compile(rb'^[A-Za-z_][A-Za-z0-9_:<>, *\[\]&]*$')

# Builtin type tags (high nibble 0xf) -> catalog type, as libserver.so numbers them. Learned from
# the live dump. The engine modules number some tags differently (MODULE_BUILTIN below).
BUILTIN = {
    0x01: {"kind": "unknown", "name": "bitfield:1"},
    0x02: {"kind": "ptr", "inner": "char"},
    0x04: {"kind": "atomic", "name": "CUtlVector< CHandle< CBaseEntity > >"},
    0x05: {"kind": "atomic", "name": "char"},
    0x06: {"kind": "atomic", "name": "int8"}, 0x07: {"kind": "atomic", "name": "uint8"},
    0x08: {"kind": "atomic", "name": "int16"}, 0x09: {"kind": "atomic", "name": "uint16"},
    0x0a: {"kind": "atomic", "name": "int32"}, 0x0b: {"kind": "atomic", "name": "uint32"},
    0x0c: {"kind": "atomic", "name": "int64"}, 0x0d: {"kind": "atomic", "name": "uint64"},
    0x0e: {"kind": "atomic", "name": "float32"}, 0x0f: {"kind": "atomic", "name": "float64"},
    0x10: {"kind": "atomic", "name": "bool"},
    0x12: {"kind": "atomic", "name": "Vector"}, 0x13: {"kind": "atomic", "name": "VectorWS"},
    0x14: {"kind": "atomic", "name": "Vector2D"}, 0x15: {"kind": "atomic", "name": "Vector4D"},
    0x16: {"kind": "atomic", "name": "QAngle"}, 0x17: {"kind": "atomic", "name": "Quaternion"},
    0x1c: {"kind": "atomic", "name": "matrix3x4_t"}, 0x1d: {"kind": "atomic", "name": "matrix3x4a_t"},
    0x1e: {"kind": "atomic", "name": "CTransform"}, 0x20: {"kind": "atomic", "name": "Color"},
    0x21: {"kind": "atomic", "name": "CUtlSymbolLarge"}, 0x22: {"kind": "atomic", "name": "CUtlBinaryBlock"},
    0x23: {"kind": "atomic", "name": "CUtlString"}, 0x24: {"kind": "atomic", "name": "CUtlSymbol"},
    0x26: {"kind": "atomic", "name": "CUtlStringToken"}, 0x27: {"kind": "atomic", "name": "fltx4"},
    0x28: {"kind": "atomic", "name": "CEntityIndex"}, 0x2a: {"kind": "atomic", "name": "CSplitScreenSlot"},
    0x2b: {"kind": "atomic", "name": "CPlayerSlot"}, 0x2c: {"kind": "atomic", "name": "HSCRIPT"},
    0x2d: {"kind": "atomic", "name": "CBufferString"},
    0x30: {"kind": "class", "name": "AttachmentHandle_t"}, 0x31: {"kind": "class", "name": "ParticleIndex_t"},
    0x32: {"kind": "class", "name": "CountdownTimer"}, 0x34: {"kind": "class", "name": "EngineLoopState_t"},
    0x35: {"kind": "handle", "inner": "CBaseEntity"}, 0x36: {"kind": "atomic", "name": "CEntityHandle"},
    0x37: {"kind": "class", "name": "CEntityIOOutput"},
    0x39: {"kind": "atomic", "name": "CEntityOutputTemplate< int32 >"},
    0x3b: {"kind": "atomic", "name": "CEntityOutputTemplate< float32 >"},
    0x3c: {"kind": "atomic", "name": "CEntityOutputTemplate< CUtlSymbolLarge >"},
    0x3d: {"kind": "atomic", "name": "CEntityOutputTemplate< Vector >"},
    0x3e: {"kind": "atomic", "name": "CEntityOutputTemplate< Color >"},
    0x43: {"kind": "enum", "name": "JointMotion_t", "size": 4},
    0x45: {"kind": "atomic", "name": "KeyValues3"},
    0x47: {"kind": "atomic", "name": "CVariantBase< CVariantDefaultAllocator >"},
    0x48: {"kind": "ptr", "inner": "CScriptComponent"},
    0x4a: {"kind": "class", "name": "ChangeAccessorFieldPathIndex_t"},
    0x4b: {"kind": "class", "name": "CNetworkVarChainer"},
    0x4c: {"kind": "atomic", "name": "CNetworkedQuantizedFloat"}, 0x4d: {"kind": "class", "name": "HSequence"},
    0x52: {"kind": "atomic", "name": "CWeakHandle< InfoForResourceTypeIParticleSystemDefinition >"},
    0x55: {"kind": "atomic", "name": "CStrongHandle< InfoForResourceTypeIMaterial2 >"},
    0x56: {"kind": "atomic", "name": "CStrongHandle< InfoForResourceTypeCModel >"},
    0x58: {"kind": "atomic", "name": "CStrongHandle< InfoForResourceTypeIParticleSystemDefinition >"},
    0x59: {"kind": "atomic", "name": "CStrongHandle< InfoForResourceTypeCTextureBase >"},
    0x5b: {"kind": "atomic", "name": "CStrongHandleCopyable< InfoForResourceTypeIMaterial2 >"},
    0x6e: {"kind": "class", "name": "CSkillFloat"}, 0x6f: {"kind": "class", "name": "CSkillInt"},
    0x71: {"kind": "atomic", "name": "RotationVector"}, 0x72: {"kind": "atomic", "name": "WorldGroupId_t"},
    0x74: {"kind": "atomic", "name": "CGlobalSymbol"}, 0x75: {"kind": "atomic", "name": "CGlobalSymbolCaseSensitive"},
}


class Sym:
    """A symbolic value in the replay: what a register holds."""
    def __init__(self, kind, *a):
        self.kind, self.a = kind, a

    def __repr__(self):
        return f'{self.kind}{self.a}'


class Module:
    def __init__(self, path):
        self.im = Image(path)
        self.name = pathlib.Path(path).name
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
        self.md.detail = True

    def data_sections(self):
        # Schema records are writable relocated data: the type scope fills their pointers in.
        for name in ('.data.rel.ro', '.data.rel.ro.local', '.data'):
            a, b = self.im.sec_bytes(name)
            if b:
                yield a, b

    def cstr(self, p, strict=True):
        if not p or p < 0 or p > 0x10000000:
            return None
        s = self.im.read(p, 160)
        if not s:
            return None
        s = s.split(b'\0')[0]
        if strict and not (s and IDENT.match(s)):
            return None
        return s.decode('latin1')

    # ---- static records
    def classes(self):
        im, out = self.im, {}
        for v, b in self.data_sections():
            mb = b
            for off in range(0, len(mb) - 0x70, 8):
                if struct.unpack_from('<Q', mb, off)[0] not in (0, v + off) or \
                        mb[off + 0x28] not in (1, 2, 4, 8, 16, 32, 64, 128, 0xff):  # 0xff = alignment unknown
                    continue
                name = self.cstr(struct.unpack_from('<Q', mb, off + 8)[0])
                if not name or ' ' in name:
                    continue
                size, cnt = struct.unpack_from('<iH', mb, off + 0x20)
                if size <= 0 or size > 0x1000000 or cnt > 4000:
                    continue
                nbase = mb[off + 0x29]
                fptr, bptr = struct.unpack_from('<QQ', mb, off + 0x30)
                inst = struct.unpack_from('<Q', mb, off + 0x68)[0]
                if nbase > 4 or not im.executable(inst):
                    continue
                if cnt and not im.read(fptr, 0x20 * cnt):
                    continue
                fields = []
                for i in range(cnt):
                    e = fptr + 0x20 * i
                    fn = self.cstr(im.r64(e))
                    if not fn:
                        fields = None
                        break
                    fields.append({'name': fn, 'tag': im.r64(e + 8) & (2**64 - 1),
                                   'offset': im.r64(e + 16) & 0xffffffff,
                                   'meta': (im.r64(e + 16) >> 32) & 0xffffffff, 'metaptr': im.r64(e + 24)})
                if fields is None:
                    continue
                parent = None
                if nbase:
                    bp = im.r64(bptr + 8)
                    parent = self.cstr(im.r64(bp + 8)) if bp else None
                    if not parent:
                        parent = '?'   # set at install time; resolved from the replay
                if name in out:
                    continue
                out[name] = {'S': v + off, 'size': size, 'parent': parent, 'fields': fields,
                             'fptr': fptr, 'install': inst}
        return out

    def enums(self):
        """SchemaEnumInfoData_t: {self, name, module, size u8, align u8, pad, count u16, ..., enumerators*}."""
        im, out = self.im, {}
        for v, b in self.data_sections():
            mb = b
            for off in range(0, len(mb) - 0x40, 8):
                if struct.unpack_from('<Q', mb, off)[0] not in (0, v + off):
                    continue
                name = self.cstr(struct.unpack_from('<Q', mb, off + 8)[0])
                if not name or ' ' in name:
                    continue
                sz, al = mb[off + 0x18], mb[off + 0x19]
                if sz not in (1, 2, 4, 8) or al not in (1, 2, 4, 8):
                    continue
                cnt = struct.unpack_from('<H', mb, off + 0x1c)[0]
                eptr = struct.unpack_from('<Q', mb, off + 0x20)[0]
                if not cnt or cnt > 5000 or not im.read(eptr, 0x20 * cnt):
                    continue
                vals = []
                for i in range(cnt):
                    e = eptr + 0x20 * i
                    en = self.cstr(im.r64(e))
                    if not en:
                        vals = None
                        break
                    vals.append((en, im.r64(e + 8)))
                if vals is None or name in out:
                    continue
                out[name] = {'size': sz, 'values': vals}
        return out

    # ---- install-callback replay
    def lea_targets(self, fn, limit=0x800):
        """Every rip-relative lea in the function, in order (for a base class stored straight from a lea)."""
        out = []
        for ins in self.md.disasm(self.im.read(fn, limit) or b'', fn):
            if ins.mnemonic == 'int3':
                break
            o = ins.operands
            if ins.mnemonic == 'lea' and o[1].type == x86.X86_OP_MEM and o[1].mem.base == x86.X86_REG_RIP:
                out.append(ins.address + ins.size + o[1].mem.disp)
        return out

    def typefn(self, fn):
        """A helper the install callback calls with the type scope in rdi; returns the type it builds."""
        cache = self.__dict__.setdefault('_typefn', {})
        if fn not in cache:
            cache[fn] = None
            cache[fn] = self.replay(fn, 0x400, init={'rdi': Sym('scope')}, want_ret=True)
        return cache[fn]

    def replay(self, fn, limit=0x8000, init=None, want_ret=False, init_slots=None):
        """Returns {field_array_disp: Sym} for the stores into [fields + disp]."""
        im = self.im
        R = dict(init) if init else {'rsi': Sym('arg2'), 'rdi': Sym('arg1')}
        prev_ret = False
        table = None   # `switch (edi)` jump table: install callbacks that dispatch on the op
        stack = []
        slots = dict(init_slots or {})   # [rbp+disp] spills
        stores = {}
        code = im.read(fn, limit) or b''
        volatile = ['rax', 'rcx', 'rdx', 'rsi', 'rdi', 'r8', 'r9', 'r10', 'r11']
        for ins in self.md.disasm(code, fn):
            m, ops = ins.mnemonic, ins.operands
            if m == 'int3':
                break
            if m == 'nop':
                continue
            # A ret straight into another prologue: the sweep has left the function (some modules
            # pack functions with no int3 padding between them).
            if prev_ret and (m in ('push', 'endbr64') and ins.op_str in ('rbp', '') or
                             m in ('cmp', 'test') and ins.op_str.startswith('edi')):
                break
            prev_ret = m == 'ret'
            if m == 'movsxd' and not init and ops[1].type == x86.X86_OP_MEM and ops[1].mem.index \
                    and ins.reg_name(ops[1].mem.index) == 'rdi' and ops[1].mem.scale == 4:
                b = R.get(ins.reg_name(ops[1].mem.base).replace('e', 'r', 1))
                table = b.a[-1] if b is not None and b.kind in ('addr', 'str') else None
                continue
            if m == 'jmp' and table is not None and ops[0].type == x86.X86_OP_REG:
                # Op 0 (install the field types) is the table's first entry.
                rel = struct.unpack('<i', im.read(table, 4))[0]
                R['rsi'] = next((v for v in R.values() if v is not None and v.kind == 'arg2'), None)
                return self.replay(table + rel, limit, init=R, init_slots=slots)
            def reg(o):
                return ins.reg_name(o.reg)
            def full(r):
                r = r.replace('e', 'r', 1) if r.startswith('e') and len(r) == 3 else r
                return {'r8d': 'r8', 'r9d': 'r9', 'r10d': 'r10', 'r11d': 'r11', 'r12d': 'r12', 'r13d': 'r13',
                        'r14d': 'r14', 'r15d': 'r15'}.get(r, r)
            if m == 'lea' and ops[1].type == x86.X86_OP_MEM and ops[1].mem.base == x86.X86_REG_RIP:
                t = ins.address + ins.size + ops[1].mem.disp
                s = self.cstr(t, strict=False)
                R[full(reg(ops[0]))] = Sym('str', s, t) if s and IDENT.match(s.encode()) else Sym('addr', t)
            elif m == 'mov' and len(ops) == 2 and ops[0].type == x86.X86_OP_REG:
                dst = full(reg(ops[0]))
                if ops[1].type == x86.X86_OP_REG:
                    R[dst] = R.get(full(reg(ops[1])))
                elif ops[1].type == x86.X86_OP_IMM:
                    R[dst] = Sym('imm', ops[1].imm)
                elif ops[1].type == x86.X86_OP_MEM and ops[1].mem.base not in (0, x86.X86_REG_RIP) \
                        and ops[1].mem.index == 0:
                    bname = full(ins.reg_name(ops[1].mem.base))
                    base = R.get(bname)
                    d = ops[1].mem.disp
                    if bname == 'rbp':
                        R[dst] = slots.get(d)
                        continue
                    # The install record arrives in rsi; earlier branches in the same function may
                    # have clobbered our tracking of it, so an [rsi+0x30]/[rsi+0x50] load counts too.
                    is_rec = (base is not None and base.kind == 'arg2') or (base is None and bname == 'rsi')
                    if is_rec and d == 0x30:
                        R[dst] = Sym('fields')
                    elif is_rec and d == 0x38:
                        R[dst] = Sym('bases')
                    elif base is not None and base.kind in ('addr', 'str') and d == 0:
                        R[dst] = Sym('deref', im.r64(base.a[-1]))
                    elif is_rec and d == 0x50:
                        R[dst] = Sym('scope')
                    elif base is not None and base.kind == 'scope' and d == 0:
                        R[dst] = Sym('vt')
                    elif base is not None and base.kind == 'vt':
                        R[dst] = Sym('method', d)
                    else:
                        R[dst] = None
                else:
                    R[dst] = None
            elif m == 'mov' and len(ops) == 2 and ops[0].type == x86.X86_OP_MEM:
                b = ops[0].mem.base
                if b and b != x86.X86_REG_RIP and ops[0].mem.index == 0 and ops[1].type == x86.X86_OP_REG:
                    bname = full(ins.reg_name(b))
                    base = R.get(bname)
                    if bname == 'rbp':
                        slots[ops[0].mem.disp] = R.get(full(reg(ops[1])))
                    elif base is not None and base.kind == 'fields':
                        stores[ops[0].mem.disp] = R.get(full(reg(ops[1])))
                    elif base is not None and base.kind == 'bases':
                        val = R.get(full(reg(ops[1])))
                        if val is not None and val.kind == 'deref' and val.a[0]:
                            stores[('base', ops[0].mem.disp)] = self.cstr(im.r64(val.a[0] + 8))
            elif m == 'push':
                stack.append(R.get(full(reg(ops[0]))) if ops[0].type == x86.X86_OP_REG else
                             Sym('imm', ops[0].imm) if ops[0].type == x86.X86_OP_IMM else None)
            elif m == 'call' or (want_ret and m == 'jmp' and ops[0].type != x86.X86_OP_IMM):  # tail call
                o = ops[0]
                meth = None
                if o.type == x86.X86_OP_MEM and o.mem.base and o.mem.base != x86.X86_REG_RIP:
                    base = R.get(full(ins.reg_name(o.mem.base)))
                    if base is not None and base.kind == 'vt':
                        meth = o.mem.disp
                elif o.type == x86.X86_OP_REG:
                    v_ = R.get(full(reg(o)))
                    if v_ is not None and v_.kind == 'method':
                        meth = v_.a[0]
                elif o.type == x86.X86_OP_IMM and not want_ret:
                    sc = R.get('rdi')
                    if sc is not None and sc.kind == 'scope':
                        ret = self.typefn(o.imm)
                        for r in volatile:
                            R[r] = None
                        R['rax'] = ret
                        stack = []
                        continue
                args = [R.get(r) for r in ('rsi', 'rdx', 'rcx', 'r8', 'r9')] + list(reversed(stack))
                for r in volatile:
                    R[r] = None
                R['rax'] = Sym('call', meth, tuple(args)) if meth is not None else None
                stack = []
                if m == 'jmp':
                    return R['rax']
            elif m in ('add',) and ops[0].type == x86.X86_OP_REG and reg(ops[0]) == 'rsp':
                stack = []
            elif m == 'ret':
                if want_ret:
                    return R.get('rax')
            elif len(ops) >= 1 and ops[0].type == x86.X86_OP_REG and m not in ('cmp', 'test', 'push'):
                R[full(reg(ops[0]))] = None
        return None if want_ret else stores


# libserver is built against a newer copy of the builtin-type enum than the engine modules: from
# 0x13 on the two disagree, and each module appends its own declared types after the shared ones.
# Learned per module against the last live dump (majority type of every builtin-tagged field).
_ENGINE = {
    0x03: {"kind": "ptr", "inner": "KeyValues"},
    0x13: {"kind": "atomic", "name": "VectorAligned"},
    0x18: {"kind": "atomic", "name": "QuaternionStorage"},
    0x19: {"kind": "atomic", "name": "RadianEuler"}, 0x1a: {"kind": "atomic", "name": "DegreeEuler"},
    0x1f: {"kind": "atomic", "name": "CMotionTransform"},
    0x25: {"kind": "atomic", "name": "CUtlSymbolLarge"},
    0x57: {"kind": "atomic", "name": "CStrongHandle< InfoForResourceTypeIParticleSnapshot >"},
}
MODULE_BUILTIN = {
    'libanimationsystem.so': {
        0x60: {"kind": "atomic", "name": "int32"},
        0x61: {"kind": "class", "name": "ParticleAttributeIndex_t"},
        0x68: {"kind": "class", "name": "AnimParamID"},
        0x6a: {"kind": "class", "name": "CAnimInputDamping"},
        0x6b: {"kind": "class", "name": "CAnimParamHandle"},
        0x6c: {"kind": "enum", "name": "AnimValueSource", "size": 4},
        0x73: {"kind": "atomic", "name": "CBufferString"},
    },
    'libparticles.so': {
        0x61: {"kind": "class", "name": "ParticleAttributeIndex_t"},
        0x63: {"kind": "class", "name": "CPerParticleFloatInput"},
        0x64: {"kind": "class", "name": "CParticleCollectionFloatInput"},
        0x65: {"kind": "class", "name": "CPerParticleVecInput"},
        0x66: {"kind": "class", "name": "CParticleCollectionVecInput"},
        0x67: {"kind": "enum", "name": "ParticleSetMethod_t", "size": 4},
    },
}


def builtin(tag, mod='libserver.so'):
    if mod != 'libserver.so':
        t = MODULE_BUILTIN.get(mod, {}).get(tag) or _ENGINE.get(tag)
        if t:
            return t
    return BUILTIN.get(tag)


def tname(t):
    """The type's spelled name, as the live schema prints it inside a template."""
    if t is None:
        return None
    k = t['kind']
    if k in ('atomic', 'class', 'enum'):
        return t['name']
    if k == 'handle':
        return f"CHandle< {t['inner']} >"
    if k == 'ptr':
        return f"{t['inner']}*"
    return t.get('name')


def decode(sym, enums, depth=0, mod='libserver.so'):
    """Replayed type-scope call -> catalog type (vtable offsets learned against the live dump)."""
    if sym is None or depth > 8 or sym.kind != 'call':
        return None
    off, args = sym.a
    def s(i):
        v = args[i] if i < len(args) else None
        return v.a[0] if v is not None and v.kind == 'str' else None
    def n(i):
        v = args[i] if i < len(args) else None
        return v.a[0] if v is not None and v.kind == 'imm' else None
    def t(i):
        return decode(args[i], enums, depth + 1, mod) if i < len(args) else None
    if off == 0x28:                      # Builtin(id); its tag is id + 3
        if n(0) == 1:
            return {"kind": "atomic", "name": "void"}
        return n(0) is not None and builtin(n(0) + 3, mod) or None
    if off == 0x30:                      # Pointer(T)
        inner = tname(t(0))
        return inner and {"kind": "ptr", "inner": inner}
    if off == 0x38:                      # Atomic(name, size, align)
        return s(0) and {"kind": "atomic", "name": s(0)}
    if off == 0x40:                      # AtomicT(name, size, align, T)
        a = tname(t(3))
        if not (s(0) and a):
            return None
        if s(0) == 'CHandle':
            return {"kind": "handle", "inner": a}
        return {"kind": "atomic", "name": f"{s(0)}< {a} >"}
    if off == 0x48:                      # AtomicCollectionOfT(name, size, align, elemSize, T, fixedCount, fn)
        a = tname(t(4))
        if not (s(0) and a):
            return None
        if n(5):
            return {"kind": "atomic", "name": f"{s(0)}< {a}, {n(5)} >"}
        return {"kind": "atomic", "name": f"{s(0)}< {a} >"}
    if off == 0x50:                      # AtomicTT(name, size, align, T1, T2)
        a, b = tname(t(3)), tname(t(4))
        return s(0) and a and b and {"kind": "atomic", "name": f"{s(0)}< {a}, {b} >"}
    if off == 0x58:                      # AtomicI(name, size, align, N)
        return s(0) and n(3) is not None and {"kind": "atomic", "name": f"{s(0)}< {n(3)} >"}
    if off == 0x60:                      # DeclaredClass(name)
        return s(0) and {"kind": "class", "name": s(0)}
    if off == 0x68:                      # DeclaredEnum(name)
        e = s(0)
        return e and {"kind": "enum", "name": e, "size": enums.get(e, {}).get('size', 0)}
    if off == 0x70:                      # FixedArray(T, count, ...)
        a = tname(t(0))
        return a and n(1) is not None and {"kind": "unknown", "name": f"{a}[{n(1)}]"}
    if off == 0x80:                      # Bitfield(bits)
        return n(0) is not None and {"kind": "unknown", "name": f"bitfield:{n(0)}"}
    return None


# Modules a CS2 dedicated server loads that register schema classes. libserver.so is first: the
# live dump walks the server scope before the global one, and the first record of a name wins.
MODULES = ['libserver.so', 'libanimationsystem.so', 'libengine2.so', 'libmaterialsystem2.so',
           'libmeshsystem.so', 'libnetworksystem.so', 'libparticles.so', 'libpulse_system.so',
           'librendersystemempty.so', 'libresourcesystem.so', 'libscenefilecache.so', 'libscenesystem.so',
           'libschemasystem.so', 'libsoundsystem.so', 'libvphysics2.so', 'libworldrenderer.so']


def build(bindir, prev=None, log=sys.stderr):
    classes, enums, unresolved = {}, {}, []
    mods = []
    for name in MODULES:
        p = pathlib.Path(bindir) / name
        if not p.exists():
            print(f'missing module {name}', file=log)
            continue
        m = Module(str(p))
        cs, es = m.classes(), m.enums()
        print(f'{name}: {len(cs)} classes, {len(es)} enums', file=log)
        mods.append((m, cs))
        for k, e in es.items():
            enums.setdefault(k, e)
    for m, cs in mods:
        for k, c in cs.items():
            if k in classes:
                continue
            st = m.replay(c['install']) if (c['parent'] == '?' or any(f['tag'] == 0 for f in c['fields'])) else {}
            parent = c['parent']
            if parent == '?':
                parent = next((v for kk, v in st.items() if isinstance(kk, tuple) and kk[0] == 'base'), None)
                if not parent:
                    by_s = {v['S']: kk for kk, v in cs.items()}
                    cands = [x for a in m.lea_targets(c['install']) for x in (a, m.im.r64(a))]
                    parent = next((by_s[a] for a in cands if a in by_s and by_s[a] != k), None)
                if not parent:
                    unresolved.append((k, '<parent>'))
            fields = []
            for i, f in enumerate(c['fields']):
                tag = f['tag']
                if tag == 0:
                    t = decode(st.get(0x20 * i + 8), enums, mod=m.name)
                elif tag >> 60 == 0xf:
                    t = builtin(tag & 0xff, m.name)
                else:
                    t = None
                if t is None:
                    unresolved.append((k, f['name']))
                    t = {"kind": "unknown"}
                ty = {"kind": t["kind"]}
                if t.get("name") is not None:
                    ty["name"] = t["name"]
                if t.get("inner") is not None:
                    ty["inner"] = t["inner"]
                if t["kind"] == "enum":
                    size = enums.get(t.get("name"), {}).get("size")
                    if size in (1, 2, 4, 8):
                        ty["size"] = size
                off = f['offset']
                fields.append({"name": f['name'], "offset": off - (1 << 32) if off >= 1 << 31 else off, "type": ty})
            entry = {}
            if parent:
                entry["parent"] = parent
            entry["fields"] = fields
            classes[k] = entry
    carried = []
    if prev:
        # A few fieldless classes (script-API shims like CCSPlayerPawn_API, Pulse function libraries)
        # are built by static constructors into .bss, so there is no record on disk to read. Keep
        # those from the previous catalog while their name is still in a module.
        blobs = [m.im for m, _ in mods]
        for k, c in prev.items():
            if k in classes or c.get('fields'):
                continue
            needle = b'\0' + k.encode() + b'\0'
            if any(needle in b for im in blobs for _, b, _ in im.segs):
                classes[k] = dict(c)
                carried.append(k)
    enum_out = {k: {"size": e["size"], "values": {n: v for n, v in sorted(e["values"])}}
                for k, e in sorted(enums.items())}
    return dict(sorted(classes.items())), enum_out, unresolved, carried


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--bin', required=True, help='directory holding the module .so files')
    ap.add_argument('--out', default='games/cs2/gamedata/schema-catalog.json')
    ap.add_argument('--enums', default='games/cs2/gamedata/schema-enums.json')
    ap.add_argument('--prev', help='previous catalog (default: --out as it stands), for runtime-built classes')
    a = ap.parse_args()
    prev_path = pathlib.Path(a.prev or a.out)
    prev = json.loads(prev_path.read_text()) if prev_path.exists() else None
    classes, enums, unresolved, carried = build(a.bin, prev)
    pathlib.Path(a.out).write_text(json.dumps(classes, indent=2))
    pathlib.Path(a.enums).write_text(json.dumps(enums, indent=2))
    print(f'{len(classes)} classes, {sum(len(c["fields"]) for c in classes.values())} fields, '
          f'{len(enums)} enums; {len(unresolved)} unresolved; {len(carried)} fieldless classes kept from '
          f'the previous catalog', file=sys.stderr)
    for k, f in unresolved[:40]:
        print(f'  unresolved {k}.{f}', file=sys.stderr)


if __name__ == '__main__':
    main()
