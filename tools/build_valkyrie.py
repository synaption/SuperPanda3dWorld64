"""Generate a low-poly mechanised valkyrie in Blender.

Run inside Blender (the MV4.blend on the Windows host).

Everything here is built from *lofted cross-sections*, not from glued-on box
primitives. Three profile shapes cover the whole model:

  `ring`   elliptical  -- bodies, limbs, head, hair, armour shells
  `blade`  lens        -- wing feathers, sword blade, anything flat
  `tube`   round       -- staff, nozzles, grips

Curved armour is laid over the body with `shell`, which follows the same
cross-sections as the part underneath, so plates wrap instead of sitting on
top as slabs. Face features (visor, eyes, mouth) are curved patches on the
skull for the same reason.

Value structure carries the silhouette: dark bodysuit underneath, light
armour over it. Without that separation the limbs merge into the torso.

The original amorphous MV4 blob is not deleted; it is moved to a hidden
collection so it stays available as reference.

Layout: Z up, character faces -Y (Blender front view), her right hand is +X.
"""

import math

import bmesh
import bpy
from mathutils import Euler, Matrix, Vector

# ---------------------------------------------------------------- materials

# (name, base colour, metallic, roughness, emission strength, alpha)
PALETTE = [
    ("VK_ArmorWhite", (0.800, 0.825, 0.870), 0.30, 0.50, 0.0, 1.0),
    ("VK_ArmorBlue", (0.075, 0.145, 0.440), 0.35, 0.42, 0.0, 1.0),
    ("VK_TrimGold", (0.870, 0.680, 0.230), 0.85, 0.28, 0.0, 1.0),
    ("VK_Steel", (0.300, 0.320, 0.360), 0.80, 0.38, 0.0, 1.0),
    ("VK_Suit", (0.085, 0.095, 0.135), 0.20, 0.62, 0.0, 1.0),
    ("VK_HairGreen", (0.180, 0.640, 0.330), 0.00, 0.85, 0.0, 1.0),
    ("VK_Skin", (0.960, 0.800, 0.700), 0.00, 0.90, 0.0, 1.0),
    ("VK_Visor", (0.150, 0.880, 0.950), 0.00, 0.15, 1.6, 0.30),
    ("VK_Thruster", (1.000, 0.400, 0.080), 0.00, 0.30, 3.0, 1.0),
    ("VK_EyeDark", (0.055, 0.075, 0.135), 0.00, 0.35, 0.0, 1.0),
    # kept low: any higher and the cyan clips to white in EEVEE
    ("VK_Energy", (0.220, 0.900, 1.000), 0.00, 0.20, 1.2, 1.0),
    ("VK_EyeLight", (0.980, 0.995, 1.000), 0.00, 0.30, 0.6, 1.0),
]

WHITE, BLUE, GOLD, STEEL, SUIT, HAIR, SKIN, VISOR, THRUST, EYE, ENERGY, GLINT = \
    range(12)


def build_materials():
    mats = []
    for name, colour, metallic, rough, emit, alpha in PALETTE:
        mat = bpy.data.materials.get(name)
        if mat is None:
            mat = bpy.data.materials.new(name)
        mat.use_nodes = True
        bsdf = mat.node_tree.nodes.get("Principled BSDF")
        bsdf.inputs["Base Color"].default_value = (*colour, 1.0)
        bsdf.inputs["Metallic"].default_value = metallic
        bsdf.inputs["Roughness"].default_value = rough
        if "Emission Color" in bsdf.inputs:
            bsdf.inputs["Emission Color"].default_value = (*colour, 1.0)
            bsdf.inputs["Emission Strength"].default_value = emit
        bsdf.inputs["Alpha"].default_value = alpha
        if alpha < 1.0:
            # name of the blended-surface setting moved around across versions
            for attr, value in (
                ("surface_render_method", "BLENDED"),
                ("blend_method", "BLEND"),
            ):
                if hasattr(mat, attr):
                    try:
                        setattr(mat, attr, value)
                    except (TypeError, ValueError):
                        pass
        mat.diffuse_color = (*colour, alpha)
        mats.append(mat)
    return mats


# ------------------------------------------------------------------ builder


