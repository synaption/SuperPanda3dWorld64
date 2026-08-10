"""Whistling allies into a squad, and sending them somewhere.

Two commands on one button, told apart by how long it is held:

  * held -- a circle grows on the ground where the view is pointing, up to a
    cap, with the arc a thrown thing would fly to reach it. Every ally inside
    the circle when the button comes up joins the squad and follows.
  * tapped -- the squad is sent to whatever spot the same aim resolves to.
    They walk there, spread out around it, and are on their own again once
    they arrive.

Which is Pikmin's shape rather than an RTS's: there is no cursor to drag a
box with, so the selection is aimed the same way the throw is, and the same
arc shows where it is going either way.

Nothing here touches Panda3D. Aiming needs the camera's position and focus and
the level's collision, and the rest is arithmetic on object positions, so the
whole thing is runnable headless -- see `tools/check_squad.py`. The front end
draws the circle and the arc from the numbers `aim_point`, `circle_radius` and
`throw_arc` hand back.
"""

import math

from .math_util import coss_f, s16, sins_f
from .objects import OBJECT_GRAVITY

# -- aiming -----------------------------------------------------------------

# How near and how far the aim may land, measured out from the player in the
# horizontal plane. Beyond the far one the arc stops meaning anything -- an
# ally sent 4000 units off is halfway across the field and out of the fight --
# and inside the near one the circle is drawn around his own feet.
AIM_MIN_RANGE = 250.0
AIM_MAX_RANGE = 2600.0

# The ray march that turns a view direction into a spot on the ground: how far
# apart the samples are, how many times the crossing is then halved, and how
# far above a sample the floor is looked for. The probe has to cover a whole
# step or a sample that lands just under a slope reads as still being above it.
AIM_STEP = 150.0
AIM_REFINE = 6
AIM_PROBE = AIM_STEP + 80.0

# Looking for the floor under a point the ray never reached: from this far
# above the player, so a hill that rises between them is still found, and
# stepping back toward him this far at a time when there is nothing there at
# all -- which over the moat and off the edge of the map is the usual answer.
AIM_SEARCH_UP = 1500.0
AIM_BACKOFF = 200.0

# Longer than a tap, in seconds. Under this the press is an order to the squad
# it already has; over it, it is a whistle for a new one.
TAP_SECONDS = 0.18

# The whistle circle: where it starts, where it stops, and how long it takes to
# grow between the two once it appears.
CIRCLE_MIN_RADIUS = 220.0
CIRCLE_MAX_RADIUS = 1100.0
CIRCLE_GROW_SECONDS = 1.1

# How far above his feet an ally may be and still be whistled. A circle is a
# flat thing drawn on the ground and reads as one, so the height is generous
# but not unbounded: somebody on the castle roof is not in a circle drawn on
# the lawn beneath him.
RECRUIT_HEIGHT = 800.0

# -- the arc ----------------------------------------------------------------

# How high the lob rises over the straight line between its ends, as a share of
# how long that line is, and a floor under it so that a throw at your own feet
# still arcs rather than reading as a flat streak.
ARC_APEX_RATIO = 0.30
ARC_MIN_APEX = 120.0
ARC_POINTS = 24

# -- the formation ----------------------------------------------------------

# Where the group gathers relative to the leader, and how far apart its members
# stand once they are there.
FOLLOW_DISTANCE = 330.0
FOLLOW_SPACING = 170.0
FOLLOW_ARRIVE = 110.0

# The same, for a spot they have been sent to. Wider, because nothing is moving
# the target around and a tight cluster there just means they shove each other.
SEND_SPACING = 200.0
SEND_ARRIVE = 140.0

# The angle between one slot in a cluster and the next. The golden angle is
# what keeps a spiral from lining its points up into spokes, which is the same
# reason a sunflower uses it: any simpler step leaves the allies in rows with
# gaps between them.
GOLDEN_ANGLE = 2.399963229728653


