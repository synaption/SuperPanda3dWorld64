"""Give the Hero's exported skeleton the runtime pivots docs/aim.md asks for.

docs/aim.md wants an `AIM_TORSO` bone carrying no authored motion, so the game
can turn the upper body toward the aim while the clips underneath keep playing,
and a `WEAPON_SOCKET` under the right hand for a weapon to hang off. Neither
exists in the export, and neither can simply be added in Blender, because of
what the export actually looks like:

**The exported skeleton is flat.** Rigify's DEF bones in TheHero.blend are not
parented to each other -- they are driven by constraints off the control rig --
so `export_def_bones` has nothing to hang them from and every one of them comes
out as a child of `rig`. The arms are not under the shoulders, the shoulders
are not under the spine, and the pelvis is a sibling of the thighs. Rotating a
spine bone therefore moves the spine mesh and nothing else.

That rules out the anatomical repair the doc's skeleton diagram implies. The
clips are authored as one local transform per bone per frame, and re-expressing
them under a different parent means decomposing a world matrix back into
translation/rotation/scale -- which the Rigify stretch bones make lossy, since
their non-uniform scale leaves shear in the world matrix that a glTF TRS cannot
hold. Measured over every clip and frame, rebuilding an anatomical hierarchy
moves the Hero's fingertips by up to 315 mm. It is not a rounding error, it is
a different animation.

What *is* exact is inserting a pivot the clips never touch. A new node with no
animation and no rotation, sitting between `rig` and a set of joints that are
already children of `rig`, changes each of those joints by a constant
translation and nothing else -- so every keyframe survives as itself, shifted.
`verify` re-derives every joint's world matrix on every frame of every clip and
holds the whole file to 0.001 mm.

The flat skeleton is what makes that enough. Because the thighs hang off `rig`
rather than off the pelvis, the pelvis can join the upper body without dragging
the legs along, and one pivot carrying

    DEF-spine (and the whole spine chain, head included)
    the shoulders, arms, hands and fingers
    the cape, the sheath and the breast bones

leaves behind exactly the lower body: the thighs, the pelvis bones, the belt
and the sash. Turning AIM_TORSO turns everything above the hips as one piece.

The cost of doing it this way is that the twist is rigid: the pelvis mesh turns
with the chest instead of the spine curving through it, since the spine chain
rides along inside the group rather than bending within it. Distributing the
turn up the spine -- docs/aim.md's "AIM_TORSO 30, upper spine 12, shoulders 8"
-- needs the DEF bones parented properly in the .blend first. This is the doc's
"simple first implementation can use only AIM_TORSO", and the reason it is the
only one available.

Run last, after tools/lock_root_motion.py: locking reads the joints that have
no parent among the joints, and afterwards the pelvis has one. It will refuse
rather than mislock, which is the point of the ordering.

    python3 tools/aim_rig.py assets/hero/hero.glb

tools/build_hero.py runs this as its final stage.
"""

import argparse
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import rig  # noqa: E402

PIVOT = "AIM_TORSO"

# Where the pivot sits. The pelvis bone turns with the group, so the group's
# natural centre of rotation is the pelvis's own origin -- pivoting anywhere
# else would swing the hips sideways across the legs as he twists. The point
# only sets the centre; which mesh bends is a question of weights.
PIVOT_ANCHOR = "DEF-spine"

# Everything above the hips, named as the joints that are children of the
# skeleton root. Their own chains follow them without being mentioned: naming
# DEF-spine takes the whole spine and the head, DEF-upper_arm.L takes the
# forearm and the hand.
CARRIED = (
    "DEF-spine",
    "DEF-shoulder.L", "DEF-upper_arm.L", "fingers.l", "thumb.l",
    "DEF-shoulder.R", "DEF-upper_arm.R", "fingers.r", "thumb.r",
    "DEF-breast.L", "DEF-breast.R",
    "Cape", "Sheath bone",
)

# What is deliberately left behind, listed so that a future export growing a
# new root-level joint fails the check below instead of quietly ending up in
# whichever half it was not meant to be in.
LOWER_BODY = (
    "DEF-thigh.L", "DEF-thigh.R", "DEF-pelvis.L", "DEF-pelvis.R",
    "Belt", "Sash.00",
)

SOCKET = "WEAPON_SOCKET"
SOCKET_PARENT = "DEF-hand.R"

# How far the world may move before the restructure is called a failure, in mm
# on a character 1.77 units tall. The arithmetic is exact in float64 and the
# file stores float32, so the floor is rounding rather than method.
TOLERANCE_MM = 0.001