class Build:
    """Accumulates raw verts/faces plus a material index per face."""

    def __init__(self):
        self.verts = []
        self.faces = []
        self.mats = []

    def add(self, verts, faces, mat, mtx=None):
        base = len(self.verts)
        if mtx is not None:
            verts = [mtx @ Vector(v) for v in verts]
        self.verts.extend(tuple(v) for v in verts)
        for face in faces:
            self.faces.append(tuple(i + base for i in face))
            self.mats.append(mat)

    def merge(self, other, mirror_x=False):
        base = len(self.verts)
        if mirror_x:
            self.verts.extend((-x, y, z) for x, y, z in other.verts)
            self.faces.extend(
                tuple(i + base for i in reversed(face)) for face in other.faces
            )
        else:
            self.verts.extend(other.verts)
            self.faces.extend(tuple(i + base for i in face) for face in other.faces)
        self.mats.extend(other.mats)


# ------------------------------------------------------------------ profiles


def ring(n, z, hx, hy, cx=0.0, cy=0.0, clamp_front=None):
    """Elliptical cross-section. The workhorse for bodies and armour."""
    pts = []
    for i in range(n):
        a = 2.0 * math.pi * (i + 0.5) / n
        py = cy + hy * math.sin(a)
        if clamp_front is not None and py < clamp_front:
            py = clamp_front
        pts.append((cx + hx * math.cos(a), py, z))
    return pts


def blade(z, hw, ht, cx=0.0, cy=0.0):
    """Lens cross-section: wide, thin, tapering to an edge at both tips."""
    return [
        (cx + hw, cy, z),
        (cx + hw * 0.45, cy - ht, z),
        (cx - hw * 0.45, cy - ht, z),
        (cx - hw, cy, z),
        (cx - hw * 0.45, cy + ht, z),
        (cx + hw * 0.45, cy + ht, z),
    ]


def skin_rows(rows, cap_bottom=True, cap_top=True):
    """Stitch a stack of equal-length point rows into a closed tube."""
    n = len(rows[0])
    verts = [p for row in rows for p in row]
    faces = []
    for r in range(len(rows) - 1):
        a, b = r * n, (r + 1) * n
        for i in range(n):
            j = (i + 1) % n
            faces.append((a + i, a + j, b + j, b + i))
    if cap_bottom:
        faces.append(tuple(range(n - 1, -1, -1)))
    if cap_top:
        base = (len(rows) - 1) * n
        faces.append(tuple(range(base, base + n)))
    return verts, faces


def loft(sections, n, cap_bottom=True, cap_top=True, clamp_front=None):
    """Skin elliptical sections. `sections` is (z, hx, hy, cx, cy)."""
    rows = [ring(n, *s, clamp_front=clamp_front) for s in sections]
    return skin_rows(rows, cap_bottom, cap_top)


def loft_blade(sections, cap_bottom=True, cap_top=True):
    """Skin lens sections -- flat parts that still have a rounded edge."""
    return skin_rows([blade(*s) for s in sections], cap_bottom, cap_top)


def tube(n, r0, r1, h, phase=0.0):
    """Round shaft. Segment count stays high enough not to read as a prism."""
    return skin_rows([ring(n, 0.0, r0, r0, phase), ring(n, h, r1, r1, phase)])


def shell(sections, n, arc, out, inn):
    """Curved armour plate hovering over a lofted body section.

    `arc` is the run of ring indices the plate covers, `out`/`inn` how far its
    outer and inner surfaces sit off the section underneath. Because it reuses
    the body's own cross-sections, the plate wraps rather than sitting proud
    as a slab.
    """
    rows = []
    for z, hx, hy, cx, cy in sections:
        rows.append(
            (
                [ring(n, z, hx + out, hy + out, cx, cy)[k] for k in arc],
                [ring(n, z, hx + inn, hy + inn, cx, cy)[k] for k in arc],
            )
        )
    m = len(arc)
    verts = []
    for outer, inner in rows:
        verts.extend(outer)
        verts.extend(inner)

    def o(r, k):
        return r * 2 * m + k

    def i(r, k):
        return r * 2 * m + m + k

    faces = []
    for r in range(len(rows) - 1):
        for k in range(m - 1):
            faces.append((o(r, k), o(r, k + 1), o(r + 1, k + 1), o(r + 1, k)))
            faces.append((i(r, k + 1), i(r, k), i(r + 1, k), i(r + 1, k + 1)))
        faces.append((o(r, 0), o(r + 1, 0), i(r + 1, 0), i(r, 0)))
        faces.append((o(r, m - 1), i(r, m - 1), i(r + 1, m - 1), o(r + 1, m - 1)))
    last = len(rows) - 1
    for k in range(m - 1):
        faces.append((o(0, k + 1), o(0, k), i(0, k), i(0, k + 1)))
        faces.append((o(last, k), o(last, k + 1), i(last, k + 1), i(last, k)))
    return verts, faces


