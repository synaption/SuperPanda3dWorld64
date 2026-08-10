"""Level objects: the trees the level places, a couple of enemies, and the
warp pipes that produce more of them.

Objects are simulated on the same fixed 30 Hz tick as Mario and use the same
surface queries, so they stand on the same floors and are stopped by the same
walls. They are much simpler than he is -- one velocity, one yaw, gravity, and
a small behaviour state machine -- because that is all the originals are.

Nothing here touches Panda3D. Objects carry a position, a yaw and the name of
the clip they want playing, and the front end draws whatever it finds, which
keeps the simulation runnable headless for testing.
"""

import math
import random

from .math_util import atan2s, coss, s16, sins
from .surfaces import WallCollisionData

# Objects fall at this rate until they land, matching the object gravity the
# originals use rather than Mario's, which is stronger.
OBJECT_GRAVITY = -4.0
TERMINAL_VELOCITY = -78.0

# Frames an object stays visible after being defeated, so the hit reads. Up
# here rather than beside the interaction code because anything that can kill
# something reaches for it, and by now that is the pipes' clientele as well as
# whoever is being played.
DEATH_FRAMES = 12

# How long a launched object is flown ballistically before its own behaviour
# gets it back, in case the arc never finds a floor to land on. Comfortably
# longer than the arc itself, which is about a second.
LAUNCH_MAX_TICKS = 120


class Object:
    """Base for anything that stands on the floor and can be drawn."""

    model = None
    radius = 40.0
    height = 60.0
    anim = None
    # Extra scale the behaviour applies on top of the model's own. The geo
    # bakes in the authored scale; this is the per-object size the game sets
    # at spawn, which is how one goomba model serves the regular, huge and
    # tiny variants.
    draw_scale = 1.0
    # How the whole object faces the camera, if at all. SM64 sets this from
    # the *behaviour* rather than the geo layout, which is why a tree's geo
    # carries no GEO_BILLBOARD despite a tree plainly being one.
    #   None   - ordinary geometry
    #   "axis" - turns about the vertical only, so it never tips over
    billboard = None

    def __init__(self, surfaces, x, y, z, yaw=0):
        self.surfaces = surfaces
        self.pos = [float(x), float(y), float(z)]
        self.home = [float(x), float(y), float(z)]
        self.vel_y = 0.0
        self.forward_vel = 0.0
        self.yaw = s16(yaw)
        self.floor_height = 0.0
        self.floor = None
        self.on_ground = False
        self.timer = 0
        self.active = True
        # The set this belongs to, filled in by ObjectSet.spawn. Most objects
        # never look at it; the ones that hunt or spawn need to see the rest.
        self.world = None
        # Counts down after a defeat, so the hit is visible before it vanishes.
        self.dying = 0
        # Frames left of a launch. While this is running the object flies the
        # arc it was thrown on and its behaviour is suspended; see `coast`.
        self.launched = 0
        # Killed, as opposed to merely switched off. A pipe counts its brood by
        # this rather than by `active`, so standing an object down -- which is
        # what happens to the Mario NPCs when Mario himself is being played --
        # does not read as a vacancy the pipe should fill.
        self.defeated = False
        # Distance and bearing to Mario, refreshed once a tick because the
        # behaviours below read them several times each.
        self.dist_to_mario = 0.0
        self.angle_to_mario = 0

        self.snap_to_floor()

    # -- shared movement ----------------------------------------------------

    def snap_to_floor(self):
        height, floor = self.surfaces.find_floor(
            self.pos[0], self.pos[1] + 200.0, self.pos[2])
        if floor is not None:
            self.pos[1] = height
            self.floor_height, self.floor = height, floor

    def observe(self, mario):
        dx = mario.pos[0] - self.pos[0]
        dz = mario.pos[2] - self.pos[2]
        self.dist_to_mario = math.hypot(dx, dz)
        self.angle_to_mario = atan2s(dz, dx)

    def turn_toward(self, target_yaw, rate):
        """Rotate toward a heading, at most `rate` binary units this tick."""
        delta = s16(target_yaw - self.yaw)
        self.yaw = s16(self.yaw + max(-rate, min(rate, delta)))
        return delta

    def move(self):
        """Advance by the current velocity. Returns True if a wall was hit."""
        step_x = self.forward_vel * sins(self.yaw)
        step_z = self.forward_vel * coss(self.yaw)

        next_pos = [self.pos[0] + step_x,
                    self.pos[1] + self.vel_y,
                    self.pos[2] + step_z]

        data = WallCollisionData(next_pos[0], next_pos[1], next_pos[2],
                                 self.height * 0.5, self.radius)
        hit_wall = self.surfaces.find_wall_collisions(data) > 0
        next_pos[0], next_pos[2] = data.x, data.z

        height, floor = self.surfaces.find_floor(
            next_pos[0], next_pos[1] + 50.0, next_pos[2])
        if floor is None:
            # Walked off the edge of the world; stay put rather than fall out.
            return True

        self.floor_height, self.floor = height, floor
        if next_pos[1] <= height:
            next_pos[1] = height
            self.vel_y = 0.0
            self.on_ground = True
        else:
            self.on_ground = False
            self.vel_y = max(self.vel_y + OBJECT_GRAVITY, TERMINAL_VELOCITY)

        self.pos = next_pos
        return hit_wall

    def update(self, mario):
        self.timer += 1

    def coast(self):
        """Fly the arc a pipe threw it on, with its behaviour suspended.

        Every one of these behaviours writes its own speed each tick -- a
        goomba bleeds 0.4 a frame off whatever it has back toward a walk, and a
        scuttlebug simply overwrites it with its crawl -- so a launch handed to
        the behaviour is gone within a tick or two and the mob lands back on
        the pipe it came out of. Flying the whole arc here is what makes the
        distance thrown mean anything.
        """
        self.timer += 1
        self.launched -= 1
        self.move()
        if self.on_ground:
            self.launched = 0

    def defeat(self):
        """Take the killing blow: play the death frames, then leave."""
        if self.defeated:
            return False
        self.defeated = True
        self.dying = DEATH_FRAMES
        return True

    # -- what the renderer asks for ----------------------------------------

    @property
    def draw_pos(self):
        return self.pos

    @property
    def draw_yaw(self):
        return self.yaw


