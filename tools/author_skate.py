"""Author an ice-skating cycle directly onto Mario's skeleton.

Nothing in reference/mesh2motion-app skates -- the nearest it has is a slide
and a sprint -- so unlike the zombie shamble there is no clip to retarget.
This makes one.

The stride is written as blade positions rather than joint angles.  A skating
push is defined by where the blade is: it goes down under the hip, travels out
to the side and back as the body glides over it, lifts, and swings in and
forward to land again.  Saying that in joint angles means picking a thigh and a
shin rotation that happen to put the foot there, on a rig whose proportions are
nothing like a human's.  Saying it as a position and solving two-bone IK for the
knee means the blade lands where it was asked to, and the leg bends however
Mario's leg has to bend to reach.

Everything is measured off his A-pose rather than typed in, so the numbers below
are all in the one frame that matters -- body-local units, origin on the ice
between his feet, +X his left, +Y up, +Z the way he faces.

Two clips come out:

    skate_stride   the pushing cycle, played faster the quicker he goes
    skate_glide    both blades down, coasting, for when he is not pushing

Usage:
    python3 tools/author_skate.py
"""

import argparse
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))
sys.path.insert(0, HERE)

import rig  # noqa: E402

DEFAULT_TARGET = os.path.join(ROOT, "assets", "mario", "mario.glb")

STRIDE_FRAMES = 45
GLIDE_FRAMES = 30

# -- the skater ---------------------------------------------------------------
#
# A skater is not a walker with different timing. The differences that matter:
# the hips ride low and never come up, the push goes sideways rather than
# backwards, the blade stays on the ice for most of the cycle, and the whole
# body leans forward far enough that the shoulders are ahead of the hips.

# How low he rides, as a fraction of his standing hip height. This is the one
# number that most decides whether it reads as skating or as walking oddly:
# above about 0.9 he looks like he is strolling.
CROUCH = 0.86
# Rise and fall through the push, and the sway onto whichever blade is loaded.
BOB = 2.5
SWAY = 4.5

# Forward lean, in degrees from vertical, at the hips, chest and neck. The head
# is deliberately the most upright of the three: he is looking where he is
# going, not at the ice.
PELVIS_LEAN = 10.0
TORSO_LEAN = 22.0
HEAD_LEAN = 0.0
# Shoulders counter-rotating against the hips, in degrees, as a roll about the
# spine -- the spine is near enough vertical that a yaw would not show.
TORSO_TWIST = 9.0

# The blade's path over one push, in body-local units. Sideways from the hip,
# and fore-and-aft about the body's centre.
PUSH_OUT_START = 2.0     # tucked in under the hip as it lands
PUSH_OUT_END = 22.0      # driven out to the side at full extension
PUSH_FORWARD = 13.0      # how far ahead of centre it lands
PUSH_BACK = -13.0        # and how far behind it finishes
# Fraction of the cycle the blade spends on the ice. Long, because that is what
# gliding is -- a walk cycle is nearer half.
PUSH_PHASE = 0.62
# How high the blade comes up on the recovery swing.
RECOVER_LIFT = 9.0

# Toe-out, in degrees. The blade turns further out through the push, which is
# what makes the V a skater pushes against.
TOE_OUT_START = 14.0
TOE_OUT_END = 34.0

# The arms are aimed rather than solved for, in degrees: out from vertical,
# how far they swing fore and aft, and how much the elbow folds forward.
#
# Mario's arms are short -- 31 units against 39 for his legs -- so a hand
# position that looks right for a skater is usually somewhere he cannot reach,
# and the solver quietly clamps to a pose nobody chose. Aiming the two bones
# has no unreachable case.
ARM_OUT = 34.0
ARM_SWING = 26.0
ELBOW_BEND = 38.0

# Which way the knee is allowed to fold: forward, and slightly out.
KNEE_POLE = np.array([0.25, 0.0, 1.0])

# Reference axis per joint, fixed rather than chosen per frame: the constant
# each joint is measured with has to be the one it is later posed with, and a
# rule that switches on the current direction would switch mid-clip.
AXES = {
    "butt": rig.FORWARD, "torso": rig.FORWARD, "head": rig.FORWARD,
    "upper_arm.L": rig.FORWARD, "forearm.L": rig.FORWARD,
    "upper_arm.R": rig.FORWARD, "forearm.R": rig.FORWARD,
    "thigh.L": rig.FORWARD, "shin.L": rig.FORWARD, "foot.L": rig.UP,
    "thigh.R": rig.FORWARD, "shin.R": rig.FORWARD, "foot.R": rig.UP,
}


