"""Turning an actor's billboarded parts to face the camera.

SM64 draws some actor parts -- a goomba's face, most of a scuttlebug's body --
as flat quads that the original rebuilds every frame to point at the camera.
glTF has no billboard concept, so they arrive as ordinary geometry, and
Panda3D's own billboard effect cannot help: it acts on a node's transform, and
this geometry is skinned to character joints. The exporter therefore makes each
such quad a joint of its own, and they get driven from here.

The part that took several wrong turns to find is the frame the driving happens
in. `Actor.control_joint` hands back a NodePath parented to the *model root*,
not into the joint hierarchy -- so its transform reads as the joint's local
value while its scene-graph parent is somewhere else entirely. Two consequences,
both of which produced plausible-looking code that did nothing:

  * `set_hpr(some_other_node, ...)` is not a way to escape the joint's parents.
    Panda3D solves that against the *scene graph* parent, which is the model
    root, so it comes out identical to the plain local call. It never sees the
    joint chain at all.

  * The joint chain's rotation is still applied, inside the Character, on top of
    whatever is set here. On the goomba that hidden rotation is about a quarter
    turn of roll, which turns a local heading into net *pitch* -- so heading
    tipped the quad up and down instead of turning it about vertical, and no
    value of it could ever have worked.

So the rotation wanted in world space is composed against the inverse of the
parent joint's measured net rotation. `net = local * parent`, hence
`local = world * parent^-1`. Measured on an isolated goomba face, that takes the
width it holds around an orbit from 0.13 of its widest to 0.84 -- the remainder
being perspective, since the quad does not sit on the axis it orbits.

Everything the aiming depends on lives in `Tuning` so it can be adjusted from
the asset workbench and written back out, rather than being guessed at in
source. See tools/workbench.py.
"""

import json
import math
import os

# name, default, step for interactive adjustment, what it does
FIELDS = (
    ("enabled", True, None,
     "drive these joints at all"),
    ("cancel_parent", True, None,
     "compose against the parent joint's inverse rotation; without this the "
     "quad turns about whatever axis the joint chain leaves it with"),
    ("heading_offset", 0.0, 5.0,
     "degrees added to the heading that points at the camera, for quads whose "
     "authored facing is not straight down -Y"),
    # Neither of these changes the silhouette of a flat quad facing the
    # camera -- pitch tips it away, roll spins it about its own normal -- so
    # both stay at zero. They are here because getting that wrong is what the
    # earlier attempts were, and because a quad that is not flat would need
    # them.
    ("pitch", 0.0, 5.0, "world pitch held while facing the camera"),
    ("roll", 0.0, 5.0, "world roll held while facing the camera"),
    ("scale", 4.0, 0.25,
     "billboarded geometry escapes the GEO_SCALE(0x00, 16384) wrapping these "
     "actors, because the original rebuilds the matrix at a billboard rather "
     "than accumulating into it; 4.0 is exactly 1/0.25 and puts it back"),
)

DEFAULTS = {name: value for name, value, _, _ in FIELDS}
STEPS = {name: step for name, _, step, _ in FIELDS}
HELP = {name: text for name, _, _, text in FIELDS}

DEFAULT_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "assets", "billboard_tuning.json")


class Tuning:
    """Billboard settings, globally and per actor.

    Per-actor entries hold only the fields that differ, so a change to a
    default is picked up by every actor that has not been overridden.
    """

    def __init__(self, base=None, per_actor=None):
        self.base = dict(DEFAULTS)
        if base:
            self.base.update(base)
        self.per_actor = {k: dict(v) for k, v in (per_actor or {}).items()}

    # -- reading and writing ------------------------------------------------

    def get(self, field, actor=None):
        if actor and field in self.per_actor.get(actor, {}):
            return self.per_actor[actor][field]
        return self.base.get(field, DEFAULTS.get(field))

    def set(self, field, value, actor=None):
        if actor:
            self.per_actor.setdefault(actor, {})[field] = value
        else:
            self.base[field] = value

    def nudge(self, field, direction, actor=None):
        """Step a numeric field, or flip a boolean one."""
        current = self.get(field, actor)
        if isinstance(current, bool):
            self.set(field, not current, actor)
        else:
            step = STEPS.get(field) or 1.0
            self.set(field, round(current + step * direction, 4), actor)
        return self.get(field, actor)

    def clear(self, actor=None):
        if actor:
            self.per_actor.pop(actor, None)
        else:
            self.base = dict(DEFAULTS)

    def to_dict(self):
        return {"base": self.base, "per_actor": self.per_actor}

    def describe(self, actor=None):
        """One line per field, marking which are actor overrides."""
        lines = []
        for name, _, _, _ in FIELDS:
            override = actor and name in self.per_actor.get(actor, {})
            value = self.get(name, actor)
            shown = value if not isinstance(value, float) else f"{value:g}"
            lines.append(f"{name:<15} {str(shown):>8}"
                         f"{'  (override)' if override else ''}")
        return lines

    # -- files --------------------------------------------------------------

    @classmethod
    def load(cls, path=None):
        """Read tuning from disk, falling back to the defaults above."""
        path = path or DEFAULT_PATH
        try:
            with open(path) as handle:
                data = json.load(handle)
        except (OSError, ValueError):
            return cls()
        return cls(data.get("base"), data.get("per_actor"))

    def save(self, path=None):
        path = path or DEFAULT_PATH
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as handle:
            json.dump(self.to_dict(), handle, indent=2, sort_keys=True)
            handle.write("\n")
        return path


