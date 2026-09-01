use crate::{
    console::GameTuning,
    gravity::{self, Gravity},
    input::InputState,
    level::LevelData,
    player::{Player, RenderPose},
    GameState,
};
use bevy::prelude::*;

#[derive(Component)]
pub struct FollowCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// How much of the boom the level currently leaves free, from 0 at the
    /// player's head to 1 at the full [`Self::distance`]. Kept between frames
    /// because it is eased rather than applied raw -- see [`update`].
    pub clearance: f32,
    /// The frame [`Self::yaw`] and [`Self::pitch`] are measured in.
    ///
    /// Identity on a flat level, where the orbit is about world `+Y` and always
    /// was. On a planet it is carried along as the player walks: each frame it
    /// is turned by the smallest rotation that takes its own up onto the local
    /// one, which is parallel transport and is the only part of this that is
    /// not obvious.
    ///
    /// The obvious thing -- rebuilding the frame from scratch out of the local
    /// up every frame -- does not work. `Quat::from_rotation_arc(Vec3::Y, up)`
    /// has no answer at the antipode of `+Y` and an arbitrary one near it, so a
    /// player walking towards the planet's south pole would find the view
    /// spinning faster and faster and then flipping over. Turning by the small
    /// step from *last* frame's up to this one never asks that question,
    /// because between two frames the up has barely moved.
    pub frame: Quat,
    /// The frame the view is actually built in, chasing [`Self::frame`].
    ///
    /// Two frames rather than one, which is how the Outer Wilds prototype in
    /// `experimental/ow` is arranged and the reason walking a planet there does
    /// not feel jerky: what the ground says and what the camera does are kept
    /// apart, with a rate between them. `frame` is the ground's answer, exact
    /// and updated every frame; `view` lags it by [`UP_ALIGN_RATE`].
    ///
    /// It is the *frame* that lags and not the whole orbit. Yaw and pitch are
    /// the player's own input and stay immediate -- easing those would be input
    /// latency, and the prototype avoids it the other way round, by running its
    /// camera filter four times faster on foot than in flight.
    pub view: Quat,
    /// The point the boom hangs off, easing onto the player's own.
    ///
    /// The smoothing lives here rather than on the camera's position, and that
    /// is the difference between a camera that follows and one that swings.
    /// Lerping the *camera* towards where it wanted to be was fine only while
    /// it was also pointed at the player: turning the view moves the wanted
    /// position the whole width of the orbit, and a camera crawling after that
    /// at [`GameTuning::cam_smooth`] a frame is a camera the player slides
    /// across the screen of on every mouse movement and slides back on when it
    /// stops. Smoothing the focus filters the only thing that ever needed
    /// filtering -- the player's own steps and bumps -- and leaves the camera
    /// rigid about it, so the boom, the picture and the crosshair never
    /// disagree.
    ///
    /// `None` until the first frame places it, so the camera arrives already
    /// framed rather than flying in from the origin.
    pub focus: Option<Vec3>,
    /// Whether the last frame was flown weightless. In space the look is
    /// free -- see [`update`]'s first branch -- and this is the edge that
    /// says a fold or a landing is happening this frame rather than every
    /// frame.
    pub free: bool,
    /// Seconds of grace after a landing during which a large gap between the
    /// view and the ground's frame is *eased* across rather than snapped.
    /// Coming back from free flight upside down is exactly such a gap, and
    /// it is the one kind that must roll the horizon over visibly -- the
    /// player flew themselves into that attitude and the camera showing the
    /// recovery is the camera being honest. See [`UP_SNAP`], which this
    /// suspends.
    pub landing: f32,
}

impl Default for FollowCamera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.2,
            distance: 9.5,
            // Starts fully extended: the first frame eases in from wherever the
            // level actually leaves room, rather than out from the player's
            // head.
            clearance: 1.0,
            frame: Quat::IDENTITY,
            view: Quat::IDENTITY,
            focus: None,
            free: false,
            landing: 0.0,
        }
    }
}

/// Pitch range, in radians. Short of straight up and straight down: the boom
/// passing through either pole gives the player no ground to read.
const PITCH_LIMITS: (f32, f32) = (-0.75, 0.85);