# Arcs, by ring count. Front is -Y, back is +Y, her right is +X.
FRONT_12 = (6, 7, 8, 9, 10, 11)
BACK_12 = (0, 1, 2, 3, 4, 5)
FACE_12 = (7, 8, 9, 10)
EYE_L_12 = (7, 8)
EYE_R_12 = (9, 10)
MOUTH_12 = (8, 9)
FRONT_10 = (5, 6, 7, 8, 9)
BACK_10 = (0, 1, 2, 3, 4)
OUTER_10 = (9, 0)
FRONT_8 = (4, 5, 6, 7)
BACK_8 = (0, 1, 2, 3)


def taper_to_point(sections):
    """Collapse the last section so a loft ends in a tip, not a flat cap."""
    z, hx, hy, cx, cy = sections[-1]
    return sections[:-1] + [(z, hx * 0.06, hy * 0.06, cx, cy)]


def orb(r, seg_n=10, rings=4):
    verts = [(0.0, 0.0, -r)]
    for k in range(1, rings + 1):
        phi = math.pi * k / (rings + 1)
        rz, zz = r * math.sin(phi), -r * math.cos(phi)
        for i in range(seg_n):
            a = 2.0 * math.pi * i / seg_n
            verts.append((rz * math.cos(a), rz * math.sin(a), zz))
    verts.append((0.0, 0.0, r))
    top = len(verts) - 1
    faces = []
    for i in range(seg_n):
        j = (i + 1) % seg_n
        faces.append((0, 1 + j, 1 + i))
        faces.append((top, 1 + (rings - 1) * seg_n + i, 1 + (rings - 1) * seg_n + j))
    for k in range(rings - 1):
        a0, a1 = 1 + k * seg_n, 1 + (k + 1) * seg_n
        for i in range(seg_n):
            j = (i + 1) % seg_n
            faces.append((a0 + i, a0 + j, a1 + j, a1 + i))
    return verts, faces


def crescent(radius=0.135, width=0.062, thick=0.020, span=150.0, n=20,
             taper=1.7, m=6):
    """Crescent moon in the XZ plane, horns pointing up.

    Both the radial width and the thickness fall off toward the ends, so the
    horns come to real points instead of stopping as blunt bars.
    """
    span = math.radians(span)
    verts, faces = [], []
    for i in range(n + 1):
        t = -span + 2.0 * span * i / n
        fade = 1.0 - (abs(t) / span) ** taper
        w = max(width * fade, 0.004)
        th = max(thick * fade, 0.003)
        dx, dz = math.sin(t), -math.cos(t)
        rc = radius - w * 0.5
        for k in range(m):
            a = 2.0 * math.pi * (k + 0.5) / m
            rr = rc + w * 0.5 * math.cos(a)
            verts.append((rr * dx, th * math.sin(a), rr * dz))
    for i in range(n):
        a0, a1 = i * m, (i + 1) * m
        for k in range(m):
            j = (k + 1) % m
            faces.append((a0 + k, a0 + j, a1 + j, a1 + k))
    faces.append(tuple(range(m - 1, -1, -1)))
    faces.append(tuple(range(n * m, n * m + m)))
    return verts, faces


def at(loc=(0.0, 0.0, 0.0), rot=(0.0, 0.0, 0.0)):
    return Matrix.Translation(loc) @ Euler(rot, "XYZ").to_matrix().to_4x4()