def _slot(index, spacing):
    """Offset of the index'th member of a loose cluster, in the plane.

    Not rotated to face anything. The cluster is placed by whoever calls this
    and the leader turning on the spot should not send everyone shuffling
    around him to keep a formation they were never in.
    """
    radius = spacing * math.sqrt(index)
    angle = index * GOLDEN_ANGLE
    return radius * math.sin(angle), radius * math.cos(angle)


def _along(origin, direction, distance):
    return (origin[0] + direction[0] * distance,
            origin[1] + direction[1] * distance,
            origin[2] + direction[2] * distance)


def _underground(surfaces, point):
    """Is this point at or below the floor beneath it?"""
    height, floor = surfaces.find_floor(point[0], point[1] + AIM_PROBE, point[2])
    return floor is not None and point[1] <= height, height


def _ray_ground(surfaces, origin, direction, start, end):
    """March a ray until it goes underground. Returns the distance, or None.

    Coarse steps and then a handful of bisections rather than fine steps
    throughout: this runs every frame the button is held, and each sample is a
    collision query. Six halvings of a 150-unit step land the crossing inside
    two and a half units, which is far finer than anything downstream of it.
    """
    previous = start
    distance = start
    while distance <= end:
        under, _ = _underground(surfaces, _along(origin, direction, distance))
        if under:
            low, high = previous, distance
            for _ in range(AIM_REFINE):
                middle = (low + high) * 0.5
                under, _ = _underground(surfaces,
                                        _along(origin, direction, middle))
                low, high = (low, middle) if under else (middle, high)
            return high
        previous = distance
        distance += AIM_STEP
    return None


def _view_ray(camera, player_pos):
    """The crosshair's line, as (origin, direction, where the player is on it).

    A shooter camera can hand this over directly -- it is built from the yaw
    and pitch the view was placed with, so it is exact even on a frame the boom
    is being shoved through a wall, and it accounts for the shoulder offset,
    which a line drawn between two placed points does not.

    The fallback subtracts the camera's focus from its position, which is what
    this always used to do and is all a camera without an `aim_ray` can offer.
    Returns None when there is no usable ray at all.
    """
    if hasattr(camera, "aim_ray"):
        origin, direction = camera.aim_ray()
        # Where along the ray the player is: the march starts there rather than
        # at the camera, since the ground between the camera and his back is
        # behind him and a target there points the arc the wrong way.
        start = ((player_pos[0] - origin[0]) * direction[0]
                 + (player_pos[1] - origin[1]) * direction[1]
                 + (player_pos[2] - origin[2]) * direction[2])
        return origin, direction, max(start, 1.0)

    origin = camera.pos
    focus = (camera.focus[0], camera.focus[1] + 60.0, camera.focus[2])
    dx = focus[0] - origin[0]
    dy = focus[1] - origin[1]
    dz = focus[2] - origin[2]
    length = math.sqrt(dx * dx + dy * dy + dz * dz)
    if length < 1.0:
        return None
    return origin, (dx / length, dy / length, dz / length), length