/// The gap left between the camera and a wall it has been pulled in front of,
/// so the near plane never ends up inside the geometry.
const WALL_GAP: f32 = 0.3;

/// The rate the boom is allowed back out at once a wall stops blocking it,
/// as a fraction of the remaining gap per sixtieth of a second.
const REOPEN_RATE: f32 = 0.08;

/// How far the boom sits above the focus, as a fraction of its own length.
///
/// A slope rather than the flat 0.8 units it was, and the difference is the
/// whole of the aim-down-sights drift. A fixed rise is a *changing angle* as
/// the boom shortens: 4.8 degrees below the horizon at [`GameTuning::cam_distance`]
/// and 8.0 at `cam_aim_distance`, so raising the weapon walked the crosshair
/// three degrees down the screen over the couple of hundred milliseconds the
/// distance took to ease -- a metre of drop at twenty metres, arriving after
/// the shot was lined up. Held as a fraction, the angle is the same at every
/// distance and the zoom moves the camera without moving the aim.
///
/// The number is what the old offset worked out to at the default distance, so
/// nothing about the framing changes where the camera normally sits.
const BOOM_RISE: f32 = 0.8 / 9.5;

/// What the look sensitivity is multiplied by while the weapon is up.
///
/// A mouse reports whole counts, so the smallest turn a player can ask for is
/// one count -- [`GameTuning::mouse_sens`] radians -- and nothing smaller
/// exists at any frame rate, resolution or DPI. That is the floor on how
/// finely anything can be aimed. At the default it is 0.086 degrees, which at
/// this game's sixty-degree field of view over a 1080-tall picture is about a
/// pixel and a half of crosshair.
///
/// Halved while aiming, because the two things the stick is being asked to do
/// are different jobs with opposite wants: turning round wants a whole circle
/// in a hand's width, and landing the shot on an ant at thirty metres wants
/// the smallest step to be smaller than the ant. Sensitivity that suits one
/// cannot suit the other, and which of them the player is doing is already
/// known -- they are holding the aim button.
const AIM_SENSITIVITY: f32 = 0.5;

/// The most the focus may trail behind the player, in metres.
///
/// The focus ease is a fraction of the remaining gap per frame, and a
/// fraction of a gap that grows without bound is a lag that does too. At
/// walking speed the trail is well under a metre and the cap never touches
/// it; at the speeds the jetpack reaches between planets it was tens of
/// metres -- far enough to drag the focus underground on lift-off, where the
/// boom probe read the terrain overhead as a wall and pulled the camera all
/// the way in to the player's head. First person, uninvited, at exactly the
/// moment there was a planet to look back at. Capped, the ease keeps its feel
/// at the speeds it was tuned for and turns into a rigid follow past them.
const FOCUS_TRAIL: f32 = 2.5;

/// How fast the view's idea of up chases the ground's, per second.
///
/// A ninth of a second of lag. The prototype in `experimental/ow` uses 20 on
/// foot and 2.25 in flight; this sits nearer the walking end because there is
/// no flight mode here to serve, and because it is not the only filter in the
/// chain -- the body's own levelling in [`crate::player`] runs slower again,
/// and what reaches the screen is the two of them in series.
const UP_ALIGN_RATE: f32 = 9.0;

/// How long after leaving free flight a large view-to-ground gap is still
/// eased rather than snapped. Long enough for [`UP_ALIGN_RATE`] to close even
/// an upside-down arrival; a respawn inside this window is eased too, which
/// is rare enough not to matter.
const LANDING_EASE: f32 = 1.2;

/// Past this much of an angle between the two ups, the gap is not something to
/// ease across.
///
/// Walking never opens one: on a planet a few hundred metres across, running
/// flat out turns up by a couple of degrees a second, and the rate above holds
/// the error well under one. What does open one is arriving somewhere else
/// entirely -- a respawn, a warp pipe, the far side of the planet -- and easing
/// across that is a second of the horizon rolling over for no reason.
const UP_SNAP: f32 = 0.5;