class Tree(Object):
    """Static scenery. The level places these; they never move."""

    model = "tree"
    radius = 80.0
    height = 400.0
    # bhvTree is CYLBOARD/BILLBOARD: it turns to face the camera about the
    # vertical axis. Without it the trees are flat cards seen from one fixed
    # side and vanish to a line as you walk around them.
    billboard = "axis"

    def update(self, mario):
        pass


class Goomba(Object):
    """Wanders, and charges Mario if he comes close.

    The original picks a new heading on a timer, occasionally hopping, and
    switches to a much faster chase inside 500 units. Speed is approached
    rather than set so the change of pace reads as acceleration.
    """

    model = "goomba"
    radius = 70.0
    height = 100.0
    # The hitbox ratio (0.47x mario) turned out to undersell them badly on
    # screen: a hitbox height of 50 is the collision cylinder, not the model,
    # and a goomba's is far shorter than the thing you see. Sized to read
    # correctly instead, at roughly three quarters of mario's height.
    draw_scale = 1.5

    CHASE_RANGE = 500.0
    WALK_SPEED = 4.0 / 3.0
    CHASE_SPEED = 20.0
    TURN_RATE = 0x200
    JUMP_VELOCITY = 20.0

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.target_yaw = self.yaw
        self.relative_speed = self.WALK_SPEED
        self.walk_timer = random.randint(20, 30)
        self.anim = "walk"

    def update(self, mario):
        self.timer += 1
        self.observe(mario)

        chasing = self.dist_to_mario < self.CHASE_RANGE
        if chasing:
            # Jumping on sight is what makes a goomba noticing you readable.
            if self.relative_speed <= 2.0 and self.on_ground:
                self.vel_y = self.JUMP_VELOCITY
            self.target_yaw = self.angle_to_mario
            self.relative_speed = self.CHASE_SPEED
        else:
            self.relative_speed = self.WALK_SPEED
            if self.walk_timer > 0:
                self.walk_timer -= 1
            else:
                # Mostly a gentle turn; occasionally a hop and a sharp one.
                if random.randrange(4):
                    self.target_yaw = s16(self.yaw + random.randint(-0x2000, 0x2000))
                    self.walk_timer = random.randint(100, 200)
                else:
                    if self.on_ground:
                        self.vel_y = self.JUMP_VELOCITY
                    self.target_yaw = s16(self.yaw + random.randint(-0x6000, 0x6000))
                    self.walk_timer = random.randint(20, 30)

        self.turn_toward(self.target_yaw, self.TURN_RATE)

        target_speed = self.relative_speed
        self.forward_vel += max(-0.4, min(0.4, target_speed - self.forward_vel))

        if self.move():
            # Bounced off something: pick a new way to go rather than grind.
            self.target_yaw = s16(self.yaw + 0x8000
                                  + random.randint(-0x2000, 0x2000))

        # The walk cycle keeps pace with how fast he is actually going.
        self.anim = "walk"
        self.anim_rate = max(0.4, abs(self.forward_vel) * 0.4)


