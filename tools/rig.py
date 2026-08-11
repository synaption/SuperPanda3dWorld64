"""Posing Mario's skeleton from outside the decomp's animation data.

Two tools need this: `retarget_anim.py`, which copies a clip off another rig,
and `author_skate.py`, which makes one up from scratch. Both face the same
obstacle, and it is worth stating once here rather than in each of them.

**Mario's bind pose is not a pose.** The exporter writes every joint with an
identity rotation and its offset along local +X, so unposed Mario is a stack of
parts all pointing the same way. Nothing can be measured against it. What can
be measured is his A-pose *clip* -- MARIO_ANIM_A_POSE, 0x0E -- a real,
untwisted, upright pose. Reading each joint's world rotation there recovers the
one thing the bind pose withholds: how that joint's local axes sit relative to
the body. Every SM64 joint puts its bone on local +X, but the roll about that
axis is per-joint and mirrored between the left and right limbs.

So each joint gets a constant K = F(A-pose)^T @ R(A-pose), where F is the
joint's *anatomical* frame -- bone direction, plus a reference axis projected
perpendicular to it. Give a bone an anatomical frame and `F @ K` is the world
rotation that puts it there, whether that frame was copied off another rig or
worked out from a stride equation.

Mario's world axes throughout: +X is his left, +Y is up, +Z is the way he
faces.
"""

import json
import math
import struct

import numpy as np

# The game ticks at 30 Hz and every clip is authored to it.
FRAME_RATE = 30.0


def panda_frame_count(last_key_time):
    """How many frames Panda3D will actually build for a clip that long.

    panda3d-gltf throws the glTF key times away and resamples every channel
    onto its own uniform grid, sizing the table as

        num_frames = max(ceil(max_time * fps), 1)     # gltf/_converter.py

    which reads the last key's *time* as the clip's *duration*. A clip authored
    as N poses one tick apart has its last key at (N-1)/FRAME_RATE, so Panda
    builds an N-1 frame bundle and never shows that final pose. Counting keys
    instead -- the obvious thing, and what this used to do -- leaves the sidecar
    one frame ahead of the AnimBundle the game is playing, and the extra frame
    is one the Actor cannot be posed to.

    For a cyclic clip the truncation is right anyway: its last pose is a repeat
    of its first, and dropping it is what makes the loop not stutter.

    The time must arrive as float32 (straight off the accessor, or through
    float(), which widens exactly) or the ceil can disagree with Panda's on
    clips whose length lands on a frame boundary -- which is most of them.
    """
    return max(math.ceil(last_key_time * FRAME_RATE), 1)


# Mario's A-pose clip, which stands in for the bind pose he does not usefully
# have, and MARIO_ANIM_WALKING, whose average clearance sets the height a made
# clip is dropped to.
REFERENCE_CLIP = "anim_0E"
GROUND_CLIP = "anim_48"

# The joint whose translation carries the whole body.
ROOT = "root"

LEFT = np.array([1.0, 0.0, 0.0])
UP = np.array([0.0, 1.0, 0.0])
FORWARD = np.array([0.0, 0.0, 1.0])

# Reference axes for the anatomical frame. Forward is the usual choice; a bone
# already pointing along it -- a foot does -- needs the fallback instead.
PARALLEL = 0.8

COMPONENT_DTYPE = {
    5120: "<i1", 5121: "<u1", 5122: "<i2", 5123: "<u2", 5125: "<u4", 5126: "<f4",
}
TYPE_COUNT = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}

FLOAT = 5126


# -- glTF ---------------------------------------------------------------------