def skeleton_root(gltf, joints):
    """The one node every carried joint hangs off."""
    parents = {gltf.parent.get(gltf.index[name]) for name in joints}
    if len(parents) != 1 or None in parents:
        raise ValueError(
            "the carried joints do not share a single parent: "
            + ", ".join(sorted(
                "%s under %s" % (name, gltf.nodes[gltf.parent[gltf.index[name]]]
                                 .get("name", "?"))
                for name in joints)))
    return parents.pop()


def check_membership(gltf, root):
    """Every joint hanging off the root is in one list or the other."""
    named = set(CARRIED) | set(LOWER_BODY)
    joints = set(gltf.json["skins"][0]["joints"])
    loose = [gltf.nodes[c].get("name") for c in gltf.nodes[root].get("children", [])
             if c in joints and gltf.nodes[c].get("name") not in named]
    if loose:
        raise ValueError(
            "these joints hang off %r and are in neither CARRIED nor "
            "LOWER_BODY, so which half of the body they belong to is not "
            "recorded anywhere: %s" % (gltf.nodes[root].get("name"),
                                       ", ".join(sorted(loose))))


def translation_samplers(gltf, nodes):
    """The translation sampler of each named node, per clip.

    Raises if a sampler is shared with a node outside the set: shifting it
    would move that node too, and the exporter is not guaranteed not to share.
    """
    out = []
    for anim in gltf.json.get("animations", []):
        users = {}
        for channel in anim["channels"]:
            if channel["target"]["path"] == "translation":
                users.setdefault(channel["sampler"], set()).add(
                    channel["target"]["node"])
        for sampler, targets in users.items():
            if targets & nodes:
                if targets - nodes:
                    raise ValueError(
                        "clip %r shares one translation sampler between the "
                        "torso and the legs" % anim.get("name"))
                out.append(anim["samplers"][sampler])
    return out


def matrix_column_major(matrix):
    """A 4x4 as glTF stores it."""
    return np.asarray(matrix, dtype=np.float64).flatten(order="F")


def restructure(gltf):
    """Insert the pivot and the weapon socket. Returns what was done."""
    for name in (PIVOT, SOCKET):
        if name in gltf.index:
            raise ValueError(
                "%s already has a %s; this file has been through aim_rig "
                "already" % (os.path.basename(getattr(gltf, "path", "the file")),
                             name))

    root = skeleton_root(gltf, CARRIED)
    check_membership(gltf, root)

    world = gltf.world()
    carried = {gltf.index[name] for name in CARRIED}
    pivot_at = world[gltf.index[PIVOT_ANCHOR]][:3, 3]

    # -- the pivot node, unrotated and unanimated ---------------------------
    gltf.nodes.append({
        "name": PIVOT,
        "translation": [float(v) for v in pivot_at],
        "children": sorted(carried),
    })
    pivot = len(gltf.nodes) - 1

    children = gltf.nodes[root].setdefault("children", [])
    gltf.nodes[root]["children"] = [c for c in children if c not in carried]
    gltf.nodes[root]["children"].append(pivot)

    # -- the joints it took with it -----------------------------------------
    # Their parent moved from the origin to the pivot, so every translation
    # they hold -- the rest one and every keyframe of every clip -- loses the
    # pivot. Nothing else about them changes.
    for node in carried:
        rest = np.array(gltf.nodes[node].get("translation", [0.0, 0.0, 0.0]))
        gltf.nodes[node]["translation"] = [float(v) for v in rest - pivot_at]

    for sampler in translation_samplers(gltf, carried):
        values = gltf.read(sampler["output"]).copy()
        values -= pivot_at
        sampler["output"] = gltf.add_array(values, "VEC3", with_bounds=True)

    # -- the weapon socket ---------------------------------------------------
    # Identity, so it is the hand: a weapon's own grip offset belongs to the
    # weapon, which is the thing that knows where its handle is.
    hand = gltf.index[SOCKET_PARENT]
    gltf.nodes.append({"name": SOCKET})
    socket = len(gltf.nodes) - 1
    gltf.nodes[hand].setdefault("children", []).append(socket)

    # -- and both of them as joints -----------------------------------------
    # A skinned character is built from the skin's joint list, and a node left
    # out of it arrives as scenery rather than as a joint the animation and
    # aiming code can drive. No vertex is weighted to either, so appending is
    # free: weights index this list by position and nothing indexes the end.
    skin = gltf.json["skins"][0]
    bind = gltf.read(skin["inverseBindMatrices"])
    pivot_bind = np.eye(4)
    pivot_bind[:3, 3] = -pivot_at
    hand_bind = bind[skin["joints"].index(hand)]        # the socket is the hand
    skin["joints"].extend([pivot, socket])
    skin["inverseBindMatrices"] = gltf.add_array(
        np.vstack([bind, matrix_column_major(pivot_bind), hand_bind]), "MAT4")

    gltf.parent[pivot] = root
    gltf.parent[socket] = hand
    for node in carried:
        gltf.parent[node] = pivot
    gltf.index[PIVOT] = pivot
    gltf.index[SOCKET] = socket

    return {"pivot": PIVOT, "at": [round(float(v), 4) for v in pivot_at],
            "carried": len(carried), "socket": SOCKET}