class Mario(Object):
    """Mario, now that the Hero is the one being played.

    He keeps his own animation set -- the clips are still in
    assets/mario/mario.glb, addressed by the same `anim_XX` names his action
    code uses -- but none of that action code runs here. An NPC needs a
    fraction of it, and driving the real state machine from a fake controller
    would mean pretending to press buttons to get him to walk in a circle.

    So this is the same shape as the goomba: wander a while, stand a while, and
    turn to watch whoever is playing when they come near. What it borrows from
    the decomp is the part that reads on screen -- the clips, and the walk
    cycle keeping pace with his actual speed.

    On top of that he fights: an enemy inside HUNT_RANGE is dropped everything
    for, run down and punched. That takes priority over the wandering and over
    greeting the player, since a Mario who stops to wave while a goomba walks
    into him reads as broken rather than as friendly.
    """

    model = "mario"
    radius = 37.0
    height = 160.0

    # MARIO_ANIM_IDLE_HEAD_CENTER and MARIO_ANIM_WALKING, the two his own
    # action code uses for standing about and for walking.
    ANIM_IDLE = "anim_C5"
    ANIM_WALK = "anim_48"
    ANIM_RUN = "anim_72"
    ANIM_JUMP = "anim_4D"
    ANIM_SWIM = "anim_AA"
    ANIM_SWIM_GLIDE = "anim_AB"
    # MARIO_ANIM_FIRST_PUNCH_FAST -- the short jab, rather than the wind-up
    # version, because it is over in ten frames and so reads as one blow.
    ANIM_PUNCH = "anim_69"

    # Close enough that he notices and turns to watch.
    NOTICE_RANGE = 700.0
    WALK_SPEED = 8.0
    SWIM_SPEED = 10.0
    TURN_RATE = 0x400
    JUMP_VELOCITY = 42.0
    JUMP_MIN_TICKS = 90
    JUMP_MAX_TICKS = 240
    SWIM_DEPTH = 80.0
    SWIM_STROKE_TICKS = 14

    # -- fighting --
    # How far off he will notice something worth hitting. Generous, so a mob
    # coming out of a pipe halfway across the field is still answered.
    HUNT_RANGE = 3500.0
    CHASE_SPEED = 22.0
    # His own hitbox radius plus the length of an arm; the enemy's own radius
    # is added to it, so a wide scuttlebug is hit from further out than a
    # goomba is, which is what stops him punching at thin air beside it.
    STRIKE_RANGE = 90.0
    # Only swings at something he is roughly facing, so the punch lands where
    # the animation points rather than out of his shoulder.
    STRIKE_ALIGNED = 0x2000
    # Frames the jab holds him still, and how far into it the blow lands.
    PUNCH_TICKS = 10
    PUNCH_CONTACT = 3

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.target_yaw = self.yaw
        # Staggered so several Marios do not step off together.
        self.walk_timer = random.randint(30, 150)
        self.resting = True
        self.swimming = False
        self.jump_timer = random.randint(self.JUMP_MIN_TICKS,
                                         self.JUMP_MAX_TICKS)
        self.swim_timer = 0
        self.anim = self.ANIM_IDLE
        self.anim_rate = 1.0
        self.target = None
        self.punch_timer = 0

    def update(self, player):
        self.timer += 1
        self.observe(player)

        water_level = self.surfaces.find_water_level(self.pos[0], self.pos[2])
        if water_level is not None and self.pos[1] < water_level - 20.0:
            self.swimming = True
        elif water_level is None:
            self.swimming = False

        if self.swimming:
            self._swim(water_level)
            return

        # Fighting outranks everything else he does on land, including a
        # half-finished swing: the jab has to play out before he moves again.
        if self.punch_timer > 0 or self._acquire_target():
            self._fight()
            return

        if self.dist_to_mario < self.NOTICE_RANGE:
            # Stop and face the player rather than wandering off mid-greeting.
            self.resting = True
            self.walk_timer = random.randint(30, 90)
            self.target_yaw = self.angle_to_mario
        elif self.walk_timer > 0:
            self.walk_timer -= 1
        else:
            self.resting = not self.resting
            if self.resting:
                self.walk_timer = random.randint(60, 180)
            else:
                self.walk_timer = random.randint(90, 240)
                self.target_yaw = s16(self.yaw + random.randint(-0x4000, 0x4000))

        self.turn_toward(self.target_yaw, self.TURN_RATE)

        target_speed = 0.0 if self.resting else self.WALK_SPEED
        self.forward_vel += max(-0.5, min(0.5, target_speed - self.forward_vel))

        # An occasional jump keeps his wandering from reading as a looped
        # walk animation. Only launch from solid ground, never in greeting.
        self.jump_timer -= 1
        if self.jump_timer <= 0 and self.on_ground and not self.resting:
            self.vel_y = self.JUMP_VELOCITY
            self.on_ground = False
            self.jump_timer = random.randint(self.JUMP_MIN_TICKS,
                                             self.JUMP_MAX_TICKS)

        if self.move() and not self.resting:
            self.target_yaw = s16(self.yaw + 0x8000
                                  + random.randint(-0x2000, 0x2000))

        if not self.on_ground:
            self.anim = self.ANIM_JUMP
            self.anim_rate = 1.0
        # Below a crawl the walk cycle reads as a stumble, so he stands instead.
        elif abs(self.forward_vel) < 1.0:
            self.anim = self.ANIM_IDLE
            self.anim_rate = 1.0
        else:
            self.anim = self.ANIM_WALK
            # The divisor his own animation code uses for the walk cycle.
            self.anim_rate = max(0.4, abs(self.forward_vel) / 4.0)

    def _acquire_target(self):
        """The nearest enemy worth crossing the field for, if there is one."""
        self.target = None
        if self.world is None:
            return None
        nearest = self.HUNT_RANGE
        for obj in self.world.objects:
            if not obj.active or obj.defeated:
                continue
            if not isinstance(obj, (Goomba, Scuttlebug)):
                continue
            distance = math.hypot(obj.pos[0] - self.pos[0],
                                  obj.pos[2] - self.pos[2])
            if distance < nearest:
                self.target, nearest = obj, distance
        return self.target

    def _fight(self):
        """Run the current target down, then jab it."""
        target = self.target

        if self.punch_timer > 0:
            self.punch_timer -= 1
            # He plants his feet for the swing. The blow lands partway through
            # rather than on the first frame, so the enemy falls to the punch
            # instead of to having been stood next to.
            self.forward_vel = 0.0
            if (self.punch_timer == self.PUNCH_TICKS - self.PUNCH_CONTACT
                    and target is not None and not target.defeated
                    and self._in_reach(target)):
                target.defeat()
            self.move()
            self.anim = self.ANIM_PUNCH
            self.anim_rate = 1.0
            return

        self.resting = False
        dx = target.pos[0] - self.pos[0]
        dz = target.pos[2] - self.pos[2]
        bearing = atan2s(dz, dx)
        # Measured before the turn, so a target he is already facing can be
        # hit on the same tick he closes on it.
        aligned = abs(s16(bearing - self.yaw)) < self.STRIKE_ALIGNED
        self.turn_toward(bearing, self.TURN_RATE)

        if aligned and self.on_ground and self._in_reach(target):
            self.punch_timer = self.PUNCH_TICKS
            self.forward_vel = 0.0
            self.move()
            self.anim = self.ANIM_PUNCH
            self.anim_rate = 1.0
            return

        self.forward_vel += max(-1.0, min(
            1.0, self.CHASE_SPEED - self.forward_vel))
        if self.move():
            # A wall between him and it. Slide along rather than grind into it.
            self.yaw = s16(self.yaw + 0x2000)

        if not self.on_ground:
            self.anim = self.ANIM_JUMP
            self.anim_rate = 1.0
        else:
            self.anim = self.ANIM_RUN
            self.anim_rate = max(0.4, abs(self.forward_vel) / 4.0)

    def _in_reach(self, target):
        """Close enough to hit, in the plane and vertically both."""
        dx = target.pos[0] - self.pos[0]
        dz = target.pos[2] - self.pos[2]
        reach = self.STRIKE_RANGE + target.radius
        if dx * dx + dz * dz > reach * reach:
            return False
        return (self.pos[1] <= target.pos[1] + target.height
                and self.pos[1] + self.height >= target.pos[1])

    def _swim(self, water_level):
        """Paddle through a water box near its surface."""
        self.resting = False
        self.swim_timer = (self.swim_timer + 1) % self.SWIM_STROKE_TICKS

        # Pick a fresh course every few strokes, with the same broad wandering
        # turns he uses on land.
        if self.swim_timer == 0 and random.randrange(3) == 0:
            self.target_yaw = s16(self.yaw + random.randint(-0x3000, 0x3000))
        self.turn_toward(self.target_yaw, self.TURN_RATE)
        self.forward_vel += max(-0.4, min(
            0.4, self.SWIM_SPEED - self.forward_vel))

        target_y = water_level - self.SWIM_DEPTH
        self.vel_y = max(-3.0, min(3.0, target_y - self.pos[1]))
        if self.move():
            self.target_yaw = s16(self.yaw + 0x8000
                                  + random.randint(-0x2000, 0x2000))

        self.anim = (self.ANIM_SWIM if self.swim_timer < 10
                     else self.ANIM_SWIM_GLIDE)
        self.anim_rate = 1.0


