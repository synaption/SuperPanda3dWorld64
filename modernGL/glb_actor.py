"""Small GLB skin/animation reader used by the ModernGL front end."""

import io
import json
import math
import struct

import numpy as np
from PIL import Image


_DTYPES = {5121: np.uint8, 5123: np.uint16, 5125: np.uint32, 5126: np.float32}
_WIDTHS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}


def _quat_matrix(q):
    x, y, z, w = q
    n = x*x + y*y + z*z + w*w
    if n < 1e-12:
        return np.eye(4, dtype="f4")
    s = 2.0 / n
    xx, yy, zz = x*x*s, y*y*s, z*z*s
    xy, xz, yz = x*y*s, x*z*s, y*z*s
    wx, wy, wz = w*x*s, w*y*s, w*z*s
    return np.array([
        [1-yy-zz, xy-wz, xz+wy, 0],
        [xy+wz, 1-xx-zz, yz-wx, 0],
        [xz-wy, yz+wx, 1-xx-yy, 0],
        [0, 0, 0, 1],
    ], dtype="f4")


def _local_matrix(translation, rotation, scale):
    result = _quat_matrix(rotation)
    result[:3, :3] *= np.asarray(scale, dtype="f4")[None, :]
    result[:3, 3] = translation
    return result


def _slerp(a, b, t):
    dot = float(np.dot(a, b))
    if dot < 0:
        b, dot = -b, -dot
    if dot > 0.9995:
        q = a + (b - a) * t
        return q / np.linalg.norm(q)
    theta = math.acos(max(-1.0, min(1.0, dot)))
    return (a * math.sin((1-t)*theta) + b * math.sin(t*theta)) / math.sin(theta)


class GLBActor:
    def __init__(self, path):
        raw = open(path, "rb").read()
        magic, version, _ = struct.unpack_from("<4sII", raw)
        if magic != b"glTF" or version != 2:
            raise ValueError("Mario model is not a glTF 2.0 GLB")
        json_len, _ = struct.unpack_from("<II", raw, 12)
        self.doc = json.loads(raw[20:20+json_len])
        bin_at = 20 + json_len
        bin_len, _ = struct.unpack_from("<II", raw, bin_at)
        self.blob = memoryview(raw)[bin_at+8:bin_at+8+bin_len]
        self.nodes = self.doc["nodes"]
        self.parents = [-1] * len(self.nodes)
        for parent, node in enumerate(self.nodes):
            for child in node.get("children", []):
                self.parents[child] = parent

        skin = self.doc["skins"][0]
        self.joints = skin["joints"]
        self.inverse_bind = self.accessor(skin["inverseBindMatrices"]).reshape(-1, 4, 4).transpose(0, 2, 1)
        self.animations = {a["name"]: a for a in self.doc.get("animations", [])}
        self.node_defaults = [(
            np.asarray(n.get("translation", [0, 0, 0]), dtype="f4"),
            np.asarray(n.get("rotation", [0, 0, 0, 1]), dtype="f4"),
            np.asarray(n.get("scale", [1, 1, 1]), dtype="f4"),
        ) for n in self.nodes]
        self.primitives = self.doc["meshes"][0]["primitives"]

    def accessor(self, index):
        acc = self.doc["accessors"][index]
        view = self.doc["bufferViews"][acc["bufferView"]]
        dtype = np.dtype(_DTYPES[acc["componentType"]]).newbyteorder("<")
        width = _WIDTHS[acc["type"]]
        offset = view.get("byteOffset", 0) + acc.get("byteOffset", 0)
        stride = view.get("byteStride", dtype.itemsize * width)
        if stride == dtype.itemsize * width:
            return np.frombuffer(self.blob, dtype, acc["count"] * width, offset).reshape(acc["count"], width).copy()
        return np.ndarray((acc["count"], width), dtype, self.blob, offset, (stride, dtype.itemsize)).copy()

    def image(self, texture_index):
        tex = self.doc["textures"][texture_index]
        image = self.doc["images"][tex["source"]]
        view = self.doc["bufferViews"][image["bufferView"]]
        data = self.blob[view.get("byteOffset", 0):view.get("byteOffset", 0)+view["byteLength"]]
        return Image.open(io.BytesIO(bytes(data))).convert("RGBA")

    def material(self, index):
        pbr = self.doc.get("materials", [])[index].get("pbrMetallicRoughness", {})
        factor = pbr.get("baseColorFactor", [1, 1, 1, 1])
        texture = pbr.get("baseColorTexture", {}).get("index")
        return np.asarray(factor, dtype="f4"), texture

    def bone_matrices(self, name, elapsed, loop=True):
        translations = [v[0].copy() for v in self.node_defaults]
        rotations = [v[1].copy() for v in self.node_defaults]
        scales = [v[2].copy() for v in self.node_defaults]
        animation = self.animations.get(name)
        if animation:
            duration = 0.0
            tracks = []
            for channel in animation["channels"]:
                sampler = animation["samplers"][channel["sampler"]]
                times = self.accessor(sampler["input"]).ravel()
                values = self.accessor(sampler["output"])
                duration = max(duration, float(times[-1]))
                tracks.append((channel["target"], times, values))
            t = elapsed % duration if loop and duration > 0 else min(elapsed, duration)
            for target, times, values in tracks:
                hi = min(int(np.searchsorted(times, t, side="right")), len(times)-1)
                lo = max(0, hi-1)
                span = float(times[hi] - times[lo])
                f = 0.0 if span <= 0 else (t - float(times[lo])) / span
                value = _slerp(values[lo], values[hi], f) if target["path"] == "rotation" else values[lo] + (values[hi]-values[lo])*f
                arrays = {"translation": translations, "rotation": rotations, "scale": scales}
                arrays[target["path"]][target["node"]] = value

        global_mats = [None] * len(self.nodes)
        for i in range(len(self.nodes)):
            local = _local_matrix(translations[i], rotations[i], scales[i])
            parent = self.parents[i]
            global_mats[i] = local if parent < 0 else global_mats[parent] @ local
        return np.stack([global_mats[j] @ self.inverse_bind[k] for k, j in enumerate(self.joints)])