def aim_point(surfaces, camera, player_pos, player_facing=0):
    """Where on the ground the crosshair is pointing, as (x, y, z).

    The crosshair is the middle of the screen and the aim is the ray out of it,
    picked up from the camera and marched until it meets ground. Left and right
    is where the view is pointed; up and down is range, since a view tilted
    down meets the ground nearer and one tilted up throws the meeting further
    out. That is the whole of the aim, and it is why the reticle never has to
    leave the middle of the screen.

    The answer is a point in front of the player rather than the ray's own hit:
    on the bearing from *him* to that hit, so the camera sitting off his
    shoulder does not skew where the throw is aimed, at the distance the ray
    chose -- pulled back to AIM_MAX_RANGE when it is beyond throwing, pushed
    out to AIM_MIN_RANGE when the view is pointed at his own feet, and walked
    back toward him until there is floor under it when it is out over the moat
    or off the edge of the world. A throw does not have to land exactly where
    it was pointed; it does have to land somewhere.
    """
    ray = _view_ray(camera, player_pos)
    if ray is None:
        # The camera has not been placed yet, which is only ever true on the
        # first frame. Aim along the player's own facing.
        facing = (sins_f(player_facing), coss_f(player_facing))
        return _ground_ahead(surfaces, player_pos, facing, AIM_MIN_RANGE)
    origin, direction, start = ray

    flat = math.hypot(direction[0], direction[2])
    if flat < 1e-4:
        # Straight down. Nothing to aim along; put it at his feet.
        return (player_pos[0], player_pos[1], player_pos[2])
    heading = (direction[0] / flat, direction[2] / flat)

    hit = _ray_ground(surfaces, origin, direction,
                      start, start + AIM_MAX_RANGE * 1.5)
    if hit is None:
        distance = AIM_MAX_RANGE
    else:
        point = _along(origin, direction, hit)
        dx = point[0] - player_pos[0]
        dz = point[2] - player_pos[2]
        distance = math.hypot(dx, dz)
        # The bearing from him to what the crosshair found, which is the aim
        # the player is actually taking. Only the ray's own heading is left
        # when the two coincide, where a bearing would be noise.
        if distance > 1.0:
            heading = (dx / distance, dz / distance)

    return _ground_ahead(surfaces, player_pos, heading,
                         min(max(distance, AIM_MIN_RANGE), AIM_MAX_RANGE))


def _ground_ahead(surfaces, player_pos, heading, distance):
    """The floor `distance` along `heading`, backing off until there is one."""
    while distance >= AIM_MIN_RANGE:
        x = player_pos[0] + heading[0] * distance
        z = player_pos[2] + heading[1] * distance
        height, floor = surfaces.find_floor(
            x, player_pos[1] + AIM_SEARCH_UP, z)
        if floor is not None:
            return (x, height, z)
        distance -= AIM_BACKOFF
    # Standing over a hole with more hole in front of him. His own feet are
    # the one place known to be somewhere.
    return (player_pos[0], player_pos[1], player_pos[2])


def circle_radius(grow_seconds):
    """How wide the whistle circle has grown after this long at full size."""
    share = min(max(grow_seconds / CIRCLE_GROW_SECONDS, 0.0), 1.0)
    return CIRCLE_MIN_RADIUS + (CIRCLE_MAX_RADIUS - CIRCLE_MIN_RADIUS) * share


def throw_arc(start, end, points=ARC_POINTS):
    """Sample the lob that would put a thrown thing down at `end`.

    Nothing is actually thrown -- the allies walk -- so this is a preview, but
    it is the preview of a real throw: the height comes from the object gravity
    the rest of the simulation falls under, so the shape is the one a goomba out
    of a pipe flies and not a curve chosen to look like one.

    Flight time follows from the apex rather than being picked: a lob that
    rises `a` over its own chord is in the air sqrt(8a / -g) ticks, whatever it
    is crossing.
    """
    dx = end[0] - start[0]
    dz = end[2] - start[2]
    apex = max(ARC_MIN_APEX, math.hypot(dx, dz) * ARC_APEX_RATIO)
    ticks = math.sqrt(8.0 * apex / -OBJECT_GRAVITY)
    rise = ((end[1] - start[1]) - 0.5 * OBJECT_GRAVITY * ticks * ticks) / ticks

    out = []
    for i in range(points + 1):
        share = i / points
        t = ticks * share
        out.append((start[0] + dx * share,
                    start[1] + rise * t + 0.5 * OBJECT_GRAVITY * t * t,
                    start[2] + dz * share))
    return out