def smoothstep(u):
    u = np.clip(u, 0.0, 1.0)
    return u * u * (3.0 - 2.0 * u)


def solve_knee(root, target, upper, lower, pole):
    """Where the middle joint of a two-bone chain goes.

    The elbow or knee sits on a circle around the line from root to target;
    `pole` picks the point on it, which is what stops a leg bending backwards.
    """
    to_target = target - root
    distance = np.linalg.norm(to_target)
    # Never fully straight: a chain at exactly full stretch has no defined
    # bend plane, and one past it has no solution at all.
    distance = float(np.clip(distance, abs(upper - lower) + 1e-3,
                             (upper + lower) * 0.999))
    axis = to_target / np.linalg.norm(to_target)

    along = (upper * upper - lower * lower + distance * distance) / (2 * distance)
    height = np.sqrt(max(upper * upper - along * along, 0.0))

    side = pole - axis * np.dot(pole, axis)
    norm = np.linalg.norm(side)
    if norm < 1e-6:
        side = np.cross(axis, rig.UP)
        norm = np.linalg.norm(side)
    return root + axis * along + (side / norm) * height


class Body:
    """Mario's measurements, taken off his A-pose."""

    def __init__(self, target):
        world = target.world(rig.a_pose(target))

        def at(name):
            return world[target.index[name]][:3, 3]

        self.root_height = float(at("root")[1])
        self.hip = at("thigh.L") - at("root")
        self.shoulder = at("shoulder.L") - at("root")
        self.thigh = float(np.linalg.norm(at("shin.L") - at("thigh.L")))
        self.shin = float(np.linalg.norm(at("foot.L") - at("shin.L")))
        self.upper_arm = float(np.linalg.norm(at("forearm.L") - at("upper_arm.L")))
        self.forearm = float(np.linalg.norm(at("hand.L") - at("forearm.L")))
        # How high the ankle sits when the foot is flat on the floor, which is
        # where a blade on the ice puts it.
        self.blade = float(at("foot.L")[1])

    def mirror(self, vector, side):
        """A left-side vector, for whichever side is asked for."""
        return np.array([vector[0] * side, vector[1], vector[2]])


def blade_at(body, phase, side):
    """Where one blade is, and which way it points, at a phase of the cycle."""
    hip_x = body.hip[0]
    if phase < PUSH_PHASE:
        u = phase / PUSH_PHASE
        out = PUSH_OUT_START + (PUSH_OUT_END - PUSH_OUT_START) * u
        forward = PUSH_FORWARD + (PUSH_BACK - PUSH_FORWARD) * u
        height = body.blade
        toe = TOE_OUT_START + (TOE_OUT_END - TOE_OUT_START) * u
    else:
        u = smoothstep((phase - PUSH_PHASE) / (1.0 - PUSH_PHASE))
        out = PUSH_OUT_END + (PUSH_OUT_START - PUSH_OUT_END) * u
        forward = PUSH_BACK + (PUSH_FORWARD - PUSH_BACK) * u
        height = body.blade + RECOVER_LIFT * np.sin(np.pi * u)
        toe = TOE_OUT_END + (TOE_OUT_START - TOE_OUT_END) * u

    position = np.array([(hip_x + out) * side, height, forward])
    angle = np.radians(toe)
    direction = np.array([np.sin(angle) * side, 0.0, np.cos(angle)])
    return position, direction / np.linalg.norm(direction)


def rotate_toward(direction, goal, degrees):
    """Swing a direction `degrees` of the way round toward another."""
    axis = np.cross(direction, goal)
    norm = np.linalg.norm(axis)
    if norm < 1e-6:
        return direction
    axis = axis / norm
    angle = np.radians(degrees)
    # Rodrigues, which is shorter than building the matrix.
    return (direction * np.cos(angle)
            + np.cross(axis, direction) * np.sin(angle)
            + axis * np.dot(axis, direction) * (1.0 - np.cos(angle)))


def arm_at(phase, side):
    """Upper arm and forearm directions. Arms swing against the same-side leg."""
    out = np.radians(ARM_OUT)
    swing = np.radians(ARM_SWING * np.cos(2.0 * np.pi * phase))
    upper = np.array([np.sin(out) * side,
                      -np.cos(out) * np.cos(swing),
                      np.cos(out) * np.sin(swing)])
    return upper, rotate_toward(upper, rig.FORWARD, ELBOW_BEND)


def spine(phase):
    """Pelvis, chest and head directions, plus the shoulders' counter-twist."""
    def lean(degrees):
        angle = np.radians(degrees)
        return np.array([0.0, np.cos(angle), np.sin(angle)])

    twist = TORSO_TWIST * np.sin(2.0 * np.pi * phase)
    return lean(PELVIS_LEAN), lean(TORSO_LEAN), lean(HEAD_LEAN), twist