class Gltf:
    """A GLB opened for reading, and for appending animations to."""

    def __init__(self, path):
        with open(path, "rb") as fh:
            data = fh.read()

        magic, version, _ = struct.unpack("<4sII", data[:12])
        if magic != b"glTF" or version != 2:
            raise ValueError(f"{path} is not a glTF 2.0 binary")

        offset = 12
        self.json = None
        self.blob = bytearray()
        while offset < len(data):
            length, kind = struct.unpack("<II", data[offset:offset + 8])
            chunk = data[offset + 8:offset + 8 + length]
            if kind == 0x4E4F534A:
                self.json = json.loads(chunk)
            elif kind == 0x004E4942:
                self.blob = bytearray(chunk)
            offset += 8 + length

        self.nodes = self.json["nodes"]
        self.index = {n["name"]: i for i, n in enumerate(self.nodes) if "name" in n}
        self.parent = {}
        for i, node in enumerate(self.nodes):
            for child in node.get("children", []):
                self.parent[child] = i

    # -- reading ------------------------------------------------------------

    def read(self, accessor_index):
        accessor = self.json["accessors"][accessor_index]
        view = self.json["bufferViews"][accessor["bufferView"]]
        start = view.get("byteOffset", 0) + accessor.get("byteOffset", 0)
        count = TYPE_COUNT[accessor["type"]]
        values = np.frombuffer(
            self.blob, dtype=np.dtype(COMPONENT_DTYPE[accessor["componentType"]]),
            count=accessor["count"] * count, offset=start,
        )
        return values.reshape(accessor["count"], count).astype(np.float64)

    def animation(self, name):
        for anim in self.json.get("animations", []):
            if anim.get("name") == name:
                return anim
        raise KeyError(f"no animation named {name!r}")

    def tracks(self, name):
        """{node: {path: (times, values)}} for one clip."""
        anim = self.animation(name)
        out = {}
        for channel in anim["channels"]:
            sampler = anim["samplers"][channel["sampler"]]
            target = channel["target"]
            out.setdefault(target["node"], {})[target["path"]] = (
                self.read(sampler["input"])[:, 0], self.read(sampler["output"])
            )
        return out

    def rest_local(self, node_index):
        node = self.nodes[node_index]
        return (np.array(node.get("translation", [0.0, 0.0, 0.0])),
                np.array(node.get("rotation", [0.0, 0.0, 0.0, 1.0])))

    def world(self, pose=None):
        """World matrices for every node, optionally with a pose applied.

        `pose` is {node: (translation, quaternion)} and overrides the rest
        transform of the nodes it names.
        """
        cache = {}

        def matrix(i):
            if i in cache:
                return cache[i]
            translation, rotation = self.rest_local(i)
            if pose and i in pose:
                translation, rotation = pose[i]
            local = np.eye(4)
            local[:3, :3] = quat_to_matrix(rotation)
            local[:3, 3] = translation
            if i in self.parent:
                local = matrix(self.parent[i]) @ local
            cache[i] = local
            return local

        for i in range(len(self.nodes)):
            matrix(i)
        return cache

    def order(self):
        """Nodes, parents before children."""
        out = []

        def walk(node):
            out.append(node)
            for child in self.nodes[node].get("children", []):
                walk(child)

        roots = set(range(len(self.nodes))) - set(self.parent)
        for node in sorted(roots):
            walk(node)
        return out

    # -- writing ------------------------------------------------------------

    def add_array(self, values, type_name, with_bounds=False):
        data = np.ascontiguousarray(values, dtype="<f4").tobytes()
        while len(self.blob) % 4:
            self.blob.append(0)
        self.json["bufferViews"].append({
            "buffer": 0, "byteOffset": len(self.blob), "byteLength": len(data),
        })
        self.blob.extend(data)

        accessor = {
            "bufferView": len(self.json["bufferViews"]) - 1,
            "componentType": FLOAT,
            "count": len(values),
            "type": type_name,
        }
        if with_bounds:
            array = np.atleast_2d(values)
            accessor["min"] = [float(v) for v in array.min(axis=0)]
            accessor["max"] = [float(v) for v in array.max(axis=0)]
        self.json["accessors"].append(accessor)
        return len(self.json["accessors"]) - 1

    def replace_animation(self, animation):
        """Add a clip, or swap out the one already using its name."""
        animations = self.json.setdefault("animations", [])
        for i, existing in enumerate(animations):
            if existing.get("name") == animation["name"]:
                animations[i] = animation
                return
        animations.append(animation)

    def compact(self):
        """Drop accessors and buffer views nothing references any more.

        Replacing a clip strands the data the old one used.  Without this the
        .glb grows by the size of a clip every time a tool is re-run, which
        matters because the file is checked in: a rebuild that changes nothing
        should produce the same bytes, not a diff of accumulated litter.
        """
        used = set()
        for mesh in self.json.get("meshes", []):
            for primitive in mesh["primitives"]:
                used.update(primitive["attributes"].values())
                if "indices" in primitive:
                    used.add(primitive["indices"])
                for target in primitive.get("targets", []):
                    used.update(target.values())
        for skin in self.json.get("skins", []):
            if "inverseBindMatrices" in skin:
                used.add(skin["inverseBindMatrices"])
        for animation in self.json.get("animations", []):
            for sampler in animation["samplers"]:
                used.add(sampler["input"])
                used.add(sampler["output"])

        blob = bytearray()
        views, kept_views = [], {}

        def keep(index):
            if index not in kept_views:
                view = dict(self.json["bufferViews"][index])
                start = view.get("byteOffset", 0)
                data = bytes(self.blob[start:start + view["byteLength"]])
                while len(blob) % 4:
                    blob.append(0)
                view["byteOffset"] = len(blob)
                blob.extend(data)
                views.append(view)
                kept_views[index] = len(views) - 1
            return kept_views[index]

        accessors, moved = [], {}
        for old in sorted(used):
            accessor = dict(self.json["accessors"][old])
            if "bufferView" in accessor:
                accessor["bufferView"] = keep(accessor["bufferView"])
            accessors.append(accessor)
            moved[old] = len(accessors) - 1
        for image in self.json.get("images", []):
            if "bufferView" in image:
                image["bufferView"] = keep(image["bufferView"])

        for mesh in self.json.get("meshes", []):
            for primitive in mesh["primitives"]:
                primitive["attributes"] = {
                    name: moved[a] for name, a in primitive["attributes"].items()
                }
                if "indices" in primitive:
                    primitive["indices"] = moved[primitive["indices"]]
                for target in primitive.get("targets", []):
                    target.update({name: moved[a] for name, a in target.items()})
        for skin in self.json.get("skins", []):
            if "inverseBindMatrices" in skin:
                skin["inverseBindMatrices"] = moved[skin["inverseBindMatrices"]]
        for animation in self.json.get("animations", []):
            for sampler in animation["samplers"]:
                sampler["input"] = moved[sampler["input"]]
                sampler["output"] = moved[sampler["output"]]

        self.json["accessors"] = accessors
        self.json["bufferViews"] = views
        self.blob = blob

    def write(self, path):
        self.compact()
        self.json["buffers"] = [{"byteLength": len(self.blob)}]
        json_bytes = json.dumps(self.json, separators=(",", ":")).encode("utf-8")
        json_bytes += b" " * (-len(json_bytes) % 4)
        blob = bytes(self.blob) + b"\x00" * (-len(self.blob) % 4)

        total = 12 + 8 + len(json_bytes) + 8 + len(blob)
        with open(path, "wb") as fh:
            fh.write(struct.pack("<4sII", b"glTF", 2, total))
            fh.write(struct.pack("<II", len(json_bytes), 0x4E4F534A))
            fh.write(json_bytes)
            fh.write(struct.pack("<II", len(blob), 0x004E4942))
            fh.write(blob)


