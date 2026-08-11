"""A third-person shooter camera: a spring arm behind the shoulder.

What was here before was a follow camera in Lakitu's spirit -- it trailed
Mario, it could be swung around him, and its yaw was what the analog stick was
measured against.  That is the right camera for a platformer and the wrong one
for a game you aim in, for one reason above all the others: the player's own
look input was *eased*.  Pushing the stick set a target yaw and the camera
crept toward it over a couple of hundred milliseconds, so the view was always
somewhere the player had asked to be a moment ago.  On a platformer that reads
as weight.  On anything with a crosshair it reads as latency, because the
crosshair is the thing being steered and it never arrives.

So the rule this camera is built on:

    **The player's look input is never smoothed.  Everything else is.**

A mouse delta or a stick push moves the view on the frame it arrives, one to
one, with no spring between the hand and the angle.  What *is* smoothed is
everything the player did not ask for -- the character walking around under the
camera, the ground rising into stairs, a wall sliding in behind the boom, the
transition into and out of the sights.  Those are the motions that want easing,
and easing them is what makes the result feel smooth rather than merely fast.

The pieces:

  * **A pivot that follows him, not tracks him.**  The boom hangs off a point
    at chest height that chases the character with a critically damped spring
    (`smooth_damp` below -- Unity's, the same algebra).  Horizontally it is
    quick, 55 ms, which is enough to take the stair-step off the 30 Hz
    simulation without feeling loose.  Vertically it is slower and has a dead
    band: small changes in his height -- steps, kerbs, a slope -- do not move
    the camera at all, which is what stops a run up the castle path from
    pumping the horizon up and down.

  * **A boom that pulls in hard and pushes out soft.**  The segment from the
    pivot to where the camera wants to be is marched against the collision;
    anything in the way shortens it.  Coming in is instantaneous, because a
    camera that eases into a wall spends those milliseconds inside it.  Going
    back out takes a third of a second, because a camera that snaps back out
    the moment a pillar clears reads as a jolt.  That asymmetry is the whole
    trick, and it is why the two are different constants.

  * **A shoulder offset**, so he stands to one side of the screen instead of
    on top of what you are aiming at.  It is applied along the camera's own
    right and up axes, and the collision march runs to the offset position, so
    backing into a corner folds the offset away with the rest of the boom.

  * **Sights.**  `set_aim` blends the whole rig -- boom length, shoulder,
    field of view, look sensitivity, pitch limits -- toward a tight
    over-the-shoulder framing.  It takes an amount rather than a flag, so an
    analog control could hold it half way in; the front end currently hands it
    a nought or a one.

  * **A ray, and only one.**  The camera looks exactly along its own yaw and
    pitch: no separate focus point above his head, no look-at that quietly
    disagrees with where the boom is pointing.  That is what makes the
    crosshair mean something -- `aim_ray()` is the line out of the middle of
    the screen, and it is the same line the view was built from.

And the corollary, which is what decides several of the arguments below: the
camera may slide *along* that ray as freely as it likes, because a ray is the
same ray whichever point on it you start from, and it may not move off it
without the player having asked.  Occlusion shortens the boom -- it never lifts
the camera over things.  Nothing re-points the view but the player: there is no
drift back behind him while he runs, no framing assist, no correction.  The
only thing here that turns the view on its own is the recentre, which is a
button.

Everything with a number in it is an attribute rather than a constant read at
use, so the console's sliders can move it while the game runs.
"""

import math

from .math_util import (
    coss_f, degrees_to_s16, s16, sins_f, to_panda,
)
from .surfaces import WallCollisionData

# -- framing -----------------------------------------------------------------

# How far above his feet the boom pivots.  Chest height on the Hero, who stands
# a little over 200 units: the camera swings around the part of him that does
# the aiming rather than around his ankles, which is what keeps the horizon
# still while he turns.
PIVOT_HEIGHT = 160.0

# Boom length at the hip and down the sights.  Both are far shorter than the
# 1500 the follow camera used, and that number is the clearest measure of what
# has changed: a platformer's camera stands back so you can see the ledge you
# are jumping to, and at 1500 units the Hero is a hundred pixels tall and what
# he is pointing at is a smudge.  A shooter's camera stands where you can read
# what is in front of him.
HIP_DISTANCE = 820.0
AIM_DISTANCE = 430.0