class Rig:
    """One billboard joint, and everything needed to aim it every frame."""

    def __init__(self, name, control, parent, exposed, rest_pos,
                 actor_name=None, owner=None):
        self.name = name
        self.control = control
        # The parent joint, exposed. Read live rather than cached because an
        # animated parent moves, and the correction has to move with it.
        self.parent = parent
        # This joint, exposed. Reports where the quad ended up after the
        # character was evaluated, which is what a heading has to be measured
        # from.
        self.exposed = exposed
        # In the parent's frame, which is where a joint's local translation
        # lives. Not the difference of the two net positions: that is the same
        # vector expressed in the actor's frame, and the two only agree while
        # the parent is unrotated -- which it is not.
        self.rest_pos = rest_pos
        self.actor_name = actor_name
        self.owner = owner

    def world_pos(self, reference):
        """Where this quad actually is, for working out a heading from.

        Not the control node's position: that reads as the joint's local
        offset, which is measured from a parent that is elsewhere.
        """
        if self.exposed is not None:
            return self.exposed.get_pos(reference)
        return self.control.get_pos(reference)

    def aim(self, reference, target, tuning):
        """Point the quad at `target`, both given in `reference`'s space."""
        from panda3d.core import Mat4, Quat, VBase3

        actor = self.actor_name
        if not tuning.get("enabled", actor):
            return

        here = self.world_pos(reference)
        heading = math.degrees(math.atan2(-(target[0] - here[0]),
                                          target[1] - here[1]))
        heading += tuning.get("heading_offset", actor)

        world = Quat()
        world.set_hpr(VBase3(heading, tuning.get("pitch", actor),
                             tuning.get("roll", actor)))

        if tuning.get("cancel_parent", actor) and self.parent is not None:
            # Only the rotation, taken from hpr rather than from the matrix, so
            # the parent's baked 0.25 scale stays out of it.
            inverse = Quat()
            inverse.set_hpr(self.parent.get_hpr(reference))
            inverse.invert_in_place()
            world = world * inverse

        mat = Mat4()
        world.extract_to_matrix(mat)
        scale = tuning.get("scale", actor)
        for row in range(3):
            mat.set_row(row, mat.get_row3(row) * scale)
        mat.set_row(3, self.rest_pos)
        self.control.set_mat(mat)


def claim(actor, prefix="billboard_", actor_name=None, owner=None):
    """Take over every joint the exporter marked as a billboard.

    Returns a Rig per joint. The joint's rest translation is worked out before
    control is taken, because `control_joint` initialises its node to identity
    and would otherwise silently drop any authored offset.
    """
    from panda3d.core import Point3

    # Exposed joints report identity until the character has been evaluated
    # once, and a rest offset measured against identity is silently wrong.
    actor.update(force=True)

    parents = joint_parents(actor)
    rigs = []
    for joint in actor.get_joints():
        name = joint.get_name()
        if not name.startswith(prefix):
            continue

        parent = expose(actor, parents.get(name))

        # Rest offset, in the parent's frame, read while the joint is still
        # animated normally.
        exposed = expose(actor, name)
        rest = Point3(0, 0, 0)
        if exposed is not None and parent is not None:
            rest = Point3(exposed.get_pos(parent))

        control = actor.control_joint(None, "modelRoot", name)
        if control is None:
            continue
        rigs.append(Rig(name, control, parent, exposed, rest, actor_name,
                        owner))
    return rigs