def aim(p0, p1):
    """Matrix placing a +Z primitive so it runs from p0 to p1, plus its length."""
    d = Vector(p1) - Vector(p0)
    mtx = Matrix.Translation(p0) @ d.to_track_quat("Z", "Y").to_matrix().to_4x4()
    return mtx, d.length


def basis(origin, z_dir, normal):
    """Like `aim`, but with the primitive's local Y pinned to `normal`.

    Wing blades need their flat facing a chosen plane, which `to_track_quat`
    cannot express -- it always resolves roll against global Y.
    """
    z = Vector(z_dir).normalized()
    x = Vector(normal).normalized().cross(z).normalized()
    y = z.cross(x)
    return Matrix((
        (x.x, y.x, z.x, origin[0]),
        (x.y, y.y, z.y, origin[1]),
        (x.z, y.z, z.z, origin[2]),
        (0.0, 0.0, 0.0, 1.0),
    ))


def nacelle(b, base, tip, r_body, r_bell, mat, ribs=True):
    """Rocket pod: flared bell, glow disc, ribbed body."""
    mtx, length = aim(base, tip)
    b.add(*tube(12, r_bell, r_body * 0.92, length * 0.22), STEEL, mtx)
    b.add(*tube(12, r_bell * 0.80, r_bell * 0.80, 0.014), THRUST,
          mtx @ at((0.0, 0.0, 0.003)))
    b.add(*tube(12, r_body * 0.92, r_body, length * 0.78),
          mat, mtx @ at((0.0, 0.0, length * 0.22)))
    if ribs:
        for frac in (0.45, 0.68):
            b.add(*tube(12, r_body * 1.10, r_body * 1.10, 0.020), GOLD,
                  mtx @ at((0.0, 0.0, length * frac)))


def feather(b, root, tip, normal, hw_root, hw_tip, thick, mat, tip_mat=ENERGY):
    """Mechanical wing blade: a lens section swept root to tip, ending in a point."""
    length = (Vector(tip) - Vector(root)).length
    mtx = basis(root, Vector(tip) - Vector(root), normal)
    cut = length * 0.80
    hw_cut = hw_root + (hw_tip - hw_root) * 0.80
    b.add(*loft_blade([
        (0.0, hw_root, thick, 0.0, 0.0),
        (length * 0.42, hw_root * 0.92, thick, 0.0, 0.0),
        (cut, hw_cut, thick * 0.8, 0.0, 0.0),
    ]), mat, mtx)
    b.add(*loft_blade(taper_to_point([
        (cut, hw_cut, thick * 0.8, 0.0, 0.0),
        (length, hw_tip, thick * 0.5, 0.0, 0.0),
    ])), tip_mat, mtx)
    b.add(*loft([
        (0.0, hw_root * 0.34, thick * 2.0, 0.0, 0.0),
        (length * 0.34, hw_root * 0.22, thick * 1.5, 0.0, 0.0),
    ], 8), STEEL, mtx)


# ------------------------------------------------------------- proportions

# Torso: hips flare, waist pinches to ~0.73 of the bust, bust carries a
# forward offset. The chest and back plates reuse this section list.
TORSO = [
    (0.985, 0.150, 0.108, 0.0, 0.000),
    (1.060, 0.178, 0.122, 0.0, 0.000),
    (1.130, 0.162, 0.112, 0.0, 0.000),
    (1.210, 0.132, 0.096, 0.0, 0.000),
    (1.290, 0.146, 0.103, 0.0, -0.004),
    (1.375, 0.172, 0.119, 0.0, -0.012),
    (1.450, 0.181, 0.127, 0.0, -0.016),
    (1.520, 0.176, 0.109, 0.0, -0.004),
    (1.572, 0.150, 0.097, 0.0, 0.000),
    (1.620, 0.076, 0.069, 0.0, 0.000),
]

# Leg, ankle up to hip: calf swell, knee pinch, thigh.
LEG = [
    (0.320, 0.055, 0.062, 0.088, 0.000),
    (0.420, 0.063, 0.071, 0.090, 0.000),
    (0.540, 0.079, 0.091, 0.092, 0.004),
    (0.660, 0.067, 0.075, 0.094, 0.000),
    (0.780, 0.073, 0.079, 0.096, -0.002),
    (0.880, 0.087, 0.093, 0.098, 0.000),
    (1.000, 0.101, 0.107, 0.099, 0.000),
    (1.095, 0.110, 0.114, 0.100, 0.000),
]

