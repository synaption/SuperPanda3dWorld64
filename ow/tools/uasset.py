"""UE5 .uasset reader: package header, name table, imports, exports and
UE 5.4 tagged-property streams.

Enough of the format to pull constants and blueprint graphs out of an uncooked
editor asset. Not a general-purpose loader -- it reads, never writes."""
import struct
import sys

TAG = 0x9E2A83C1

# EPropertyTagFlags, UE 5.4. Only the two we depend on are named.
FLAG_HAS_PROPERTY_GUID = 1 << 1
FLAG_BOOL_TRUE = 1 << 4


class R:
    def __init__(self, d, p=0):
        self.d, self.p = d, p

    def i32(self):
        v = struct.unpack_from("<i", self.d, self.p)[0]; self.p += 4; return v

    def u32(self):
        v = struct.unpack_from("<I", self.d, self.p)[0]; self.p += 4; return v

    def i64(self):
        v = struct.unpack_from("<q", self.d, self.p)[0]; self.p += 8; return v

    def f64(self):
        v = struct.unpack_from("<d", self.d, self.p)[0]; self.p += 8; return v

    def f32(self):
        v = struct.unpack_from("<f", self.d, self.p)[0]; self.p += 4; return v

    def u8(self):
        v = self.d[self.p]; self.p += 1; return v

    def s(self):
        n = self.i32()
        if n == 0:
            return ""
        if n < 0:
            n = -n
            v = self.d[self.p:self.p + 2 * n].decode("utf-16-le", "replace"); self.p += 2 * n
        else:
            v = self.d[self.p:self.p + n].decode("utf-8", "replace"); self.p += n
        return v.rstrip("\x00")


class Package:
    def __init__(self, path):
        self.d = open(path, "rb").read()
        r = R(self.d)
        assert r.u32() == TAG, "not a uasset"
        legacy = r.i32()
        if legacy != -4:
            r.i32()  # LegacyUE3Version
        self.ue4ver = r.i32()
        if legacy <= -8:
            self.ue5ver = r.i32()
        r.i32()  # licensee
        ncustom = r.i32()
        r.p += ncustom * 20  # guid(16) + version(4)
        self.total_header = r.i32()
        self.folder = r.s()
        self.flags = r.u32()
        self.name_count, self.name_off = r.i32(), r.i32()
        self.soft_count, self.soft_off = r.i32(), r.i32()
        self.localization_id = r.s()
        self.gather_count, self.gather_off = r.i32(), r.i32()
        self.export_count, self.export_off = r.i32(), r.i32()
        self.import_count, self.import_off = r.i32(), r.i32()
        self._read_names()

    def _read_names(self):
        r = R(self.d, self.name_off)
        self.names = []
        for _ in range(self.name_count):
            self.names.append(r.s())
            r.p += 4  # hashes
        self.nidx = {n: i for i, n in enumerate(self.names)}

    def name(self, i):
        return self.names[i] if 0 <= i < len(self.names) else f"<{i}>"

    def fname(self, r):
        i, num = r.u32(), r.u32()
        return self.name(i)

    def imports(self):
        if hasattr(self, "_imports"):
            return self._imports
        st = self.import_stride
        out = []
        for i in range(self.import_count):
            r = R(self.d, self.import_off + i * st)
            pkg = self.fname(r); cls = self.fname(r)
            outer = r.i32(); obj = self.fname(r)
            out.append(dict(pkg=pkg, cls=cls, name=obj, outer=outer))
        self._imports = out
        return out

    def resolve(self, idx):
        """FPackageIndex -> readable name."""
        if idx == 0:
            return None
        if idx < 0:
            im = self.imports()
            i = -idx - 1
            return im[i]["name"] if i < len(im) else f"<imp{i}>"
        ex = self.exports()
        return ex[idx - 1]["name"] if idx - 1 < len(ex) else f"<exp{idx}>"

    def classof(self, e):
        return self.resolve(e["cls"])

    def _stride(self):
        """FObjectExport size varies with engine version gates. Detect it by
        requiring the whole table to describe a contiguous run of export data
        starting at TotalHeaderSize and ending at the package tail tag."""
        n = len(self.d)
        best = (None, [])
        for stride in range(44, 200, 4):
            ok, cur = [], self.total_header
            for i in range(self.export_count):
                base = self.export_off + i * stride
                try:
                    r = R(self.d, base + 16)
                    nm = self.fname(r); r.u32()
                    size = r.i64(); off = r.i64()
                except Exception:
                    break
                if off != cur or size <= 0 or off + size > n:
                    break
                ok.append((nm, off, size, base))
                cur = off + size
            if len(ok) == self.export_count and cur >= n - 8:
                return stride, ok
            if len(ok) > len(best[1]):
                best = (stride, ok)
        return best

    @property
    def import_stride(self):
        if self.import_count <= 0:
            return 0
        return (self.export_off - self.import_off) // self.import_count

    def exports(self):
        stride, recs = self._stride()
        out = []
        for i, (nm, off, size, base) in enumerate(recs):
            cls_idx = struct.unpack_from("<i", self.d, base)[0]
            out.append(dict(name=nm, cls=cls_idx, off=off, size=size, idx=i))
        return out


def typename(pkg, r):
    """UE5.4 FPropertyTypeName: FName + u32 param count + recursive params."""
    nm = pkg.fname(r)
    cnt = r.u32()
    params = [typename(pkg, r) for _ in range(cnt)]
    return nm + (("<" + ",".join(params) + ">") if params else "")


def props54(pkg, start, end, limit=4000):
    """Walk a UE5.4 tagged-property stream.

    Record: FName name | FPropertyTypeName type | i32 size | u8 flags
            | [FGuid if flags&2] | value(size)
    """
    d, out = pkg.d, []
    r = R(d, start)
    while r.p < end and len(out) < limit:
        p0 = r.p
        try:
            nm = pkg.fname(r)
        except Exception:
            break
        if nm == "None" or nm.startswith("<"):
            break
        try:
            ty = typename(pkg, r)
            size = r.i32()
            flags = r.u8()
        except Exception:
            break
        if flags & FLAG_HAS_PROPERTY_GUID:
            r.p += 16
        if size < 0 or r.p + size > end:
            break
        body = r.p
        base = ty.split("<")[0]
        arg = ty.split("<")[1].rstrip(">") if "<" in ty else None
        try:
            if base == "DoubleProperty":
                val = struct.unpack_from("<d", d, body)[0]
            elif base == "FloatProperty":
                val = struct.unpack_from("<f", d, body)[0]
            elif base in ("IntProperty", "Int32Property"):
                val = struct.unpack_from("<i", d, body)[0]
            elif base == "BoolProperty":
                # A bool has no value payload (size 0); UE 5.4 stores it in
                # the tag's flags byte as EPropertyTagFlags::BoolTrue.
                val = bool(flags & FLAG_BOOL_TRUE)
            elif base == "NameProperty":
                val = pkg.name(struct.unpack_from("<I", d, body)[0])
            elif base == "StrProperty":
                val = R(d, body).s()
            elif base == "ObjectProperty":
                val = struct.unpack_from("<i", d, body)[0]
            elif base == "StructProperty" and arg and arg.split(",")[0] in ("Vector", "Rotator"):
                val = tuple(round(x, 4) for x in struct.unpack_from("<3d", d, body))
            else:
                val = d[body:body + min(size, 40)].hex()
        except Exception:
            val = "?"
        out.append(dict(name=nm, type=ty, val=val, off=body, size=size, rec=p0))
        r.p = body + size
    return out