# The shoulder offset, along the camera's own right and up axes, at the hip and
# at the sights.  Positive right moves the *camera* right, which puts him on
# the left of the screen and leaves the middle of it looking at open ground.
#
# The two land in very different places despite being nearly the same number,
# which is worth knowing before either is changed: what decides where he sits
# on screen is the offset against the *width the view covers at his distance*,
# and the sights cut both the distance and the field of view. The same offset
# at the sights therefore pushes him farther toward the edge than at the hip.
#
# The negative lift is the other half of that.  The boom pivots well above his
# head -- which is what a camera swinging around him wants -- and at the
# sights, where the view covers barely two hundred units top to bottom, that
# would drop him off the foot of the screen.  Dropping the camera instead of
# the pivot lifts him back into the corner without touching the point the boom
# turns around.
HIP_SHOULDER = (70.0, 0.0)
AIM_SHOULDER = (70.0, -25.0)

# Field of view, horizontal degrees.  45 is what the game has always run at, so
# the hip view is unchanged; the sights pull in to a little over two thirds of
# it, which is a zoom you feel without it becoming a scope.
HIP_FOV = 45.0
AIM_FOV = 32.0

# How much a flat sprint widens the view and lengthens the boom.  Small on
# purpose: this is the kind of thing that is felt rather than seen, and a big
# number turns every stop and start into a lurch.
SPEED_REFERENCE = 38.0      # the Hero's default top speed, units per tick
SPEED_FOV_KICK = 4.0
SPEED_DOLLY = 110.0
SPEED_SMOOTH = 0.45

# Pitch, in radians, positive looking *down* (the camera rises as it looks
# down, which is what the sign means geometrically).  Far wider than the old
# -0.30..0.65: that range was set by a camera that had to keep itself out of
# the floor by hand, and this one shortens the boom instead, so it can look
# down at his feet and up over the castle.
PITCH_MIN, PITCH_MAX = -0.80, 0.95
AIM_PITCH_MIN, AIM_PITCH_MAX = -1.10, 1.15
DEFAULT_PITCH = 0.10

# -- following ---------------------------------------------------------------

# Seconds for the pivot to close on him.  Horizontal is quick enough to feel
# attached and slow enough to swallow the 33 ms simulation step and the
# quarter-step pushback off a wall.
FOLLOW_SMOOTH = 0.055

# Vertical is its own problem.  A dead band means his height has to change by
# more than a step before the camera answers at all, so stairs, kerbs and the
# ordinary bob of a run leave the horizon alone; past the band the camera moves
# to the edge of it rather than to him, which is what makes the recovery smooth
# instead of a catch-up lurch.  The air gets a wider band and a slower spring:
# a jump you can see the top of reads better than one the camera rides.
GROUND_DEADZONE = 55.0
GROUND_SMOOTH = 0.22
AIR_DEADZONE = 130.0
AIR_SMOOTH = 0.38
# ...but never let him leave the frame.  A jetpack climb outruns any spring,
# and this is the leash that keeps him on screen when it does.
MAX_VERTICAL_LAG = 520.0
# How far off the floor he has to be for the air rules to apply.
AIRBORNE_HEIGHT = 60.0

# -- the boom ----------------------------------------------------------------

# How fat the camera is when the level is asked whether it fits.
CAMERA_RADIUS = 90.0
FLOOR_CLEARANCE = 90.0
CEILING_CLEARANCE = 60.0

# Nearest the camera may ever sit to the pivot.  At this range it is nearly a
# first-person view, which is the right answer when he is jammed into a corner
# and there is nowhere else for the camera to be.
MIN_DISTANCE = 190.0

# Seconds for the boom to grow back once whatever shortened it has gone.  The
# other direction has no constant because it has no delay: see the module
# docstring.
BOOM_RETURN = 0.32

# How far in and out the player may set the boom by hand, and how fast the
# stick moves it.  The bounds are the console sliders' own, so the two ways of
# setting the same number cannot disagree about what is reachable.  The rate is
# a little over a boom length a second: fast enough to go from over his shoulder
# to a wide view in the time a thumb stays on the stick, slow enough to stop
# where you meant to.
BOOM_MIN, BOOM_MAX = 250.0, 4000.0
AIM_BOOM_MIN, AIM_BOOM_MAX = 150.0, 2000.0
DOLLY_SPEED = 900.0

# Seconds the shoulder offset takes to fold away against a wall and to come
# back out.  Both directions, because unlike the boom this one moves the camera
# sideways, and sideways is the direction the aim can feel.
FOLD_SMOOTH = 0.18

# How finely the pivot-to-camera segment is sampled for occlusion.  A step of
# about 110 units against a camera radius of 90 means nothing thinner than the
# camera can slip between two samples.
OCCLUSION_STEP = 110.0
OCCLUSION_MAX_SAMPLES = 14
# Once the coarse march finds an occupied sample, this many bisection passes
# locate the boundary.  The march keeps the common no-obstacle path cheap; the
# refinement removes its 110-unit quantisation when a wall or slope is close.
OCCLUSION_REFINE_STEPS = 5

# -- look input --------------------------------------------------------------