def build(target, frames, gliding=False):
    """Pose the whole skeleton for every frame of a cycle."""
    body = Body(target)
    constants = rig.joint_constants(target, AXES)
    order = target.order()
    fallback = rig.defaults(target, order)
    animated = set(rig.animated_joints(target, order))

    rotations = {node: [] for node in animated}
    root_positions = []

    for frame in range(frames):
        phase = frame / frames
        # Coasting is the same body with both blades planted and the stride
        # frozen just after the plant, so the two clips read as one skater.
        left_phase = 0.06 if gliding else phase
        right_phase = 0.06 if gliding else (phase + 0.5) % 1.0

        pelvis, chest, head, twist = spine(0.0 if gliding else phase)
        if gliding:
            sway = 0.0
            bob = BOB * 0.35 * np.cos(2.0 * np.pi * phase)
        else:
            sway = SWAY * np.cos(2.0 * np.pi * phase)
            bob = -BOB * np.cos(4.0 * np.pi * phase)

        root = np.array([sway, body.root_height * CROUCH + bob, 0.0])

        wanted = {
            target.index["butt"]:
                rig.anatomical_frame(pelvis, AXES["butt"]) @ constants["butt"],
            target.index["torso"]:
                rig.anatomical_frame(chest, AXES["torso"])
                @ rig.roll_matrix(twist) @ constants["torso"],
            target.index["head"]:
                rig.anatomical_frame(head, AXES["head"]) @ constants["head"],
        }

        # The spine decides where the hips and shoulders actually are, so it
        # has to be posed before anything can be solved against them.
        anchors = anchor_world(target, order, wanted, fallback, root)

        for side, suffix in ((1.0, "L"), (-1.0, "R")):
            hip = anchors[target.index[f"thigh.{suffix}"]][:3, 3]
            ankle, blade = blade_at(body, left_phase if side > 0 else right_phase,
                                    side)
            ankle = ankle + root * np.array([1.0, 0.0, 1.0])
            knee = solve_knee(hip, ankle, body.thigh, body.shin,
                              body.mirror(KNEE_POLE, side))
            _aim(wanted, target, constants, f"thigh.{suffix}", knee - hip)
            _aim(wanted, target, constants, f"shin.{suffix}", ankle - knee)
            _aim(wanted, target, constants, f"foot.{suffix}", blade)

            upper, lower = arm_at(left_phase if side > 0 else right_phase, side)
            _aim(wanted, target, constants, f"upper_arm.{suffix}", upper)
            _aim(wanted, target, constants, f"forearm.{suffix}", lower)

        local, _ = rig.locals_from_world(target, order, wanted, fallback)
        for node in animated:
            rotations[node].append(local[node])
        root_positions.append(root)

    return ({node: np.array(values) for node, values in rotations.items()},
            np.array(root_positions), fallback, order)


def _aim(wanted, target, constants, joint, direction):
    """Point a joint's bone along a world direction."""
    wanted[target.index[joint]] = (
        rig.anatomical_frame(direction, AXES[joint]) @ constants[joint])


def anchor_world(target, order, wanted, fallback, root):
    """World matrices for a part-built pose, to solve the limbs against."""
    local, _ = rig.locals_from_world(target, order, wanted, fallback)
    pose = {
        node: (root if node == target.index[rig.ROOT]
               else target.rest_local(node)[0], local[node])
        for node in order
    }
    return target.world(pose)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--target", default=DEFAULT_TARGET)
    args = parser.parse_args(argv[1:])

    target = rig.Gltf(args.target)
    skin = rig.Skin(target)
    reference = rig.clearance(target, skin)
    clips_path = os.path.splitext(args.target)[0] + "_clips.json"

    for name, frames, gliding in (("anim_skate_stride", STRIDE_FRAMES, False),
                                  ("anim_skate_glide", GLIDE_FRAMES, True)):
        rotations, root, fallback, order = build(target, frames, gliding)
        root, shift = rig.ground(target, skin, rotations, root, fallback,
                                 order, reference)
        written = rig.write_clip(target, name, rotations, root)
        rig.record_clip(clips_path, name, written)
        steps = rig.footfalls(target, rotations, fallback, order, root)
        print(f"{name}: {written} frames, dropped {shift:.1f} units onto the "
              f"ice, blades plant on {steps}")

    target.write(args.target)
    size = os.path.getsize(args.target)
    print(f"wrote {args.target} ({size / 1024:.0f} KB) and {clips_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