# -- maths --------------------------------------------------------------------


def quat_to_matrix(q):
    x, y, z, w = q
    return np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])


def matrix_to_quat(m):
    trace = m[0, 0] + m[1, 1] + m[2, 2]
    if trace > 0.0:
        s = np.sqrt(trace + 1.0) * 2.0
        q = [(m[2, 1] - m[1, 2]) / s, (m[0, 2] - m[2, 0]) / s,
             (m[1, 0] - m[0, 1]) / s, 0.25 * s]
    elif m[0, 0] > m[1, 1] and m[0, 0] > m[2, 2]:
        s = np.sqrt(1.0 + m[0, 0] - m[1, 1] - m[2, 2]) * 2.0
        q = [0.25 * s, (m[0, 1] + m[1, 0]) / s, (m[0, 2] + m[2, 0]) / s,
             (m[2, 1] - m[1, 2]) / s]
    elif m[1, 1] > m[2, 2]:
        s = np.sqrt(1.0 + m[1, 1] - m[0, 0] - m[2, 2]) * 2.0
        q = [(m[0, 1] + m[1, 0]) / s, 0.25 * s, (m[1, 2] + m[2, 1]) / s,
             (m[0, 2] - m[2, 0]) / s]
    else:
        s = np.sqrt(1.0 + m[2, 2] - m[0, 0] - m[1, 1]) * 2.0
        q = [(m[0, 2] + m[2, 0]) / s, (m[1, 2] + m[2, 1]) / s, 0.25 * s,
             (m[1, 0] - m[0, 1]) / s]
    q = np.array(q)
    return q / np.linalg.norm(q)