# Mouse: degrees of view per hundred pixels of movement.  Pointer positions
# arrive as whole screen pixels, so this is also the smallest un-smoothed turn
# the player can ask for.  Six gives a 0.06-degree increment (0.033 down
# the sights), which makes one-pixel adjustments usable without requiring a
# separate precision-look mode.
MOUSE_SENSITIVITY = 6.0
# How long a mouse delta is spread over.  Two hundredths of a second is below
# the threshold where it reads as lag and above the one where a 125 Hz mouse
# sampled at 200 fps stair-steps.  Set it to zero for a raw 1:1 pointer.
MOUSE_SMOOTHING = 0.02

# Stick: degrees per second at full deflection, before the ramp.
STICK_YAW_SPEED = 250.0
STICK_PITCH_SPEED = 170.0
# The response curve.  Squaring the magnitude and keeping the direction gives
# fine aim near the centre and the full rate at the rim, which is the shape
# every console shooter uses and the reason a stick can aim at all.
STICK_EXPONENT = 2.0
# Turn ramp: hold the stick near its rim and the turn rate climbs to this
# multiple over this many seconds, then drops back the moment it is released.
# Without it a stick has to choose between turning around quickly and tracking
# something slowly; with it, it does both.
STICK_RAMP_BOOST = 0.85
STICK_RAMP_SECONDS = 0.45
STICK_RAMP_THRESHOLD = 0.85
STICK_RAMP_RELEASE = 6.0

# What aiming does to the sensitivity of both.  Down the sights the same hand
# movement covers less angle, which is the whole point of aiming.
AIM_SENSITIVITY = 0.55

# Seconds the two transitions take.  In is faster than out: raising a weapon is
# a decision and lowering it is a relaxation, and matching the two makes the
# aim feel mushy to press.
AIM_IN_SMOOTH = 0.11
AIM_OUT_SMOOTH = 0.18

# -- recentring ---------------------------------------------------------------

# Recentring (R, or the right shoulder) is not a snap.  It is a quarter-second
# spring onto his back, which arrives fast enough to be a command and slow
# enough to be followed by eye.
RECENTER_SMOOTH = 0.22

# -- shake -------------------------------------------------------------------

# A landing kick, decaying.  The three frequencies are deliberately not
# multiples of each other so the sum never repeats inside the time it lasts,
# which is what keeps it from reading as a wobble.
SHAKE_DECAY = 1.6
SHAKE_YAW = 1.5             # degrees at full trauma
SHAKE_PITCH = 2.2
SHAKE_ROLL = 1.1
SHAKE_FREQUENCIES = (13.7, 17.3, 23.1)


def _wrap_angle(value):
    """Wrap a float binary angle into [-0x8000, 0x8000), keeping the fraction.

    s16() does the same thing but truncates to an integer, which is what the
    simulation wants and the camera does not.
    """
    return (value + 0x8000) % 0x10000 - 0x8000


def smooth_damp(current, target, velocity, smooth_time, dt):
    """A critically damped spring, solved rather than integrated.

    Returns (value, velocity).  This is Unity's SmoothDamp and the algebra is
    its algebra: the exact solution of a critically damped spring over the
    step, with the exponential replaced by a Pade approximation that is
    accurate to a part in ten thousand over any step this will ever see.

    Critically damped rather than exponential -- `current += (target - current)
    * rate * dt` and its frame-rate-independent cousin -- because the two
    differ in exactly the way that is felt.  Exponential smoothing is fastest
    at the instant the target moves and slows from there, so a target that
    changes velocity produces a corner in the output.  A spring carries its own
    velocity across the change, so the output is smooth in its first derivative
    as well as its value.  That is the difference between a camera that follows
    and a camera that glides, and it costs four multiplies.
    """
    smooth_time = max(smooth_time, 1e-4)
    omega = 2.0 / smooth_time
    x = omega * dt
    decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x)

    change = current - target
    temp = (velocity + omega * change) * dt
    velocity = (velocity - omega * temp) * decay
    result = target + (change + temp) * decay

    # Stop dead on the target rather than ringing through it.  Overshoot in a
    # critically damped spring is numerical, not physical, and letting it stand
    # shows up as a camera that shivers when it arrives.
    if (target - current > 0.0) == (result > target):
        result = target
        velocity = 0.0
    return result, velocity


def _smooth_damp_angle(current, target, velocity, smooth_time, dt):
    """`smooth_damp` on the short way round a circle of binary angles."""
    target = current + _wrap_angle(target - current)
    value, velocity = smooth_damp(current, target, velocity, smooth_time, dt)
    return _wrap_angle(value), velocity


def _approach(current, target, rate, dt):
    """Move toward a target at a fixed rate per second, stopping on it."""
    step = rate * dt
    if current < target:
        return min(current + step, target)
    return max(current - step, target)