class Squad:
    """The allies following the player, and the ones on their way somewhere.

    Membership lives here rather than on the allies themselves, so the order
    they joined in is the order they take up slots in -- which is what keeps
    the formation from reshuffling every time one of them is pruned -- and so
    that the behaviour code stays a thing that reads one goal and walks to it.

    The one thing written onto an ally is that goal: `(x, z, arrive)`, refreshed
    every tick. An ally with no goal is nobody's business and goes back to
    wandering, which is also what happens to one that has arrived where it was
    sent.
    """

    def __init__(self, object_set, ally_class):
        self.objects = object_set
        self.ally_class = ally_class
        self.members = []          # following the leader
        # [ally, x, z, arrived] -- sent to a spot, and whether they are on it
        # yet. They keep the goal once they are: an ally sent somewhere holds
        # it until he is whistled up again, which is what makes sending them
        # an order rather than a suggestion. He will still leave it to hit
        # something that comes near, and walks back to it afterwards.
        self.sent = []

    # -- who is available ---------------------------------------------------

    def allies(self):
        """Every ally in the field that is up and about."""
        return [obj for obj in self.objects.objects
                if isinstance(obj, self.ally_class)
                and obj.active and not obj.defeated]

    def in_circle(self, centre, radius):
        """The allies a whistle at `centre` would reach."""
        found = []
        for ally in self.allies():
            dx = ally.pos[0] - centre[0]
            dz = ally.pos[2] - centre[2]
            if math.hypot(dx, dz) > radius + ally.radius:
                continue
            if abs(ally.pos[1] - centre[1]) > RECRUIT_HEIGHT:
                continue
            found.append(ally)
        return found

    # -- the two commands ---------------------------------------------------

    def recruit(self, centre, radius):
        """Whistle up everyone inside the circle. Returns how many joined.

        One already on the way somewhere is called back rather than ignored:
        the whistle is how an order is taken back, and an ally who kept walking
        to the last spot because he was already walking would read as deaf.
        """
        joined = 0
        for ally in self.in_circle(centre, radius):
            if ally in self.members:
                continue
            self._drop_order(ally)
            self.members.append(ally)
            joined += 1
        return joined

    def send(self, target):
        """Send the whole squad to a spot, spread around it. Returns how many."""
        count = len(self.members)
        for index, ally in enumerate(self.members):
            dx, dz = _slot(index, SEND_SPACING)
            self.sent.append([ally, target[0] + dx, target[2] + dz, False])
        self.members = []
        return count

    @property
    def marching(self):
        """Sent somewhere and not there yet."""
        return sum(1 for entry in self.sent if not entry[3])

    @property
    def holding(self):
        """Sent somewhere and standing on it."""
        return sum(1 for entry in self.sent if entry[3])

    def disband(self):
        """Everyone back to their own devices, following or sent alike."""
        for ally in self.members:
            ally.goal = None
        self.members = []
        for entry in self.sent:
            entry[0].goal = None
        self.sent = []

    # -- the tick -----------------------------------------------------------

    def update(self, leader):
        """Refresh every goal. Call once a tick, before the objects update."""
        self._prune()

        # Behind the leader, so walking forward drags the group along rather
        # than through him, and so the slots stay put while he turns.
        behind = s16(leader.face_angle[1] + 0x8000)
        anchor_x = leader.pos[0] + FOLLOW_DISTANCE * sins_f(behind)
        anchor_z = leader.pos[2] + FOLLOW_DISTANCE * coss_f(behind)

        for index, ally in enumerate(self.members):
            dx, dz = _slot(index, FOLLOW_SPACING)
            ally.goal = (anchor_x + dx, anchor_z + dz, FOLLOW_ARRIVE)

        for entry in self.sent:
            ally, x, z, arrived = entry
            ally.goal = (x, z, SEND_ARRIVE)
            if not arrived and ally.on_ground and math.hypot(
                    x - ally.pos[0], z - ally.pos[2]) <= SEND_ARRIVE:
                entry[3] = True

    def _prune(self):
        """Drop anyone who has been switched off or killed.

        Their goal goes with them: a Mario stood down while the player is being
        Mario comes back when the player swaps away again, and would otherwise
        come back marching to a spot he was sent to in another life.
        """
        def keep(ally):
            if ally.active and not ally.defeated:
                return True
            ally.goal = None
            return False

        self.members = [a for a in self.members if keep(a)]
        self.sent = [e for e in self.sent if keep(e[0])]

    def _drop_order(self, ally):
        for entry in list(self.sent):
            if entry[0] is ally:
                self.sent.remove(entry)
