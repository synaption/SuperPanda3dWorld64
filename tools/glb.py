"""A small glTF 2.0 / GLB writer.

Only what the actor exporter needs: a skinned mesh, a joint hierarchy, and
rotation/translation animation tracks.  Written by hand rather than pulling in
a dependency, since the binary layout is short and being able to control
padding and accessor bounds exactly is worth more here than the convenience.
"""

import json
import struct

# glTF component types.
UNSIGNED_BYTE = 5121
UNSIGNED_SHORT = 5123
UNSIGNED_INT = 5125
FLOAT = 5126

_COMPONENT_SIZE = {
    UNSIGNED_BYTE: 1,
    UNSIGNED_SHORT: 2,
    UNSIGNED_INT: 4,
    FLOAT: 4,
}

_TYPE_COUNT = {
    "SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16,
}

ARRAY_BUFFER = 34962
ELEMENT_ARRAY_BUFFER = 34963


class GLB:
    def __init__(self):
        self.json = {
            "asset": {"version": "2.0", "generator": "sm64py actor exporter"},
            "scene": 0,
            "scenes": [{"nodes": []}],
            "nodes": [],
            "meshes": [],
            "skins": [],
            "animations": [],
            "materials": [],
            "textures": [],
            "images": [],
            "samplers": [],
            "accessors": [],
            "bufferViews": [],
            "buffers": [],
        }
        self._blob = bytearray()

    # -- buffer plumbing ----------------------------------------------------

    def _align(self, alignment=4):
        while len(self._blob) % alignment:
            self._blob.append(0)

    def add_view(self, data, target=None, stride=None):
        self._align()
        offset = len(self._blob)
        self._blob.extend(data)
        view = {"buffer": 0, "byteOffset": offset, "byteLength": len(data)}
        if target is not None:
            view["target"] = target
        if stride is not None:
            view["byteStride"] = stride
        self.json["bufferViews"].append(view)
        return len(self.json["bufferViews"]) - 1

    def add_accessor(self, data, component_type, type_name, count,
                     minimum=None, maximum=None, target=None):
        view = self.add_view(data, target)
        accessor = {
            "bufferView": view,
            "componentType": component_type,
            "count": count,
            "type": type_name,
        }
        # The spec requires min/max on POSITION; harmless elsewhere.
        if minimum is not None:
            accessor["min"] = list(minimum)
        if maximum is not None:
            accessor["max"] = list(maximum)
        self.json["accessors"].append(accessor)
        return len(self.json["accessors"]) - 1

    def add_array(self, values, component_type, type_name, target=None,
                  with_bounds=False):
        """Pack a flat/nested numeric sequence into an accessor."""
        per = _TYPE_COUNT[type_name]
        flat = []
        for item in values:
            if per == 1:
                flat.append(item)
            else:
                flat.extend(item)

        fmt = {
            FLOAT: "<f", UNSIGNED_INT: "<I",
            UNSIGNED_SHORT: "<H", UNSIGNED_BYTE: "<B",
        }[component_type]
        data = b"".join(struct.pack(fmt, v) for v in flat)

        minimum = maximum = None
        if with_bounds and values:
            if per == 1:
                minimum, maximum = [min(flat)], [max(flat)]
            else:
                cols = list(zip(*values))
                minimum = [min(c) for c in cols]
                maximum = [max(c) for c in cols]

        return self.add_accessor(data, component_type, type_name, len(values),
                                 minimum, maximum, target)

    def add_image(self, png_bytes, name=None):
        view = self.add_view(png_bytes)
        image = {"bufferView": view, "mimeType": "image/png"}
        if name:
            image["name"] = name
        self.json["images"].append(image)
        return len(self.json["images"]) - 1

    # -- output -------------------------------------------------------------

    def serialise(self):
        for key in ("skins", "animations", "materials", "textures", "images",
                    "samplers"):
            if not self.json[key]:
                del self.json[key]

        self.json["buffers"] = [{"byteLength": len(self._blob)}]

        json_bytes = json.dumps(self.json, separators=(",", ":")).encode("utf-8")
        json_bytes += b" " * (-len(json_bytes) % 4)

        bin_bytes = bytes(self._blob)
        bin_bytes += b"\x00" * (-len(bin_bytes) % 4)

        total = 12 + 8 + len(json_bytes) + 8 + len(bin_bytes)
        out = bytearray()
        out.extend(struct.pack("<4sII", b"glTF", 2, total))
        out.extend(struct.pack("<II", len(json_bytes), 0x4E4F534A))  # 'JSON'
        out.extend(json_bytes)
        out.extend(struct.pack("<II", len(bin_bytes), 0x004E4942))   # 'BIN'
        out.extend(bin_bytes)
        return bytes(out)

    def write(self, path):
        with open(path, "wb") as fh:
            fh.write(self.serialise())