def _clamp(value, lo, hi):
    return lo if value < lo else (hi if value > hi else value)


def _ease(t):
    """Smoothstep, for blends the player watches rather than steers.

    A linear blend into the sights starts and stops abruptly at both ends; this
    is flat at both, so the move has no seam where it begins or where it lands.
    """
    t = _clamp(t, 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


class FollowCamera:
    """The camera, and the only thing that decides where the view points.

    Constructed with the character it should follow; hand it a different one
    through `target` when the game switches which is being played.
    """

    def __init__(self, surfaces, mario):
        self.surfaces = surfaces
        self.target = mario

        # -- what the console is allowed to move --------------------------
        self.distance = HIP_DISTANCE
        self.aim_distance = AIM_DISTANCE
        self.height = PIVOT_HEIGHT
        self.shoulder = HIP_SHOULDER[0]
        self.aim_shoulder = AIM_SHOULDER[0]
        self.base_fov = HIP_FOV
        self.aim_fov = AIM_FOV
        self.mouse_sensitivity = MOUSE_SENSITIVITY
        self.mouse_smoothing = MOUSE_SMOOTHING
        self.stick_speed = STICK_YAW_SPEED
        self.stick_pitch_speed = STICK_PITCH_SPEED
        self.follow_smooth = FOLLOW_SMOOTH
        self.shake_scale = 1.0
        self.invert_pitch = False

        # -- angles --------------------------------------------------------
        # Yaw is the bearing from the character *to* the camera, kept as a
        # float in binary-angle units.  Rounding it to whole s16 units each
        # frame would truncate any step smaller than one unit to zero, so a
        # slow pan stalls and then jumps: easing 30 degrees at 60 fps moved on
        # only 53 of 239 frames.  Gameplay still sees a whole-unit angle,
        # through the `mario_yaw` property.
        self.yaw = float(s16(mario.face_angle[1] + 0x8000))
        self.pitch = DEFAULT_PITCH

        # -- state ---------------------------------------------------------
        self.pos = [0.0, 0.0, 0.0]
        self.focus = [0.0, 0.0, 0.0]
        self.fov = self.base_fov

        self._pivot = [0.0, 0.0, 0.0]
        self._pivot_vel = [0.0, 0.0, 0.0]
        self._initialised = False

        self._boom = self.distance
        self._boom_vel = 0.0
        self._shoulder_fold = 1.0
        self._fold_vel = 0.0

        self._aim = 0.0             # where the sights actually are, eased
        self._aim_raw = 0.0         # the same, before the ease
        self._aim_target = 0.0      # where the player is asking for them

        self._speed_blend = 0.0
        self._speed_vel = 0.0

        self._ramp = 0.0            # the stick's turn ramp, 0..1
        self._pending_mouse = [0.0, 0.0]

        self._recentring = False
        self._recenter_held = False
        self._recenter_vel = 0.0
        self._recenter_pitch_vel = 0.0

        self._trauma = 0.0
        self._shake_time = 0.0

    # -- compatibility --------------------------------------------------------

    @property
    def mario(self):
        """The character being followed, under the name the old camera used."""
        return self.target

    @mario.setter
    def mario(self, value):
        self.target = value

    # -- look input -----------------------------------------------------------

    def look(self, right_degrees, up_degrees):
        """Turn the view.  Positive is right and up, as the player sees it.

        Applied immediately and in full.  This is the one input in the whole
        camera that gets no smoothing of any kind, and the reason is the module
        docstring's: a smoothed look input is latency wearing a costume.
        """
        if self.invert_pitch:
            up_degrees = -up_degrees
        scale = self.look_scale
        # The view turning right is the camera swinging left around him, which
        # is why the yaw -- a bearing to the camera -- takes the sign inverted.
        self.yaw = _wrap_angle(self.yaw - degrees_to_s16(right_degrees * scale))
        self.pitch = self._clamp_pitch(
            self.pitch - math.radians(up_degrees * scale))
        if right_degrees or up_degrees:
            # Looking cancels a recentre in flight: the player has taken the
            # view back, and finishing the spring would be the camera taking
            # it away again.
            self._recentring = False

    def look_mouse(self, dx_pixels, dy_pixels):
        """Feed a mouse delta.  Positive dy is upward, as Panda3D reports it.

        The delta is banked rather than applied, and `update` pays it out over
        `mouse_smoothing` seconds.  At the default 20 ms that is most of it on
        the frame it arrived and the remainder on the next, which is enough to
        take the stair-step off a 125 Hz mouse being read at 200 fps without
        putting anything the hand can feel between the two.
        """
        self._pending_mouse[0] += dx_pixels
        self._pending_mouse[1] += dy_pixels

    def look_stick(self, x, y, dt):
        """Feed a look stick, already past its deadzone.  Positive is right/up.

        Everything a stick needs to aim rather than merely turn is here: a
        squared response so the middle of its travel is fine control, and a
        ramp so holding it at the rim accelerates into a fast turn without
        costing the slow one.
        """
        magnitude = math.hypot(x, y)
        if magnitude <= 0.0:
            # Drop the ramp quickly rather than instantly: flicking through
            # centre while adjusting a turn should not cost the whole ramp.
            self._ramp = max(0.0, self._ramp - STICK_RAMP_RELEASE * dt)
            return

        magnitude = min(magnitude, 1.0)
        was = self._ramp
        if magnitude >= STICK_RAMP_THRESHOLD:
            self._ramp = min(1.0, self._ramp + dt / STICK_RAMP_SECONDS)
        else:
            self._ramp = max(0.0, self._ramp - STICK_RAMP_RELEASE * dt)

        curved = magnitude ** STICK_EXPONENT / magnitude
        # The ramp across the middle of the frame rather than at either end.
        # Taking it at one end integrates a rate that is changing with a step
        # of dt, and the error in that is a turn that comes out several percent
        # different at 30 fps and at 240 -- which is exactly what this whole
        # method exists to avoid.
        boost = 1.0 + (was + self._ramp) * 0.5 * STICK_RAMP_BOOST
        self.look(x * curved * self.stick_speed * boost * dt,
                  y * curved * self.stick_pitch_speed * boost * dt)

    @property
    def look_scale(self):
        """What the sights do to sensitivity, blended with them."""
        return 1.0 + (AIM_SENSITIVITY - 1.0) * self._aim

    def rotate(self, delta_degrees):
        """Swing the *camera* left or right.  The legacy sign: Q and E's."""
        self.look(-delta_degrees, 0.0)

    def tilt(self, delta):
        """Raise or lower the *camera*, in radians.  The legacy sign."""
        self.look(0.0, -math.degrees(delta))

    def dolly(self, amount, dt):
        """Push the boom in or pull it out.  Positive pulls out.

        Which of the two lengths it moves follows which one is in effect: at the
        hip it sets the hip distance, down the sights it sets the sights', so
        the player frames the two independently instead of one dragging the
        other about.  Nothing else has to be told -- the boom is re-measured
        against this every frame, and shortening it only slides the camera along
        the aim ray, so the point under the crosshair does not move while it
        travels.
        """
        if not amount:
            return
        step = amount * DOLLY_SPEED * dt
        if self._aim >= 0.5:
            self.aim_distance = _clamp(self.aim_distance + step,
                                       AIM_BOOM_MIN, AIM_BOOM_MAX)
        else:
            self.distance = _clamp(self.distance + step, BOOM_MIN, BOOM_MAX)

    # -- the sights -----------------------------------------------------------

    def set_aim(self, amount):
        """How far down the sights to be, 0 to 1.

        An amount rather than a flag, so an analog control can hold the camera
        part of the way in.  Nothing is bound to one at the moment -- the pad
        aims on a right-stick click, which latches -- but the blend is built to
        be driven that way and the transitions read correctly at any value.
        """
        self._aim_target = _clamp(float(amount), 0.0, 1.0)

    @property
    def aiming(self):
        """Is the player asking for the sights at all?"""
        return self._aim_target > 0.01

    @property
    def aim_amount(self):
        """Where the sights actually are, eased.  What the HUD should read."""
        return self._aim

    def shake(self, amount):
        """Add trauma -- a landing, a hit.  Decays on its own from there."""
        self._trauma = _clamp(self._trauma + amount, 0.0, 1.0)

    # -- the frame ------------------------------------------------------------

    def update(self, dt, target_pos=None, recenter=False):
        if dt <= 0.0:
            return
        m = self.target
        # Follow the interpolated render position when one is supplied; the raw
        # simulation position only moves in 30 Hz steps and would judder.
        pos = target_pos if target_pos is not None else m.pos

        self._flush_mouse(dt)
        self._update_aim_blend(dt)
        self._update_recenter(dt, recenter)

        self._follow(dt, pos)
        self._place(dt)

        self._trauma = max(0.0, self._trauma - dt / SHAKE_DECAY)
        self._shake_time += dt

    def _flush_mouse(self, dt):
        """Pay out the banked mouse delta over `mouse_smoothing` seconds."""
        dx, dy = self._pending_mouse
        if dx == 0.0 and dy == 0.0:
            return
        share = 1.0 if self.mouse_smoothing <= 0.0 \
            else min(1.0, dt / self.mouse_smoothing)
        self._pending_mouse = [dx * (1.0 - share), dy * (1.0 - share)]
        scale = self.mouse_sensitivity / 100.0
        self.look(dx * share * scale, dy * share * scale)

    def _update_aim_blend(self, dt):
        target = self._aim_target
        smooth = AIM_IN_SMOOTH if target > self._aim else AIM_OUT_SMOOTH
        # Eased on top of a fixed-rate approach: the rate decides how long the
        # move takes -- a fixed one, so half pressing a trigger and letting go
        # costs half the time rather than the same time -- and the ease decides
        # its shape.  The pair is what makes raising the sights read as a
        # movement rather than as a value changing.
        self._aim_raw = _approach(self._aim_raw, target, 1.0 / smooth, dt)
        self._aim = _ease(self._aim_raw)
        # The sights are a decision to look at one thing; carry the pitch
        # limits with them so the view is not fenced in on the way.
        self.pitch = self._clamp_pitch(self.pitch)

    def _clamp_pitch(self, pitch):
        low = PITCH_MIN + (AIM_PITCH_MIN - PITCH_MIN) * self._aim
        high = PITCH_MAX + (AIM_PITCH_MAX - PITCH_MAX) * self._aim
        return _clamp(pitch, low, high)

    def _update_recenter(self, dt, held):
        """Spring onto his back, once per press rather than while held."""
        if held and not self._recenter_held:
            self._recentring = True
            self._recenter_vel = 0.0
            self._recenter_pitch_vel = 0.0
        self._recenter_held = held
        if not self._recentring:
            return

        target = float(s16(self.target.face_angle[1] + 0x8000))
        self.yaw, self._recenter_vel = _smooth_damp_angle(
            self.yaw, target, self._recenter_vel, RECENTER_SMOOTH, dt)
        self.pitch, self._recenter_pitch_vel = smooth_damp(
            self.pitch, DEFAULT_PITCH, self._recenter_pitch_vel,
            RECENTER_SMOOTH, dt)
        # Done when it is close enough that the rest would not be seen.  Held
        # keys keep re-arming it, which is what makes holding R hold the view
        # behind him.
        if abs(_wrap_angle(target - self.yaw)) < 0x0040 and not held:
            self._recentring = False

    def _follow(self, dt, pos):
        """Move the pivot toward chest height on him."""
        target_y = pos[1] + self.height
        if not self._initialised:
            self._pivot = [pos[0], target_y, pos[2]]
            self._pivot_vel = [0.0, 0.0, 0.0]
            self._initialised = True
            return

        for axis in (0, 2):
            self._pivot[axis], self._pivot_vel[axis] = smooth_damp(
                self._pivot[axis], pos[axis], self._pivot_vel[axis],
                self.follow_smooth, dt)

        floor = getattr(self.target, "floor_height", None)
        airborne = floor is None or (pos[1] - floor) > AIRBORNE_HEIGHT
        band = AIR_DEADZONE if airborne else GROUND_DEADZONE
        smooth = AIR_SMOOTH if airborne else GROUND_SMOOTH

        # Inside the band the camera does not answer at all; outside it, it
        # chases the edge of the band rather than him, so what is left when it
        # arrives is the band itself and not a rebound.
        gap = target_y - self._pivot[1]
        if abs(gap) > band:
            goal = target_y - math.copysign(band, gap)
        else:
            goal = self._pivot[1]
        self._pivot[1], self._pivot_vel[1] = smooth_damp(
            self._pivot[1], goal, self._pivot_vel[1], smooth, dt)

        # The leash.  Nothing above outruns a jetpack.
        lag = target_y - self._pivot[1]
        if abs(lag) > MAX_VERTICAL_LAG:
            self._pivot[1] = target_y - math.copysign(MAX_VERTICAL_LAG, lag)
            self._pivot_vel[1] = 0.0

    def _place(self, dt):
        """Work out where the camera goes, and put it there."""
        speed = abs(getattr(self.target, "forward_vel", 0.0))
        self._speed_blend, self._speed_vel = smooth_damp(
            self._speed_blend, min(speed / SPEED_REFERENCE, 1.0),
            self._speed_vel, SPEED_SMOOTH, dt)
        # A sprint only widens the hip view.  Down the sights the framing is
        # the point of the mode and nothing else is allowed to move it.
        hip = 1.0 - self._aim

        forward = self.forward
        right = self.right

        length = (self.distance
                  + (self.aim_distance - self.distance) * self._aim
                  + SPEED_DOLLY * self._speed_blend * hip)
        side = self.shoulder + (self.aim_shoulder - self.shoulder) * self._aim
        lift = HIP_SHOULDER[1] + (AIM_SHOULDER[1] - HIP_SHOULDER[1]) * self._aim

        # The boom root: the pivot, shifted along the camera's own axes.  Up is
        # world up rather than the camera's, so looking down does not slide the
        # framing sideways up the screen.
        #
        # The offset is folded away if it has put the root inside something,
        # which happens when he is flat against a wall on the shoulder side.
        # That is a *lateral* move and so it does shift the aim, which is why
        # it is done here, separately and rarely, rather than by folding the
        # shoulder into the boom: everything after this point runs along the
        # view ray, where sliding in and out costs the aim nothing at all.
        self._fold_shoulder(dt, right, side, lift)
        fold = self._shoulder_fold
        root = [
            self._pivot[0] + right[0] * side * fold,
            self._pivot[1] + lift * fold,
            self._pivot[2] + right[2] * side * fold,
        ]

        clear = self._clear_distance(root, forward, length)
        if clear < self._boom:
            # In, on the frame it is needed.  A spring here is a camera that
            # spends the first hundred milliseconds of every corner inside it.
            self._boom = clear
            self._boom_vel = 0.0
        else:
            self._boom, self._boom_vel = smooth_damp(
                self._boom, clear, self._boom_vel, BOOM_RETURN, dt)

        self.pos = self._safety_net(
            [root[i] - forward[i] * self._boom for i in range(3)])
        # The point on the view ray level with him: what the old camera called
        # its focus, and still what anything measuring the aim starts from.
        self.focus = [self.pos[i] + forward[i] * self._boom for i in range(3)]
        self._update_fov(hip)

    def _fold_shoulder(self, dt, right, side, lift):
        """How much of the shoulder offset there is room for, 0 to 1.

        Only ever asked because of the one case the boom cannot answer: he is
        pressed against a wall on the side the camera is offset toward, so the
        offset alone -- before the boom has gone anywhere -- is already inside
        it.  Backing the camera up would not help, since the whole line it
        would back along is in the wall.

        Smoothed in both directions, unlike the boom.  The boom is allowed to
        snap in because moving along the view ray is invisible; this is not,
        and a hard fold reads as the world sliding sideways.
        """
        wanted = 1.0
        if side or lift:
            for fraction in (1.0, 0.6, 0.3, 0.0):
                point = (self._pivot[0] + right[0] * side * fraction,
                         self._pivot[1] + lift * fraction,
                         self._pivot[2] + right[2] * side * fraction)
                if not self._occupied(*point):
                    wanted = fraction
                    break
            else:
                wanted = 0.0
        self._shoulder_fold, self._fold_vel = smooth_damp(
            self._shoulder_fold, wanted, self._fold_vel, FOLD_SMOOTH, dt)

    def _update_fov(self, hip):
        self.fov = (self.base_fov
                    + (self.aim_fov - self.base_fov) * self._aim
                    + SPEED_FOV_KICK * self._speed_blend * hip)

    def _clear_distance(self, root, forward, full):
        """How far back from `root` the camera can go before it hits something.

        Backwards along the view direction, which is the whole point: the
        camera is then somewhere on the line it is looking along, and the
        crosshair means the same thing at every distance the march might come
        back with.

        Marched rather than solved: the collision here is a soup of triangles
        in a lateral grid with no ray query on it, and the three tests it does
        have -- push a sphere out of the walls, find the floor under a point,
        find the ceiling over it -- are exactly the three a camera cares about.
        Sampling them along the segment costs about a dozen queries a frame,
        which is a fifth of what the squad reticle spends while it is up.

        Shortening the boom is the *only* answer to anything in the way, and
        that includes the ground.  It looks like an over-reaction -- a hillside
        rising behind him is something the camera could plainly see over from a
        few feet up, and lifting it would keep the distance -- but the lift is
        the one move a camera in an aimed game may not make.  Everything the
        crosshair means comes from the camera sitting *on* the aim ray: sliding
        in and out along that ray leaves the ray, and so the aim, exactly where
        it was, while lifting off it drags the aim point across the world with
        the terrain, and not slightly.  A hundred units of lift with the view
        angled eight degrees down moves the point under the crosshair seven
        hundred units further away; at five degrees it is eleven hundred.  A
        camera that crowds his shoulder on a slope is a camera doing its job.
        A camera that walks your aim off target every time you cross a hill is
        not.
        """
        samples = int(min(max(full / OCCLUSION_STEP, 3.0), OCCLUSION_MAX_SAMPLES))
        step = full / samples
        previous = 0.0
        for i in range(1, samples + 1):
            distance = step * i
            point = (root[0] - forward[0] * distance,
                     root[1] - forward[1] * distance,
                     root[2] - forward[2] * distance)
            if self._occupied(*point):
                # The march only tells us that the boundary is somewhere in
                # this step.  Snapping to its beginning used to quantise the
                # boom in ~110-unit jumps as the player moved beside a wall.
                # Refine just this last interval; five passes leave less than
                # four units of uncertainty at the largest normal step.
                clear, blocked = previous, distance
                for _ in range(OCCLUSION_REFINE_STEPS):
                    middle = (clear + blocked) * 0.5
                    middle_point = (
                        root[0] - forward[0] * middle,
                        root[1] - forward[1] * middle,
                        root[2] - forward[2] * middle,
                    )
                    if self._occupied(*middle_point):
                        blocked = middle
                    else:
                        clear = middle
                # Stay a little inside the known-clear side.  The collision
                # queries are discrete too, so landing exactly on their edge
                # can flicker from one frame to the next.
                return max(clear - 1.0, MIN_DISTANCE)
            previous = distance
        return full

    def _occupied(self, x, y, z):
        """Is a camera-sized sphere here inside the level?"""
        data = WallCollisionData(x, y, z, 0.0, CAMERA_RADIUS)
        if self.surfaces.find_wall_collisions(data, for_camera=True):
            return True

        height, floor = self.surfaces.find_floor(
            x, y + CAMERA_RADIUS, z, for_camera=True)
        if floor is not None and y < height + FLOOR_CLEARANCE:
            return True

        height, ceil = self.surfaces.find_ceil(
            x, y - CAMERA_RADIUS, z, for_camera=True)
        return ceil is not None and y > height - CEILING_CLEARANCE

    def _safety_net(self, pos):
        """Last resort: keep the camera out of what it has ended up inside.

        The march is what actually keeps the camera clear, and over open ground
        this never fires.  It is here for what the march cannot cover -- a
        surface that appears between the pivot and the camera without ever
        crossing the segment, which the level's moving platforms can do -- and
        it is deliberately a hard clamp, because a smoothed correction to being
        underground is a smooth trip underground.

        It is also the one place the camera moves off the aim ray, so the
        clearances are as small as they can be: better a frame of grazing the
        ground than a visible push, since a push here is the aim moving without
        the player having asked.
        """
        x, y, z = pos
        height, ceil = self.surfaces.find_ceil(x, y, z, for_camera=True)
        if ceil is not None and y > height - 30.0:
            y = height - 30.0
        height, floor = self.surfaces.find_floor(x, y, z, for_camera=True)
        if floor is not None and y < height + 20.0:
            y = height + 20.0
        return [x, y, z]

    # -- what the rest of the game reads --------------------------------------

    @property
    def view_yaw(self):
        """The bearing the camera looks along, as a float binary angle."""
        return _wrap_angle(self.yaw + 0x8000)

    @property
    def forward(self):
        """The unit vector out of the middle of the screen."""
        flat = math.cos(self.pitch)
        view = self.view_yaw
        return (sins_f(view) * flat, -math.sin(self.pitch), coss_f(view) * flat)

    @property
    def right(self):
        """The unit vector toward the right of the screen, level with it."""
        view = self.view_yaw
        return (-coss_f(view), 0.0, sins_f(view))

    def aim_ray(self):
        """The line out of the middle of the screen: (origin, direction).

        The one thing a crosshair means.  It comes off the camera's own angles
        rather than from subtracting two placed points, so it is exact even on
        the frame the boom is being shoved through a wall.
        """
        return tuple(self.pos), self.forward

    @property
    def mario_yaw(self):
        """The yaw the analog stick should be interpreted relative to.

        Stick-up must send him away from the camera, which is along the view.

        Rounded to a whole binary angle even though the camera tracks a float
        one: this feeds the movement code, which is meant to see the same
        quantised angle the original does.
        """
        return s16(round(self.view_yaw))

    def apply_to(self, node, lens=None):
        """Point a Panda3D camera node along the view, and set its lens."""
        node.set_pos(*to_panda(*self.pos))
        forward = self.forward
        node.look_at(*to_panda(self.pos[0] + forward[0] * 1000.0,
                               self.pos[1] + forward[1] * 1000.0,
                               self.pos[2] + forward[2] * 1000.0))

        if self._trauma > 0.0 and self.shake_scale > 0.0:
            # Trauma squared, so a small knock is much smaller than a large one
            # rather than proportionally smaller -- which is what stops every
            # footfall-sized event from registering as a shake.
            amount = self._trauma * self._trauma * self.shake_scale
            f1, f2, f3 = SHAKE_FREQUENCIES
            t = self._shake_time
            node.set_hpr(
                node.get_h() + math.sin(t * f1) * SHAKE_YAW * amount,
                node.get_p() + math.sin(t * f2 + 1.7) * SHAKE_PITCH * amount,
                node.get_r() + math.sin(t * f3 + 3.1) * SHAKE_ROLL * amount,
            )

        # Only when it has actually moved: assigning to a lens rebuilds its
        # projection matrix, and the field of view holds still for most of the
        # game's running time.
        if lens is not None and abs(lens.get_hfov() - self.fov) > 0.01:
            lens.set_fov(self.fov)