class Scuttlebug(Object):
    """Crawls toward Mario and lunges when it lines him up.

    Note: the behaviour here works, but the *model* does not render correctly
    yet. Most of a scuttlebug's body is drawn as billboarded quads that the
    original rotates to face the camera every frame. glTF has no billboard
    concept, so they are exported as ordinary geometry and collapse to thin
    lines when seen edge-on. Panda3D's billboard effect cannot fix it in place
    either, since it acts on a node's transform while this geometry is skinned
    to character joints. See README.

    Unlike the goomba it always knows where Mario is; the interest is in the
    lunge, which only triggers once it is roughly facing him and which it
    commits to for a while afterwards.
    """

    model = "scuttlebug"
    radius = 60.0
    height = 80.0
    # Hitbox is 70 tall but 130 across -- a wide, low spider -- so it wants a
    # bit more than the height ratio alone suggests.
    draw_scale = 1.6

    CRAWL_SPEED = 5.0
    LUNGE_SPEED = 15.0
    LUNGE_VELOCITY = 20.0
    ALIGNED = 0x800
    TURN_RATE = 0x200
    LUNGE_FRAMES = 50

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.lunging = False
        self.lunge_timer = 0
        self.anim = "crawl"
        self.anim_rate = 1.0

    def update(self, mario):
        self.timer += 1
        self.observe(mario)

        if self.lunging:
            self.forward_vel = self.LUNGE_SPEED
            self.lunge_timer += 1
            if self.lunge_timer > self.LUNGE_FRAMES:
                self.lunging = False
        else:
            self.forward_vel = self.CRAWL_SPEED
            self.lunge_timer = 0
            # Only pounces once it is pointed at him, so the lunge reads as
            # deliberate rather than as a sideways skid.
            aligned = abs(s16(self.angle_to_mario - self.yaw)) < self.ALIGNED
            if aligned and self.on_ground and self.dist_to_mario < 1500.0:
                self.lunging = True
                self.vel_y = self.LUNGE_VELOCITY

        self.turn_toward(self.angle_to_mario, self.TURN_RATE)

        if self.move():
            # Recoils off walls instead of pressing into them -- but only with
            # its feet down. The hop is re-applied on every frame of contact,
            # so a scuttlebug held against a wall in mid-air is thrown up by it
            # again before gravity has taken the last one back, and climbs the
            # cliff by the corner of the map like a ladder.
            self.forward_vel = -10.0
            if self.on_ground:
                self.vel_y = 30.0
            self.lunging = False

        self.anim = "crawl"
        self.anim_rate = 1.0 if not self.lunging else 2.0