def slerp(a, b, t):
    if np.dot(a, b) < 0.0:
        b = -b
    dot = np.clip(np.dot(a, b), -1.0, 1.0)
    if dot > 0.9995:
        out = a + (b - a) * t
        return out / np.linalg.norm(out)
    theta = np.arccos(dot) * t
    perpendicular = b - a * dot
    perpendicular /= np.linalg.norm(perpendicular)
    return a * np.cos(theta) + perpendicular * np.sin(theta)


def sample_track(times, values, t, rotation):
    """Value of a keyframed track at time t, held at both ends."""
    if t <= times[0]:
        return values[0]
    if t >= times[-1]:
        return values[-1]
    i = int(np.searchsorted(times, t)) - 1
    span = times[i + 1] - times[i]
    alpha = 0.0 if span <= 0.0 else (t - times[i]) / span
    if rotation:
        return slerp(values[i], values[i + 1], alpha)
    return values[i] + (values[i + 1] - values[i]) * alpha


def roll_matrix(degrees):
    """Rotation about a bone's own axis, which is local +X."""
    angle = np.radians(degrees)
    c, s = np.cos(angle), np.sin(angle)
    return np.array([[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]])


def anatomical_frame(direction, reference):
    """Frame [bone, forward, bone x forward] for a bone pointing `direction`.

    This is the body-relative frame everything is measured against. Its whole
    job is to fix the roll: the bone direction alone leaves a free spin about
    the bone, and projecting a reference axis perpendicular to it pins that
    down the same way every time.
    """
    bone = direction / np.linalg.norm(direction)
    forward = reference - bone * np.dot(reference, bone)
    forward /= np.linalg.norm(forward)
    return np.column_stack([bone, forward, np.cross(bone, forward)])


def reference_axis(direction):
    """Forward, unless the bone points along it -- a foot does."""
    return UP if abs(np.dot(direction, FORWARD)) > PARALLEL else FORWARD


# -- Mario's joints -----------------------------------------------------------


def pose_at(gltf, tracks, t):
    """{node: (translation, quaternion)} for a clip at time t."""
    pose = {}
    for node, paths in tracks.items():
        translation, rotation = gltf.rest_local(node)
        if "translation" in paths:
            translation = sample_track(*paths["translation"], t, rotation=False)
        if "rotation" in paths:
            rotation = sample_track(*paths["rotation"], t, rotation=True)
        pose[node] = (translation, rotation)
    return pose


def clip_length(tracks):
    return max(times[-1] for paths in tracks.values() for times, _ in paths.values())