# Arm, wrist up to shoulder. The outward splay pulls the arms clear of the
# torso silhouette -- tucked in, the figure reads as a slab.
ARM = [
    (0.940, 0.042, 0.046, 0.356, -0.010),
    (1.060, 0.048, 0.051, 0.336, -0.006),
    (1.200, 0.056, 0.059, 0.305, 0.000),
    (1.340, 0.059, 0.063, 0.269, 0.000),
    (1.480, 0.069, 0.073, 0.230, 0.000),
]

HEAD = [
    (1.660, 0.052, 0.060, 0.0, -0.010),
    (1.712, 0.079, 0.089, 0.0, -0.006),
    (1.768, 0.098, 0.103, 0.0, 0.000),
    (1.840, 0.102, 0.106, 0.0, 0.000),
    (1.900, 0.098, 0.101, 0.0, 0.000),
    (1.950, 0.079, 0.083, 0.0, 0.004),
    (1.982, 0.046, 0.049, 0.0, 0.006),
]

# Face-feature bands, interpolated off the skull so patches sit on the curve.
BROW = [(1.862, 0.1005, 0.1045, 0.0, 0.0), (1.884, 0.0995, 0.1035, 0.0, 0.0)]
EYES = [(1.818, 0.1015, 0.1055, 0.0, 0.0), (1.858, 0.1005, 0.1045, 0.0, 0.0)]
VISOR_BAND = [(1.806, 0.1015, 0.1055, 0.0, 0.0), (1.872, 0.1000, 0.1040, 0.0, 0.0)]
CHEEK = [(1.730, 0.0870, 0.0950, 0.0, -0.003), (1.752, 0.0910, 0.0980, 0.0, -0.002)]

# Hair shell: same skull, grown outward, with every front vertex clamped back
# to a plane at the temples so it frames the face instead of masking it.
HAIRLINE = [
    (1.600, 0.116, 0.108, 0.0, 0.014),
    (1.660, 0.124, 0.118, 0.0, 0.012),
    (1.730, 0.125, 0.122, 0.0, 0.008),
    (1.810, 0.122, 0.121, 0.0, 0.006),
    (1.880, 0.117, 0.117, 0.0, 0.006),
    (1.935, 0.106, 0.109, 0.0, 0.008),
    (1.975, 0.078, 0.082, 0.0, 0.010),
    (2.000, 0.042, 0.046, 0.0, 0.012),
]

HAIR_FRONT = -0.062

# Wing fan plane: U sweeps up-out-back, B sweeps out-back-level, and the
# blades sit at angles between them so they read as one wing.
WING_U = Vector((0.30, 0.26, 1.00)).normalized()
WING_B = Vector((0.90, 0.42, -0.22)).normalized()
WING_N = WING_U.cross(WING_B).normalized()
WING_ROOT = Vector((0.175, 0.245, 1.500))
WING_BLADES = (
    (8.0, 0.60, 0.094, 0.030),
    (26.0, 0.58, 0.088, 0.029),
    (44.0, 0.54, 0.078, 0.027),
    (61.0, 0.47, 0.066, 0.025),
    (77.0, 0.39, 0.055, 0.022),
)


# ------------------------------------------------------------------- pieces