class WarpPipe(Object):
    """A pipe that things climb out of, up to a population it then holds.

    The count is of its own brood rather than of the level: the field already
    has enemies placed by hand, and a pipe that counted those would stop short
    of its own quota or never fire at all. So each pipe is responsible for
    exactly what it produced, and for replacing it when it dies.

    The countdown only runs while there is room. At the cap it holds where it
    stands, so a kill is what starts the clock again rather than restarting it
    -- which is the difference between "one along every thirty seconds" and a
    replacement appearing the instant something dies.
    """

    model = "warp_pipe"
    # The collision the original gives it: a 306-unit square, 205 tall.
    radius = 150.0
    height = 205.0

    # Gravity takes 4 a frame back, so a launch at v peaks v*v/8 units up and
    # is in the air for v/2 frames: 60 clears the 205-unit rim by as much
    # again, and stays up for a full second on the way.
    LAUNCH_VELOCITY = 60.0
    # Carried outwards for the whole of that second, so it lands about 600
    # units out -- four pipe-widths, and far enough that five of them come down
    # spread around it rather than in a stack on top of it.
    LAUNCH_SPEED = 20.0

    # How far below the pipe a landing spot may be and still count as somewhere
    # to throw something. Thrown this far, direction matters: the west pipe
    # stands a few hundred units from where the ground drops 1700 into the
    # moat, and a share of its goombas were coming down in the water.
    LANDING_DROP = 400.0
    LANDING_TRIES = 8

    def __init__(self, surfaces, x, y, z, yaw=0,
                 spawns=None, interval=30 * 30, limit=5):
        super().__init__(surfaces, x, y, z, yaw)
        self.spawns = spawns          # the class it produces
        self.interval = interval      # ticks between one and the next
        self.limit = limit            # how many of them it keeps alive
        self.brood = []
        self.countdown = interval

    @property
    def population(self):
        return len(self.brood)

    def update(self, mario):
        self.timer += 1
        if self.spawns is None or self.world is None:
            return

        self.brood = [mob for mob in self.brood if not mob.defeated]
        if len(self.brood) >= self.limit:
            return

        self.countdown -= 1
        if self.countdown > 0:
            return

        self.countdown = self.interval
        self.launch()

    def _launch_heading(self):
        """A direction with something to land on at the end of it."""
        # Where the arc puts it down: airborne for twice the time it takes
        # gravity to cancel the launch, carried outwards throughout.
        reach = self.LAUNCH_SPEED * 2.0 * self.LAUNCH_VELOCITY / -OBJECT_GRAVITY
        yaw = 0
        for _ in range(self.LANDING_TRIES):
            yaw = random.randint(0, 0xFFFF)
            x = self.pos[0] + reach * sins(yaw)
            z = self.pos[2] + reach * coss(yaw)
            height, floor = self.surfaces.find_floor(x, self.pos[1] + 200.0, z)
            if floor is not None and height > self.pos[1] - self.LANDING_DROP:
                return yaw
        # Ringed by drops, apparently. Throw it anyway rather than not at all.
        return yaw

    def launch(self):
        """Throw one out of the mouth, in a direction of its own."""
        # Spawned at the pipe's feet rather than at its lip: the launch carries
        # it up through the barrel and out, which is what the pop is, and it
        # starts hidden inside instead of appearing in mid-air above.
        mob = self.world.spawn(self.spawns, self.pos[0], self.pos[1],
                               self.pos[2], self._launch_heading())
        mob.vel_y = self.LAUNCH_VELOCITY
        mob.forward_vel = self.LAUNCH_SPEED
        mob.on_ground = False
        mob.launched = LAUNCH_MAX_TICKS
        # Mario has a clip for being in the air; the enemies have the one clip
        # each and keep it.
        mob.anim = getattr(mob, "ANIM_JUMP", mob.anim)
        self.brood.append(mob)
        return mob