def a_pose(target):
    return pose_at(target, target.tracks(REFERENCE_CLIP), 0.0)


def joint_constants(target, axes):
    """Per-joint K, measured off the A-pose, for the joints `axes` names.

    `axes` is {joint: reference axis}. A joint has to be measured with the same
    axis it will later be posed with, or the roll being compared is not the
    same measurement.
    """
    rest = target.world(a_pose(target))
    constants = {}
    for name, axis in axes.items():
        if name not in target.index:
            raise KeyError(f"the rig has no joint {name!r}")
        rotation = rest[target.index[name]][:3, :3]
        direction = rotation[:, 0]  # every SM64 joint puts its bone on local +X
        constants[name] = anatomical_frame(direction, axis).T @ rotation
    return constants


def defaults(target, order):
    """Each joint's A-pose local rotation, as a matrix.

    What a joint nothing was authored onto falls back to. Not identity: the
    zero-length shoulders and hips carry geometry of their own, and identity is
    not a pose they are ever seen in.
    """
    pose = a_pose(target)
    return {i: quat_to_matrix(pose[i][1]) if i in pose
            else quat_to_matrix(target.rest_local(i)[1]) for i in order}


def animated_joints(target, order):
    """The joints every clip has to write a track for.

    A joint left out of a clip falls back to its rest transform at playback,
    and Mario's rest transforms are the degenerate ones -- the pose would come
    apart exactly where it was not written. The reference clip animates the
    full set, so it is what defines it.
    """
    pose = a_pose(target)
    return [i for i in order if i in pose]


def locals_from_world(target, order, wanted, fallback):
    """Turn wanted world rotations into local ones, down the hierarchy.

    `wanted` is {node: world rotation matrix}; every other joint takes its
    rotation from `fallback` and inherits whatever its parent ended up as.
    """
    world, local = {}, {}
    for node in order:
        parent = target.parent.get(node)
        parent_world = world.get(parent, np.eye(3))
        if node in wanted:
            world[node] = wanted[node]
            local[node] = matrix_to_quat(parent_world.T @ wanted[node])
        else:
            world[node] = parent_world @ fallback[node]
            local[node] = matrix_to_quat(fallback[node])
    return local, world


# -- grounding ----------------------------------------------------------------


class Skin:
    """Enough of the skinned mesh to find the lowest vertex of a pose.

    Mario's clips are authored around a logical position at his feet, so a made
    clip has to be shifted vertically until it agrees -- otherwise he wades
    through the floor or hovers over it.

    Worth knowing before picking a rule for that shift: the decomp's own clips
    are not tidy about it.  MARIO_ANIM_WALKING swings between seven units
    through the floor and six above it over one cycle, because it was authored
    to look right rather than to measure right.  So a made clip is matched to
    that average rather than pinned so nothing ever penetrates -- pinning the
    worst frame is what leaves Mario visibly hovering in all the others.
    """

    def __init__(self, target):
        skin = target.json["skins"][0]
        self.joints = skin["joints"]
        inverse = target.read(skin["inverseBindMatrices"]).reshape(-1, 4, 4)
        # glTF matrices are column-major.
        self.inverse_bind = inverse.transpose(0, 2, 1)

        positions, joints, weights = [], [], []
        for mesh in target.json["meshes"]:
            for primitive in mesh["primitives"]:
                attributes = primitive["attributes"]
                positions.append(target.read(attributes["POSITION"]))
                joints.append(target.read(attributes["JOINTS_0"]).astype(int))
                weights.append(target.read(attributes["WEIGHTS_0"]))
        self.positions = np.concatenate(positions)
        self.joint_indices = np.concatenate(joints)
        self.weights = np.concatenate(weights)

    def lowest(self, world):
        """Height of the lowest skinned vertex, given world joint matrices."""
        matrices = np.array([
            world[joint] @ self.inverse_bind[i]
            for i, joint in enumerate(self.joints)
        ])
        homogeneous = np.concatenate(
            [self.positions, np.ones((len(self.positions), 1))], axis=1)
        height = np.zeros(len(self.positions))
        for slot in range(self.joint_indices.shape[1]):
            weight = self.weights[:, slot]
            if not weight.any():
                continue
            rows = matrices[self.joint_indices[:, slot]][:, 1, :]
            height += weight * np.einsum("ij,ij->i", rows, homogeneous)
        return height.min()