def build_core(b):
    """Centreline: torso, head, hair, visor, backpack."""
    b.add(*loft(TORSO, 12), SUIT)

    # Fewer, larger plates than before -- the previous version banded the
    # torso into a dozen stripes and read as a striped corset.
    b.add(*shell(TORSO[4:8], 12, FRONT_12, 0.022, 0.004), WHITE)
    b.add(*shell(TORSO[4:8], 12, BACK_12, 0.020, 0.004), WHITE)
    b.add(*shell(TORSO[0:3], 12, FRONT_12, 0.020, 0.004), BLUE)
    b.add(*shell(TORSO[0:3], 12, BACK_12, 0.018, 0.004), BLUE)
    b.add(*shell(TORSO[3:5], 12, FRONT_12, 0.012, 0.004), GOLD)

    b.add(*orb(0.038), ENERGY, at((0, -0.158, 1.470)))

    b.add(*loft([(1.578, 0.052, 0.048, 0.0, 0.0),
                 (1.678, 0.048, 0.045, 0.0, 0.0)], 10), SUIT)
    b.add(*shell(TORSO[7:9], 12, BACK_12, 0.026, 0.008), STEEL)

    # --- head, with the face features as curved patches on the skull
    b.add(*loft(HEAD, 12), SKIN)
    b.add(*shell(EYES, 12, EYE_L_12, 0.005, -0.002), EYE)
    b.add(*shell(EYES, 12, EYE_R_12, 0.005, -0.002), EYE)
    b.add(*shell(BROW, 12, EYE_L_12, 0.006, -0.001), HAIR)
    b.add(*shell(BROW, 12, EYE_R_12, 0.006, -0.001), HAIR)
    b.add(*shell(CHEEK, 12, MOUTH_12, 0.004, -0.001), EYE)

    # visor: a thin curved band wrapping the face, not a slab bolted to it
    b.add(*shell(VISOR_BAND, 12, FACE_12, 0.020, 0.012), VISOR)
    for band, grow in ((VISOR_BAND[:1], 0.024), (VISOR_BAND[1:], 0.024)):
        z = band[0][0]
        b.add(*shell([(z - 0.006, 0.1015, 0.1055, 0.0, 0.0),
                      (z + 0.006, 0.1015, 0.1055, 0.0, 0.0)],
                     12, FACE_12, grow, 0.012), GOLD)

    # hair: bob shell clamped off the face, plus lofted fringe locks
    b.add(*loft(HAIRLINE, 12, clamp_front=HAIR_FRONT), HAIR)
    for i, sx in enumerate((-1.5, -0.5, 0.5, 1.5)):
        length = 0.112 if i % 2 else 0.094
        b.add(*loft(taper_to_point([
            (0.0, 0.046, 0.026, 0.0, 0.0),
            (length * 0.55, 0.040, 0.023, 0.0, 0.0),
            (length, 0.026, 0.015, 0.0, 0.0),
        ]), 8), HAIR,
            at((sx * 0.049, -0.092, 1.952), (math.pi - 0.26, -sx * 0.15, 0.0)))

    # backpack the wings hinge off
    b.add(*loft([(1.300, 0.140, 0.040, 0.0, 0.150),
                 (1.420, 0.150, 0.046, 0.0, 0.158),
                 (1.540, 0.128, 0.042, 0.0, 0.152)], 10), STEEL)