# -- interaction ------------------------------------------------------------
#
# Enemies resolve against Mario as an upright cylinder each, the way the
# original does: a radius in the horizontal plane and a height above the
# object's feet. Which of the two wins is decided by where Mario is vertically,
# not by who touched whom first.

# How far above an enemy's feet counts as landing on top of it.
STOMP_MARGIN = 0.6

# Upward speed Mario gets from a successful stomp.
BOUNCE_VELOCITY = 42.0

# How hard a hit throws Mario back.
KNOCKBACK_SPEED = 24.0
KNOCKBACK_VELOCITY = 20.0

# How long Mario is immune after a hit. Roughly a second, which is long enough
# for the knockback to carry him clear of whatever hit him.
INVINCIBLE_FRAMES = 30


class Interactions:
    """Resolves Mario against every enemy once a tick.

    Kept out of the objects themselves because it needs to write to Mario, and
    the object behaviours are otherwise readable without knowing anything about
    his action state machine.
    """

    def __init__(self, object_set):
        self.objects = object_set
        self.defeated = 0
        self.hits_taken = 0

    def resolve(self, mario):
        from . import audio
        from .mario import constants as C

        hero_voice = getattr(mario, "voice_profile", None) == "hero"

        # Being knocked back leaves Mario inside the enemy that hit him, so
        # without a cooldown the same touch re-triggers every tick and he is
        # hit three or four times for walking into one goomba once.
        if mario.invinc_timer > 0:
            mario.invinc_timer -= 1
            return

        for obj in self.objects.objects:
            if not obj.active or not isinstance(obj, (Goomba, Scuttlebug)):
                continue
            # Already dead and playing it out. The count is run by the object
            # set, so it keeps running whether or not it is resolved here.
            if obj.dying:
                continue

            dx = mario.pos[0] - obj.pos[0]
            dz = mario.pos[2] - obj.pos[2]
            reach = obj.radius + 37.0          # 37 is Mario's own hitbox radius
            if dx * dx + dz * dz > reach * reach:
                continue

            top = obj.pos[1] + obj.height
            # Vertical overlap: his feet below the enemy's head, his head above
            # its feet. Without this he is "touching" it from a storey up.
            if mario.pos[1] > top or mario.pos[1] + 160.0 < obj.pos[1]:
                continue

            stomping = (mario.vel[1] < 0.0
                        and mario.pos[1] > obj.pos[1] + obj.height * STOMP_MARGIN)
            attacking = bool(mario.action & C.ACT_FLAG_ATTACKING)

            if stomping or attacking:
                obj.defeat()
                self.defeated += 1
                if attacking:
                    mario.sound_events.append(
                        audio.SOUND_HERO_VICTORY_HIT if hero_voice
                        else C.SOUND_MARIO_YAHOO)
                else:
                    mario.sound_events.append(C.SOUND_ACTION_TERRAIN_LANDING)
                if stomping:
                    mario.bounce_off_enemy(BOUNCE_VELOCITY)
            else:
                # Thrown away from the enemy, facing it, the way a hit reads.
                away = s16(atan2s(dz, dx) + 0x8000)
                mario.sound_events.append(
                    audio.SOUND_HERO_HIT if hero_voice else C.SOUND_MARIO_OOOF)
                mario.take_enemy_hit(away, KNOCKBACK_SPEED, KNOCKBACK_VELOCITY)
                mario.invinc_timer = INVINCIBLE_FRAMES
                self.hits_taken += 1
                return