def clearance(target, skin, clip=GROUND_CLIP):
    """Average height of a clip's lowest vertex, over its whole cycle."""
    tracks = target.tracks(clip)
    frames = int(round(clip_length(tracks) * FRAME_RATE)) + 1
    return float(np.mean([
        skin.lowest(target.world(pose_at(target, tracks, frame / FRAME_RATE)))
        for frame in range(frames)
    ]))


def posed(target, rotations, fallback, order, root_positions, frame):
    """The pose the written clip will produce on a given frame."""
    pose = {}
    for node in order:
        quat = (rotations[node][frame] if node in rotations
                else matrix_to_quat(fallback[node]))
        translation = (root_positions[frame] if node == target.index[ROOT]
                       else target.rest_local(node)[0])
        pose[node] = (translation, quat)
    return pose


def ground(target, skin, rotations, root_positions, fallback, order, reference):
    """Drop a clip's root motion until it sits on the floor like Mario's own."""
    lowest = np.array([
        skin.lowest(target.world(
            posed(target, rotations, fallback, order, root_positions, frame)))
        for frame in range(len(root_positions))
    ])
    shift = lowest.mean() - reference
    root_positions = np.array(root_positions, dtype=np.float64)
    root_positions[:, 1] -= shift
    return root_positions, shift


def footfalls(target, rotations, fallback, order, root_positions):
    """Frames where each foot lands, for the step sounds.

    Read as the frame the ankle reaches furthest forward, which is the heel
    strike: from there the planted foot travels backwards under the body.
    Height is the more obvious signal and the wrong one -- a shuffle keeps both
    feet low, so the lowest frame lands somewhere arbitrary in a long flat
    stretch of the curve.
    """
    world_by_frame = [
        target.world(
            posed(target, rotations, fallback, order, root_positions, frame))
        for frame in range(len(root_positions))
    ]

    out = []
    for foot in ("foot.L", "foot.R"):
        forward = np.array([w[target.index[foot]][2, 3] for w in world_by_frame])
        out.append(int(forward.argmax()))
    return sorted(out)


# -- output -------------------------------------------------------------------


def write_clip(target, name, rotations, root_positions):
    """Append a clip built from per-joint local quaternions and root motion."""
    frames = len(root_positions)
    times = target.add_array(
        np.arange(frames, dtype=np.float64) / FRAME_RATE, "SCALAR",
        with_bounds=True)

    samplers = [{
        "input": times,
        "output": target.add_array(root_positions, "VEC3"),
        "interpolation": "LINEAR",
    }]
    channels = [{"sampler": 0, "target": {"node": target.index[ROOT],
                                          "path": "translation"}}]

    for node, values in sorted(rotations.items()):
        samplers.append({
            "input": times,
            "output": target.add_array(values, "VEC4"),
            "interpolation": "LINEAR",
        })
        channels.append({
            "sampler": len(samplers) - 1,
            "target": {"node": node, "path": "rotation"},
        })

    target.replace_animation(
        {"name": name, "samplers": samplers, "channels": channels})
    return frames


def record_clip(clips_path, name, frames):
    """Note a clip in the sidecar the game reads its playback metadata from."""
    with open(clips_path, "r", encoding="utf-8") as fh:
        clips = json.load(fh)
    clips[name] = {"frames": frames, "start_frame": 0,
                   "loop_start": 0, "loop_end": frames}
    with open(clips_path, "w", encoding="utf-8") as fh:
        json.dump(clips, fh, indent=2, sort_keys=True)