def build_side(b):
    """Everything on her +X half; mirrored to build the other side."""
    b.add(*loft(LEG, 10), SUIT)
    b.add(*loft(ARM, 8), SUIT)

    # --- boot: lofted foot through a flared mid-shin cuff
    boot = [
        (0.015, 0.060, 0.148, 0.088, -0.048),
        (0.090, 0.063, 0.142, 0.088, -0.042),
        (0.170, 0.059, 0.099, 0.089, -0.012),
        (0.260, 0.063, 0.077, 0.090, 0.002),
        (0.380, 0.073, 0.081, 0.091, 0.000),
        (0.520, 0.084, 0.093, 0.092, 0.002),
        (0.600, 0.093, 0.101, 0.092, 0.000),
        (0.632, 0.088, 0.096, 0.092, 0.000),
    ]
    b.add(*loft(boot, 10), WHITE)
    b.add(*shell(boot[0:3], 10, FRONT_10, 0.010, 0.002), GOLD)
    b.add(*shell(boot[4:8], 10, FRONT_10, 0.014, 0.002), BLUE)
    b.add(*loft([(-0.004, 0.036, 0.070, 0.088, -0.062),
                 (0.004, 0.036, 0.070, 0.088, -0.062)], 8), THRUST)
    nacelle(b, (0.090, 0.116, 0.030), (0.090, 0.098, 0.210), 0.040, 0.050,
            WHITE, ribs=False)

    # --- leg thrusters and plating over the dark suit
    nacelle(b, (0.092, 0.156, 0.560), (0.092, 0.176, 0.880), 0.048, 0.062, BLUE)
    b.add(*shell(LEG[3:6], 10, FRONT_10, 0.018, 0.003), BLUE)
    b.add(*shell(LEG[5:8], 10, FRONT_10, 0.020, 0.003), WHITE)
    b.add(*shell(LEG[6:8], 10, OUTER_10, 0.024, 0.004), WHITE)
    nacelle(b, (0.212, 0.062, 0.900), (0.204, 0.056, 1.090), 0.030, 0.038,
            STEEL, ribs=False)

    # hip plate, hung off the pelvis so it flares away from the leg
    b.add(*shell(TORSO[0:3], 12, (0, 1, 2), 0.034, 0.008), BLUE)
    b.add(*shell(TORSO[0:3], 12, (10, 11), 0.034, 0.008), BLUE)

    # --- shoulder, gauntlet, fist
    b.add(*shell(ARM[3:5], 8, (0, 1, 2, 3, 4), 0.036, 0.006), BLUE)
    b.add(*shell(ARM[3:5], 8, (0, 1), 0.050, 0.030), GOLD)
    b.add(*loft(taper_to_point([
        (0.0, 0.036, 0.058, 0.0, 0.0),
        (0.060, 0.030, 0.048, 0.0, 0.0),
        (0.100, 0.018, 0.030, 0.0, 0.0),
    ]), 8), GOLD, at((0.262, -0.028, 1.552), (-0.35, -0.70, 0.0)))
    b.add(*shell(ARM[0:3], 8, tuple(range(8)), 0.016, 0.003), WHITE)
    b.add(*tube(12, 0.068, 0.068, 0.020), GOLD, at((0.352, -0.008, 0.948)))
    b.add(*loft([(0.855, 0.030, 0.036, 0.362, -0.040),
                 (0.900, 0.036, 0.046, 0.362, -0.042),
                 (0.945, 0.034, 0.044, 0.360, -0.038)], 8), STEEL)

    # --- valkyrie helm wings, off the temple
    for i, (length, lift) in enumerate(((0.20, 0.10), (0.145, 0.035))):
        root = Vector((0.106, 0.000 - i * 0.020, 1.870 - i * 0.038))
        tip = root + Vector((0.085, 0.115, lift)).normalized() * length
        feather(b, root, tip, (-0.55, 0.45, 0.0), 0.028, 0.009, 0.007, GOLD,
                tip_mat=GOLD)

    # --- rocket wings: hinge, main pod, then one fanned blade sheet
    b.add(*loft([(1.408, 0.046, 0.040, 0.128, 0.196),
                 (1.532, 0.048, 0.042, 0.132, 0.200)], 8), STEEL)
    nacelle(b, (0.205, 0.200, 1.335), (0.300, 0.348, 1.730), 0.074, 0.094, WHITE)

    for i, (deg, length, hw_root, hw_tip) in enumerate(WING_BLADES):
        t = math.radians(deg)
        d = (WING_U * math.cos(t) + WING_B * math.sin(t)).normalized()
        root = WING_ROOT + Vector((0.010 * i, 0.022 * i, -0.016 * i))
        feather(b, root, root + d * length, WING_N, hw_root, hw_tip, 0.014, WHITE)

    nacelle(b, (0.352, 0.326, 1.308), (0.376, 0.284, 1.442), 0.030, 0.038, BLUE,
            ribs=False)


def build_sword(b):
    """Her right hand: mechanised broadsword, leaning forward out of the fist."""
    mtx = at((0.362, -0.040, 0.900), (0.52, 0.24, 0.0))

    b.add(*tube(10, 0.024, 0.030, 0.045), GOLD, mtx @ at((0, 0, -0.100)))
    b.add(*tube(10, 0.020, 0.020, 0.170), STEEL, mtx @ at((0, 0, -0.055)))
    b.add(*loft_blade([
        (0.120, 0.038, 0.030, 0.0, 0.0),
        (0.150, 0.110, 0.020, 0.0, 0.0),
        (0.166, 0.104, 0.016, 0.0, 0.0),
    ]), GOLD, mtx)
    b.add(*loft_blade(taper_to_point([
        (0.166, 0.048, 0.016, 0.0, 0.0),
        (0.520, 0.040, 0.012, 0.0, 0.0),
        (0.680, 0.026, 0.008, 0.0, 0.0),
        (0.770, 0.010, 0.004, 0.0, 0.0),
    ])), WHITE, mtx)
    b.add(*loft_blade([
        (0.190, 0.010, 0.015, 0.0, 0.0),
        (0.560, 0.008, 0.013, 0.0, 0.0),
    ]), ENERGY, mtx)