MODELS = {"tree": Tree, "goomba": Goomba, "scuttlebug": Scuttlebug,
          "mario": Mario, "warp_pipe": WarpPipe}

# What the level's special-object presets correspond to here.
PRESET_MODELS = {"special_bubble_tree": Tree}


class ObjectSet:
    """Every object in the area, updated once per tick."""

    def __init__(self, surfaces):
        self.surfaces = surfaces
        self.objects = []

    def spawn(self, cls, x, y, z, yaw=0, **kwargs):
        obj = cls(self.surfaces, x, y, z, yaw, **kwargs)
        obj.world = self
        self.objects.append(obj)
        return obj

    def load_special_objects(self, entries):
        """Spawn whatever the level's special-object list describes.

        Presets with no counterpart here are skipped rather than guessed at --
        the list also holds warps and geo markers, which are not objects.
        """
        spawned = 0
        for entry in entries:
            cls = PRESET_MODELS.get(entry.get("preset"))
            if cls is None:
                continue
            x, y, z = entry["pos"]
            self.spawn(cls, x, y, z, entry.get("yaw", 0))
            spawned += 1
        return spawned

    def update(self, mario):
        # Over a copy of the list, because a pipe firing appends to it: the
        # new arrival is left to start on the following tick rather than being
        # updated on the one it was created in.
        for obj in list(self.objects):
            if not obj.active:
                continue
            if obj.dying:
                # Held still for its death frames, then gone. Counted here
                # rather than in the interaction pass so that whatever landed
                # the blow -- the player, or one Mario NPC on the warpath --
                # it plays out the same way.
                obj.dying -= 1
                if obj.dying == 0:
                    obj.active = False
                continue
            if obj.launched:
                # Thrown by a pipe and still in the air. Its behaviour gets it
                # back the moment it lands.
                obj.coast()
                continue
            obj.update(mario)

    def of_model(self, model):
        return [o for o in self.objects if o.model == model]

    def of_class(self, cls):
        return [o for o in self.objects if isinstance(o, cls)]