/// Reshapes a per-frame blend factor to cover `delta` seconds instead.
///
/// `camera.lerp(wanted, k)` run once a frame is not a rate: it is `k` per
/// *frame*, so the same `k` settles in half the wall-clock time at twice the
/// frame rate. That is fine while the frame rate never changes and stops being
/// fine the moment it does -- going fullscreen was enough to make a camera
/// tuned at 60 snap noticeably harder, without a line of the camera changing.
///
/// Each frame keeps `1 - k` of the distance still to cover, so `n` frames keep
/// `(1 - k)^n`. Feeding in how many sixtieths this frame actually lasted makes
/// the same number mean what it meant at 60fps and hold there at any rate.
fn blend(per_frame: f32, delta: f32) -> f32 {
    1.0 - (1.0 - per_frame.clamp(0.0, 1.0)).powf(delta * 60.0)
}

/// The look stick's shape. Squaring the deflection keeps the first half of the
/// stick's travel slow enough to aim with while the far end still whips the
/// view around, which a linear stick cannot do at any single sensitivity.
fn stick_curve(deflection: f32) -> f32 {
    deflection * deflection.abs()
}

#[allow(clippy::too_many_arguments)]
pub fn update(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut input: ResMut<InputState>,
    mut cameras: Query<(&mut Transform, &mut FollowCamera), Without<Player>>,
    player: Res<RenderPose>,
    level: Res<LevelData>,
    gravity: Res<Gravity>,
    mut state: ResMut<GameState>,
    tuning: Res<GameTuning>,
) {
    let Ok((mut camera, mut follow)) = cameras.single_mut() else {
        return;
    };
    // `input.aim` rather than `state.aiming`, which is this system's own
    // output and is not written until further down.
    let sensitivity = tuning.mouse_sens * if input.aim { AIM_SENSITIVITY } else { 1.0 };
    // The stick is a rate rather than a displacement, so it is scaled by the
    // frame's length where the mouse is not.
    let stick = time.delta_secs() * tuning.pad_look;
    let up = gravity.up(player.translation);
    let free = gravity.weightless(player.translation);
    if free {
        // Weightless, there is no floor and so no reason the look should
        // have one. The yaw/pitch orbit cannot give that -- its pitch clamp
        // is what keeps a grounded camera off the poles of its own frame --
        // so out here the orbit is folded into the view once, and the mouse
        // then turns the *view itself* about its own axes: every turn is
        // relative to the screen, straight up keeps going, and there is no
        // pole because nothing is fixed enough to have one.
        if !follow.free {
            follow.view = follow.view
                * Quat::from_rotation_y(follow.yaw)
                * Quat::from_rotation_x(follow.pitch);
            follow.yaw = 0.0;
            follow.pitch = 0.0;
        }
        let mut turn = -input.look_mouse.x * sensitivity - stick_curve(input.look_stick.x) * stick;
        if keys.pressed(KeyCode::KeyQ) {
            turn += 0.035;
        }
        if keys.pressed(KeyCode::KeyE) {
            turn -= 0.035;
        }
        let tilt = -input.look_mouse.y * sensitivity * 0.8333
            + stick_curve(input.look_stick.y) * stick * 0.8333;
        follow.view = follow.view * Quat::from_rotation_y(turn) * Quat::from_rotation_x(tilt);
        // The frame follows the view rather than the ground: the ground is
        // whichever planet happens to be nearest, and a free camera answering
        // to it would lurch every time the crossing's midpoint went by.
        follow.frame = follow.view;
    } else {
        // Carried onto this frame's up before anything is measured in it. On
        // the castle `up` is `+Y`, the correction is the identity, and every
        // line below is the line that was there before.
        follow.frame = Quat::from_rotation_arc(follow.frame * Vec3::Y, up) * follow.frame;
        // And the view eases onto the frame. That is the whole of the lag, and
        // every axis below is measured in `view` rather than `frame`, so the
        // camera answers the ground through a filter while still answering the
        // mouse directly.
        if follow.free {
            follow.landing = LANDING_EASE;
        }
        follow.landing = (follow.landing - time.delta_secs()).max(0.0);
        let carried = follow.view * Vec3::Y;
        follow.view = if carried.angle_between(up) > UP_SNAP && follow.landing <= 0.0 {
            follow.frame
        } else {
            let rate = gravity::settle(UP_ALIGN_RATE, time.delta_secs());
            follow.view.slerp(follow.frame, rate)
        };
        follow.yaw -= input.look_mouse.x * sensitivity;
        follow.pitch = (follow.pitch - input.look_mouse.y * sensitivity * 0.8333)
            .clamp(PITCH_LIMITS.0, PITCH_LIMITS.1);
        follow.yaw -= stick_curve(input.look_stick.x) * stick;
        follow.pitch = (follow.pitch + stick_curve(input.look_stick.y) * stick * 0.8333)
            .clamp(PITCH_LIMITS.0, PITCH_LIMITS.1);
        if keys.pressed(KeyCode::KeyQ) {
            follow.yaw += 0.035;
        }
        if keys.pressed(KeyCode::KeyE) {
            follow.yaw -= 0.035;
        }
        if InputState::take(&mut input.recenter) {
            // Behind the player means behind him *in the camera's own frame*,
            // which is what taking the facing back out of that frame asks.
            let forward = follow.view.inverse() * (player.rotation * Vec3::NEG_Z);
            follow.yaw = forward.x.atan2(forward.z);
        }
    }
    follow.free = free;
    // Roll is the axis this matters most on. Taking the camera's up straight
    // off the ground puts every wobble in the surface onto the horizon, and a
    // tilting horizon is the most legible motion there is on a screen -- far
    // more so than the same wobble in pitch. Which is why `up` is not read
    // again below this line.
    let view_up = follow.view * Vec3::Y;
    state.aiming = input.aim;
    let desired_distance = if state.aiming {
        tuning.cam_aim_distance
    } else {
        tuning.cam_distance
    };
    let smoothing = blend(tuning.cam_smooth, time.delta_secs());
    follow.distance += (desired_distance - follow.distance) * blend(0.16, time.delta_secs());
    // The focus eases; the camera does not. On the first frame there is
    // nothing to ease from, so it starts where it belongs.
    let standing = player.translation + view_up * tuning.cam_height;
    let focus = match follow.focus {
        Some(previous) => previous.lerp(standing, smoothing),
        None => standing,
    };
    // However fast the player is going, the focus stays within arm's reach of
    // him. See [`FOCUS_TRAIL`]: past the cap the ease has nothing left to
    // smooth, it is simply behind.
    let focus = standing + (focus - standing).clamp_length_max(FOCUS_TRAIL);
    follow.focus = Some(focus);
    let orbit =
        follow.view * Quat::from_rotation_y(follow.yaw) * Quat::from_rotation_x(follow.pitch);
    let boom = orbit * Vec3::new(0.0, BOOM_RISE * follow.distance, follow.distance);

    // How much of the boom is free this frame, measured along the boom rather
    // than as a position, so it can be compared with the last frame's answer.
    let reach = match level.segment_hit(focus, focus + boom) {
        Some(hit) => ((hit - focus).length() - WALL_GAP).max(0.0) / boom.length().max(f32::EPSILON),
        None => 1.0,
    };
    // The two directions are deliberately not symmetric. Coming in has to be
    // immediate: the wall is between the player and the camera *now*, and any
    // easing there is a frame spent looking at the inside of it. Going back out
    // has no such deadline, and easing it is the whole point -- a wall the boom
    // clears and re-catches from one frame to the next, which is what a pillar
    // or a doorway does as the player walks past, otherwise throws the camera
    // its whole length out and back. That is the jump: not the frame rate, the
    // position, exactly as far as the boom is long.
    follow.clearance = if reach < follow.clearance {
        reach
    } else {
        follow.clearance + (reach - follow.clearance) * blend(REOPEN_RATE, time.delta_secs())
    };
    camera.translation = focus + boom * follow.clearance;
    // **Aimed along the boom, not at the focus.** With the camera placed
    // rigidly at `focus + boom * clearance` the two are the same direction
    // anyway, and saying it this way makes it stay that way: nothing about
    // where the camera has got to can reach the rotation, and `aim::drive`
    // takes the shot's direction straight off the rotation. What used to reach
    // the crosshair, back when the camera was lerped into place and pointed at
    // the focus from wherever it had got to:
    //
    // * Stopping. The camera trailed the player by roughly its speed times the
    //   smoothing, glided that distance forward over the next tenth of a
    //   second, and swung the aim through the whole of it on the way. That is
    //   the "camera settling" -- it was not the position anyone was watching,
    //   it was the crosshair.
    // * Walking onto a slope. The focus rides the player, so the ground's
    //   height goes into it; with the camera lagging, a step up or down tilted
    //   the line between the two and moved the shot.
    //
    // Both are gone, and the smoothing they came from now lives on the focus,
    // where it costs the player a little drift off centre and costs the aim
    // nothing at all.
    camera.look_to(-boom, view_up);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::LevelData;
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    const PLANET: f32 = 300.0;

    /// Everything [`update`] reads, with no renderer and no window: a bare
    /// planet with no collision in it, so the boom is never cut short and the
    /// only thing under test is which way the view thinks up is.
    fn planet_world(at: Vec3) -> World {
        let mut world = World::new();
        world.insert_resource(Gravity::towards(Vec3::ZERO));
        world.insert_resource(LevelData::planet(&[], &[], Vec3::ZERO, PLANET, None));
        world.insert_resource(RenderPose {
            translation: at,
            rotation: Quat::IDENTITY,
        });
        world.insert_resource(InputState::default());
        world.insert_resource(GameState::default());
        world.insert_resource(GameTuning::default());
        world.insert_resource(ButtonInput::<KeyCode>::default());
        world.init_resource::<Time>();
        let aligned = Quat::from_rotation_arc(Vec3::Y, at.normalize());
        world.spawn((
            Transform::default(),
            FollowCamera {
                frame: aligned,
                view: aligned,
                ..default()
            },
        ));
        world
    }

    /// Advances the clock as well as the systems: the lag is a rate, so a
    /// frame of no elapsed time is a frame in which nothing eases.
    fn frames(world: &mut World, count: usize) {
        for _ in 0..count {
            world
                .resource_mut::<Time>()
                .advance_by(Duration::from_nanos(16_666_667));
            world.run_system_once(update).expect("update could not run");
        }
    }

    fn view_up(world: &mut World) -> Vec3 {
        let mut query = world.query::<&FollowCamera>();
        let follow = query.single(world).unwrap();
        follow.view * Vec3::Y
    }

    /// The point of the whole exercise: the ground moves and the view arrives
    /// afterwards. A step of walking is a fraction of the gap, not the gap.
    #[test]
    fn the_view_follows_the_ground_late_and_then_arrives() {
        let mut world = planet_world(Vec3::X * PLANET);
        // A sixth of a radian round the planet -- more than any single frame of
        // running covers, and well inside the distance that counts as walking.
        let moved = (Vec3::X * 0.986 + Vec3::Z * 0.166).normalize() * PLANET;
        world.resource_mut::<RenderPose>().translation = moved;
        let wanted = moved.normalize();
        frames(&mut world, 1);
        let after_one = view_up(&mut world).angle_between(wanted);
        assert!(after_one > 0.1, "arrived in a single frame");
        assert!(
            after_one < 0.166,
            "did not set off at all: {after_one} rad left of 0.166"
        );
        frames(&mut world, 30);
        assert!(
            view_up(&mut world).angle_between(wanted) < 0.01,
            "half a second later the view still has not caught up"
        );
    }

    /// And a gap that is not walking is not eased across. Respawning on the far
    /// side of a planet would otherwise spend a second rolling the horizon.
    #[test]
    fn arriving_somewhere_else_entirely_snaps_the_view() {
        let mut world = planet_world(Vec3::X * PLANET);
        world.resource_mut::<RenderPose>().translation = Vec3::NEG_Z * PLANET;
        frames(&mut world, 1);
        assert!(
            view_up(&mut world).angle_between(Vec3::NEG_Z) < 1e-4,
            "eased across a teleport"
        );
    }

    /// However fast the player goes, the focus stays within arm's reach.
    ///
    /// The ease is a fraction of the gap per frame, so before the cap its lag
    /// grew with speed without bound. At jetpack speeds it trailed tens of
    /// metres -- through the terrain on lift-off, where the boom probe read
    /// the ground as a wall between camera and player and collapsed the boom
    /// to zero: first person, uninvited. The cap is the fix, and this holds
    /// it: one enormous step, and the focus is still beside the player.
    #[test]
    fn the_focus_cannot_be_outrun() {
        let mut world = planet_world(Vec3::X * PLANET);
        frames(&mut world, 10);
        // Two hundred metres in one frame, which is faster than even the
        // infinite booster manages and exactly what it looks like to the
        // camera when it does.
        let bolted = Vec3::X * (PLANET + 200.0);
        world.resource_mut::<RenderPose>().translation = bolted;
        frames(&mut world, 1);
        let mut query = world.query::<&FollowCamera>();
        let follow = query.single(&world).unwrap();
        let standing = bolted + (follow.view * Vec3::Y) * world.resource::<GameTuning>().cam_height;
        let trail = (follow.focus.expect("the focus was never placed") - standing).length();
        assert!(
            trail <= FOCUS_TRAIL + 1e-3,
            "the focus trails {trail} m behind a fast player"
        );
    }

    /// In space the look has no floor and no ceiling: a held pitch input
    /// carries the view clean over the top and round again -- the full turn
    /// the grounded orbit's clamp exists to forbid, forbidden no longer.
    #[test]
    fn weightless_look_pitches_clean_over_the_pole() {
        let mut world = planet_world(Vec3::X * 2000.0);
        // A system whose midpoint the player floats at: genuinely weightless,
        // which is what frees the look.
        world.insert_resource(Gravity::binary(Vec3::ZERO, Vec3::X * 4000.0, 300.0));
        frames(&mut world, 1);
        let forward = |world: &mut World| {
            let mut query = world.query_filtered::<&Transform, With<FollowCamera>>();
            Vec3::from(query.single(world).unwrap().forward())
        };
        let start = forward(&mut world);
        // Pitch input adding up to one whole turn across 120 frames.
        let per_frame = std::f32::consts::TAU / 120.0;
        let counts = per_frame / (world.resource::<GameTuning>().mouse_sens * 0.8333);
        for _ in 0..60 {
            world.resource_mut::<InputState>().look_mouse = Vec2::new(0.0, counts);
            frames(&mut world, 1);
        }
        let halfway = forward(&mut world);
        assert!(
            halfway.dot(start) < -0.9,
            "half a turn of pitch left the view at {halfway:?}, which is a clamp"
        );
        for _ in 0..60 {
            world.resource_mut::<InputState>().look_mouse = Vec2::new(0.0, counts);
            frames(&mut world, 1);
        }
        world.resource_mut::<InputState>().look_mouse = Vec2::ZERO;
        assert!(
            forward(&mut world).dot(start) > 0.9,
            "a full turn of pitch did not come back round"
        );
    }

    /// The castle is not paying for any of this. Up never moves there, so the
    /// frame, the view and the horizon are `+Y` on every frame as before.
    #[test]
    fn a_flat_level_keeps_the_view_upright_throughout() {
        let mut world = planet_world(Vec3::Y * PLANET);
        world.insert_resource(Gravity::default());
        world.insert_resource(LevelData::planet(&[], &[], Vec3::ZERO, PLANET, None));
        world.resource_mut::<RenderPose>().translation = Vec3::new(-13.28, 3.0, 46.64);
        for _ in 0..60 {
            frames(&mut world, 1);
            let up = view_up(&mut world);
            assert!((up - Vec3::Y).length() < 1e-5, "the horizon tilted: {up}");
        }
    }

    /// A flat level with the player standing on it, which is what the two aim
    /// tests below want: gravity is `+Y` everywhere, so the view frame never
    /// moves and the only thing that could turn the camera is the camera.
    fn flat_world(at: Vec3) -> World {
        let mut world = planet_world(Vec3::Y * PLANET);
        world.insert_resource(Gravity::default());
        world.resource_mut::<RenderPose>().translation = at;
        let mut query = world.query::<&mut FollowCamera>();
        let mut follow = query.single_mut(&mut world).unwrap();
        follow.frame = Quat::IDENTITY;
        follow.view = Quat::IDENTITY;
        world
    }

    /// Where the shot goes. `aim::drive` takes it straight off the camera's
    /// rotation, so this is the crosshair.
    fn aim(world: &mut World) -> Vec3 {
        let mut query = world.query_filtered::<&Transform, With<FollowCamera>>();
        Vec3::from(query.single(world).unwrap().forward())
    }

    /// The crosshair belongs to the player's look input and to nothing else.
    ///
    /// Everything moved here is something that used to reach it through the
    /// camera's positional lag, because the camera was pointed *at the focus*
    /// from wherever it had got to rather than along the boom. Walking moved
    /// it, stopping moved it, and a step up onto a slope moved it -- all of
    /// them by the same mechanism, and all of them while the player was holding
    /// the mouse still.
    #[test]
    fn nothing_but_the_look_input_moves_the_aim() {
        let mut world = flat_world(Vec3::new(0.0, 3.0, 0.0));
        frames(&mut world, 120);
        let settled = aim(&mut world);
        for (what, moved) in [
            ("walking", Vec3::new(0.0, 3.0, 4.0)),
            ("stopping", Vec3::new(0.0, 3.0, 4.0)),
            ("stepping onto a slope", Vec3::new(0.0, 3.6, 4.0)),
            ("walking off a ledge", Vec3::new(0.0, 1.2, 4.0)),
        ] {
            world.resource_mut::<RenderPose>().translation = moved;
            frames(&mut world, 1);
            let now = aim(&mut world);
            assert!(
                now.angle_between(settled) < 1e-4,
                "{what} moved the crosshair by {} rad",
                now.angle_between(settled),
            );
        }
    }

    /// Turning the view does not throw the player around the screen.
    ///
    /// The regression this exists for: pointing the camera along the boom while
    /// still *lerping* it into place made the rotation instant and the position
    /// lagging, so a mouse movement swung the wanted position the whole width
    /// of the orbit and the camera crawled after it at `cam_smooth` a frame.
    /// The player slid off centre on every turn and slid back when it stopped,
    /// which reads as the camera being jumpy. Placed rigidly about a focus that
    /// does the easing instead, the picture and the boom agree on every frame,
    /// turning or not.
    #[test]
    fn turning_the_view_keeps_the_player_centred() {
        let mut world = flat_world(Vec3::new(0.0, 3.0, 0.0));
        frames(&mut world, 120);
        for frame in 0..60 {
            // A brisk mouse turn, held. In radians of yaw a frame rather than
            // mouse units, because a mouse unit is `mouse_sens` of a radian and
            // the point here is the turn rate: a twentieth of a radian a frame
            // is three a second, a flick rather than a tracking shot.
            let sensitivity = world.resource::<GameTuning>().mouse_sens;
            world.resource_mut::<InputState>().look_mouse = Vec2::new(0.05 / sensitivity, 0.0);
            frames(&mut world, 1);
            let focus = world.resource::<RenderPose>().translation
                + Vec3::Y * world.resource::<GameTuning>().cam_height;
            let mut query = world.query_filtered::<&Transform, With<FollowCamera>>();
            let camera = query.single(&world).unwrap();
            let to_player = (focus - camera.translation).normalize();
            let off = to_player.angle_between(Vec3::from(camera.forward()));
            // A thousandth of a radian is a twentieth of a degree, and about
            // three times the round-off `f32` accumulates over a minute of
            // yaw. The bug this catches was worth a tenth of a radian.
            assert!(
                off < 1e-3,
                "frame {frame} of turning put the player {off} rad off centre"
            );
        }
    }

    /// And raising the weapon does not either.
    ///
    /// The boom shortens from `cam_distance` to `cam_aim_distance` over a few
    /// tenths of a second. While its rise was a fixed 0.8 units that shortening
    /// was a three-degree pitch down, eased in under the player exactly as the
    /// shot was being lined up. See [`BOOM_RISE`].
    #[test]
    fn aiming_down_the_sights_does_not_tilt_the_aim() {
        let mut world = flat_world(Vec3::new(0.0, 3.0, 0.0));
        frames(&mut world, 120);
        let hip = aim(&mut world);
        world.resource_mut::<InputState>().aim = true;
        // Every frame of the zoom, not just the end of it: a drift that eases
        // in and eases back out again is still a drift to shoot through.
        for frame in 0..90 {
            frames(&mut world, 1);
            let now = aim(&mut world);
            assert!(
                now.angle_between(hip) < 1e-4,
                "frame {frame} of aiming in tilted the view by {} rad",
                now.angle_between(hip),
            );
        }
        let mut query = world.query::<&FollowCamera>();
        let distance = query.single(&world).unwrap().distance;
        let tuning = world.resource::<GameTuning>();
        assert!(
            (distance - tuning.cam_aim_distance).abs() < 0.1,
            "the boom never actually came in: {distance}",
        );
    }

    /// Holding the aim button makes every mouse count go half as far.
    ///
    /// The floor on precision is one mouse count, so this is the only lever
    /// that moves it without slowing the player down while they are just
    /// turning round. See [`AIM_SENSITIVITY`].
    #[test]
    fn aiming_halves_what_a_mouse_count_is_worth() {
        let turn = |aiming: bool| {
            let mut world = flat_world(Vec3::new(0.0, 3.0, 0.0));
            frames(&mut world, 120);
            let before = aim(&mut world);
            {
                let mut input = world.resource_mut::<InputState>();
                input.aim = aiming;
                input.look_mouse = Vec2::new(20.0, 0.0);
            }
            frames(&mut world, 1);
            aim(&mut world).angle_between(before)
        };
        let hip = turn(false);
        let aimed = turn(true);
        assert!(hip > 0.01, "twenty counts should turn the view: {hip} rad");
        assert!(
            (aimed - hip * AIM_SENSITIVITY).abs() < 1e-4,
            "aiming turned {aimed} rad where half of {hip} was wanted"
        );
    }

    /// One sixtieth of a second is the rate the tuning numbers were chosen at,
    /// so the reshaped factor has to leave them alone there.
    #[test]
    fn a_sixtieth_of_a_second_is_the_factor_unchanged() {
        for per_frame in [0.01, 0.16, 0.24, 0.5, 1.0] {
            let reshaped = blend(per_frame, 1.0 / 60.0);
            assert!(
                (reshaped - per_frame).abs() < 1e-5,
                "{per_frame} became {reshaped}"
            );
        }
    }

    /// The bug this exists to kill: the same camera settling different amounts
    /// in the same second because the frame rate changed under it. Going
    /// fullscreen is enough to do that.
    #[test]
    fn a_second_of_smoothing_is_a_second_at_any_frame_rate() {
        let settle = |rate: f32| {
            let mut remaining: f32 = 1.0;
            for _ in 0..rate as u32 {
                remaining *= 1.0 - blend(0.24, 1.0 / rate);
            }
            remaining
        };
        let at_60 = settle(60.0);
        for rate in [30.0, 75.0, 144.0, 240.0] {
            let there = settle(rate);
            assert!(
                (there - at_60).abs() < 1e-3,
                "a second at {rate}fps left {there} where 60fps left {at_60}"
            );
        }
        // And it is genuinely smoothing, not a factor that rounds to nothing.
        assert!(at_60 < 0.01, "a second at 0.24 should have all but arrived");
    }

    /// A wall cuts the boom short at once and lets it back out gradually. The
    /// asymmetry is what stops a pillar the player walks past from throwing the
    /// camera its whole length out and back between two frames.
    #[test]
    fn the_boom_gives_way_faster_than_it_returns() {
        let step = |clearance: f32, reach: f32| {
            if reach < clearance {
                reach
            } else {
                clearance + (reach - clearance) * blend(REOPEN_RATE, 1.0 / 60.0)
            }
        };
        assert_eq!(step(1.0, 0.3), 0.3, "a wall arriving takes effect at once");
        let reopened = step(0.3, 1.0);
        assert!(
            reopened > 0.3 && reopened < 0.4,
            "a wall clearing should ease out, not snap: {reopened}"
        );
    }
}