def build_staff(b):
    """Her left hand: crescent-moon staff, held upright in front of the arm."""
    mtx = at((-0.362, -0.102, 0.900))

    b.add(*tube(10, 0.026, 0.030, 0.060), GOLD, mtx @ at((0, 0, -0.560)))
    b.add(*tube(10, 0.022, 0.022, 1.240), STEEL, mtx @ at((0, 0, -0.520)))
    b.add(*tube(10, 0.029, 0.029, 0.180), BLUE, mtx @ at((0, 0, -0.090)))
    b.add(*tube(12, 0.030, 0.046, 0.080), GOLD, mtx @ at((0, 0, 0.720)))
    b.add(*crescent(), GOLD, mtx @ at((0, 0, 0.935)))
    b.add(*orb(0.050), ENERGY, mtx @ at((0, 0, 0.900)))


# ------------------------------------------------------------------ assembly


def assemble():
    core, side, extras = Build(), Build(), Build()
    build_core(core)
    build_side(side)
    build_sword(extras)
    build_staff(extras)

    out = Build()
    out.merge(core)
    out.merge(side)
    out.merge(side, mirror_x=True)
    out.merge(extras)
    return out


def finish(build, mats):
    mesh = bpy.data.meshes.new("Valkyrie_MV4")
    mesh.from_pydata(build.verts, [], build.faces)
    mesh.validate(verbose=False)

    for mat in mats:
        mesh.materials.append(mat)
    for poly, index in zip(mesh.polygons, build.mats):
        poly.material_index = index
        poly.use_smooth = False

    bm = bmesh.new()
    bm.from_mesh(mesh)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    bm.to_mesh(mesh)
    bm.free()

    colours = mesh.color_attributes.new("Col", "FLOAT_COLOR", "CORNER")
    for poly, index in zip(mesh.polygons, build.mats):
        rgb = PALETTE[index][1]
        for loop in poly.loop_indices:
            colours.data[loop].color = (*rgb, 1.0)

    if hasattr(mesh, "shade_flat"):
        mesh.shade_flat()

    obj = bpy.data.objects.new("Valkyrie_MV4", mesh)
    bpy.context.scene.collection.objects.link(obj)
    return obj


def stash_original():
    """Park the old blob in a hidden collection rather than deleting it."""
    blob = bpy.data.objects.get("mesh_0")
    if blob is None:
        return
    ref = bpy.data.collections.get("MV4_original_reference")
    if ref is None:
        ref = bpy.data.collections.new("MV4_original_reference")
        bpy.context.scene.collection.children.link(ref)
    for coll in list(blob.users_collection):
        coll.objects.unlink(blob)
    ref.objects.link(blob)
    blob.hide_viewport = True
    blob.hide_render = True
    layer = bpy.context.view_layer.layer_collection.children.get(ref.name)
    if layer:
        layer.exclude = True


def main():
    old = bpy.data.objects.get("Valkyrie_MV4")
    if old is not None:
        data = old.data
        bpy.data.objects.remove(old, do_unlink=True)
        if data.users == 0:
            bpy.data.meshes.remove(data)

    mats = build_materials()
    obj = finish(assemble(), mats)

    # deliberately unparented: the scene's "MainModel" empty is a glTF import
    # root carrying a 0.01 scale and a Y-up axis swap, which would shrink her
    # 100x. She is authored directly in world space.

    stash_original()

    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)

    tris = sum(len(p.vertices) - 2 for p in obj.data.polygons)
    print(
        f"Valkyrie_MV4: {len(obj.data.vertices)} verts, "
        f"{len(obj.data.polygons)} faces, {tris} tris, "
        f"{len(obj.data.materials)} materials"
    )
    return obj


if __name__ == "__main__":
    main()