def expose(actor, name):
    """Expose a joint, or None if that name is not one.

    The top of the hierarchy is a PartGroup rather than a CharacterJoint and
    has no net transform to report, so a joint at the root has nothing to
    cancel against.
    """
    if not name:
        return None
    try:
        return actor.expose_joint(None, "modelRoot", name)
    except AttributeError:
        return None


def joint_parents(actor):
    """Joint name -> parent joint name, for the whole character."""
    parents = {}

    def walk(node, parent):
        parents[node.get_name()] = parent
        for i in range(node.get_num_children()):
            walk(node.get_child(i), node.get_name())

    walk(actor.get_part_bundle("modelRoot"), None)
    return parents


def probe(actor, prefix="billboard_"):
    """Everything measurable about an actor's billboard joints.

    Written for reading, by a person or by an agent with no screen: if the
    aiming is wrong again, this is the evidence to reason from rather than
    another round of guessing at constants.
    """
    from panda3d.core import GeomVertexReader, Vec3

    # Same trap as in claim(), and worse here: a probe that reports every
    # parent rotation as identity is exactly the kind of confident wrong
    # measurement this whole tool exists to prevent.
    actor.update(force=True)

    parents = joint_parents(actor)
    geometry = _geometry_by_joint(actor, GeomVertexReader, Vec3)

    rows = []
    for joint in actor.get_joints():
        name = joint.get_name()
        exposed = expose(actor, name)
        parent = expose(actor, parents.get(name))
        verts = geometry.get(name, [])
        extent = [0.0, 0.0, 0.0]
        normal = None
        if verts:
            lo = [min(v[i] for v, _ in verts) for i in range(3)]
            hi = [max(v[i] for v, _ in verts) for i in range(3)]
            extent = [round(hi[i] - lo[i], 2) for i in range(3)]
            acc = Vec3(0, 0, 0)
            for _, n in verts:
                acc += n
            if acc.length() > 1e-6:
                acc.normalize()
                normal = [round(acc[i], 3) for i in range(3)]

        # A quad is flat along exactly one axis, and that axis is its normal.
        # Worth reporting separately because the stored normals are not always
        # trustworthy -- several of these assets export a uniform placeholder.
        flat = [i for i, size in enumerate(extent) if size < 0.01]
        rows.append({
            "joint": name,
            "billboard": name.startswith(prefix),
            "parent": parents.get(name),
            "vertices": len(verts),
            "extent": extent,
            "flat_axis": "xyz"[flat[0]] if len(flat) == 1 and verts else None,
            "stored_normal": normal,
            "net_pos": _round(exposed.get_pos(actor) if exposed else None),
            "net_hpr": _round(exposed.get_hpr(actor) if exposed else None),
            "net_scale": _round(exposed.get_scale(actor) if exposed else None, 3),
            "parent_hpr": _round(parent.get_hpr(actor) if parent else None),
        })
    return rows


def _round(vector, places=2):
    return [round(v, places) for v in vector] if vector is not None else None


def _geometry_by_joint(actor, GeomVertexReader, Vec3):
    """Vertices and normals grouped by the joint they are weighted to.

    SM64 actors are rigidly segmented rather than smooth-skinned, so every
    vertex belongs to exactly one joint and this grouping is exact.
    """
    per_joint = {}
    for holder in actor.find_all_matches("**/+GeomNode"):
        node = holder.node()
        for index in range(node.get_num_geoms()):
            vdata = node.get_geom(index).get_vertex_data()
            table = vdata.get_transform_blend_table()
            if table is None:
                continue
            names = []
            for blend_index in range(table.get_num_blends()):
                blend = table.get_blend(blend_index)
                names.append(blend.get_transform(0).get_joint().get_name()
                             if blend.get_num_transforms() else None)

            vertices = GeomVertexReader(vdata, "vertex")
            normals = GeomVertexReader(vdata, "normal")
            blends = GeomVertexReader(vdata, "transform_blend")
            while not vertices.is_at_end():
                position = Vec3(vertices.get_data3())
                normal = Vec3(normals.get_data3())
                name = names[blends.get_data1i()]
                if name:
                    per_joint.setdefault(name, []).append((position, normal))
    return per_joint