# -- checking -----------------------------------------------------------------


def posed_world(gltf, tracks, time):
    """Every node's world matrix at one instant of one clip."""
    local = {}
    for node in range(len(gltf.nodes)):
        translation, rotation = gltf.rest_local(node)
        scale = np.ones(3)
        channels = tracks.get(node)
        if channels:
            if "translation" in channels:
                translation = rig.sample_track(*channels["translation"], time,
                                               rotation=False)
            if "rotation" in channels:
                rotation = rig.sample_track(*channels["rotation"], time,
                                            rotation=True)
            if "scale" in channels:
                scale = rig.sample_track(*channels["scale"], time,
                                         rotation=False)
        matrix = np.eye(4)
        matrix[:3, :3] = rig.quat_to_matrix(rotation) * scale
        matrix[:3, 3] = translation
        local[node] = matrix

    world = {}

    def resolve(node):
        if node not in world:
            parent = gltf.parent.get(node)
            world[node] = (resolve(parent) @ local[node] if parent is not None
                           else local[node])
        return world[node]

    return {node: resolve(node) for node in local}


def verify(before, after):
    """The largest distance any joint moved, in mm, over every clip and frame.

    Not a spot check of the rest pose: the whole point of the restructure is
    that it survives the animation, and a rest pose can be right while every
    clip is wrong. Each joint is sampled as a frame rather than as a point, so
    a joint that is in the right place but facing the wrong way still counts.
    """
    probes = np.eye(3) * 0.05
    worst, where = 0.0, None
    names = {i: n.get("name") for i, n in enumerate(before.nodes)}

    for anim in before.json.get("animations", []):
        name = anim["name"]
        old_tracks = before.tracks(name)
        new_tracks = after.tracks(name)
        for time in before.read(anim["samplers"][0]["input"])[:, 0]:
            old = posed_world(before, old_tracks, time)
            new = posed_world(after, new_tracks, time)
            for joint in before.json["skins"][0]["joints"]:
                a, b = old[joint], new[joint]
                for probe in probes:
                    moved = np.linalg.norm(
                        (a[:3, :3] @ probe + a[:3, 3])
                        - (b[:3, :3] @ probe + b[:3, 3])) * 1000.0
                    if moved > worst:
                        worst, where = moved, (name, names[joint], float(time))
    return worst, where


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("model", nargs="?", default=os.path.join(
        os.path.dirname(HERE), "assets", "hero", "hero.glb"))
    parser.add_argument("--out", default=None, help="defaults to in place")
    parser.add_argument("--no-verify", action="store_true",
                        help="skip re-deriving every frame of every clip")
    args = parser.parse_args(argv[1:])

    out = args.out or args.model
    # Read twice up front: the default is to rewrite in place, and the check
    # needs the file as it was.
    before = None if args.no_verify else rig.Gltf(args.model)
    gltf = rig.Gltf(args.model)
    gltf.path = args.model
    report = restructure(gltf)
    gltf.write(out)

    print("%s at %s, carrying %d joints; %s under %s"
          % (report["pivot"], report["at"], report["carried"],
             report["socket"], SOCKET_PARENT))

    if not args.no_verify:
        worst, where = verify(before, rig.Gltf(out))
        clip, joint, time = where
        print("worst joint movement %.4f mm (%s, %s, t=%.2f)"
              % (worst, joint, clip, time))
        if worst > TOLERANCE_MM:
            sys.stderr.write(
                "the restructure changed the animation; %s moves %.3f mm in "
                "%r\n" % (joint, worst, clip))
            return 1

    print("wrote %s (%.0f KB)" % (out, os.path.getsize(out) / 1024))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
