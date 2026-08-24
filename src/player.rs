use crate::{
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    gravity::{self, Gravity},
    input::InputState,
    level::LevelData,
    weapon::Loadout,
    world::Respawn,
    ActiveCharacter, GameState,
};
use bevy::prelude::*;

pub const FIXED_DT: f32 = 1.0 / 30.0;

/// The player's collision capsule: radius, and height from feet to crown.
pub const PLAYER_RADIUS: f32 = 0.42;
pub const PLAYER_HEIGHT: f32 = 1.75;

/// How far above the feet a ceiling search starts. Anything nearer is the
/// floor being stood on rather than something to bump a head on.
const CEILING_CLEARANCE: f32 = 0.6;

/// Ground distance between footfalls, and seconds between swim strokes.
const STEP_DISTANCE: f32 = 3.0;
const STROKE_SECONDS: f32 = 0.6;

/// How far under the surface counts as being in the water at all. Above this
/// the feet are wet and nothing else changes.
const SUBMERGED_DEPTH: f32 = 0.15;

/// How far under the surface each character floats once they are in deep
/// water: Mario swimming, the Hero held up by it.
const SWIM_FLOAT_DEPTH: f32 = 0.45;
const WADE_FLOAT_DEPTH: f32 = 0.6;

/// How fast the surface pulls the wading Hero back to that depth, and how
/// fast whatever vertical speed he ran off the bank with bleeds away. The
/// original approaches 8 and 4 SM64 units per frame at 30 Hz, which at this
/// port's scale of 1/100 is 2.4 and 1.2 units a second.
const WADE_RISE: f32 = 2.4;
const WADE_SETTLE: f32 = 1.2;

/// Downward speed a landing needs before it is loud enough to be heard. Below
/// this the player is settling onto a slope, not landing on it.
const LANDING_IMPACT: f32 = 2.0;

/// How long the landing pose is held, and how long after a swing the next one
/// still counts as the second half of a combo.
const LAND_SECONDS: f32 = 0.25;
const COMBO_WINDOW: f32 = 1.2;

/// How fast the body swings upright onto the local down, per second.
///
/// A rate rather than an instant correction, and that is the single biggest
/// difference between this and the Outer Wilds prototype in `experimental/ow`.
/// Nothing there is ever *set* to face the ground; it is slewed towards it,
/// at `GROUND_ALIGN_RATE`, which is this same 8. A time constant of an eighth
/// of a second is short enough that standing still looks level and long enough
/// that a surface turning under the feet arrives as a lean rather than a jolt.
const UPRIGHT_RATE: f32 = 8.0;

/// How fast the feet close a gap to the floor beneath them, per second, and
/// the most daylight allowed under them while it closes.
///
/// Faster than the body rights itself: this one is a position, and the eye
/// calls a floating character wrong long before it calls a leaning one wrong.
/// The cap is what keeps the ease from reading as float -- running downhill
/// opens the gap as fast as the filter shuts it, and without a ceiling the two
/// find a balance several times higher than this.
const FOOT_SETTLE_RATE: f32 = 18.0;
const FOOT_SKIN: f32 = 0.12;

#[derive(Component)]
pub struct Player;

#[derive(Component, Clone, Copy)]
pub struct PreviousPose {
    translation: Vec3,
    rotation: Quat,
}

impl PreviousPose {
    pub fn new(transform: &Transform) -> Self {
        Self {
            translation: transform.translation,
            rotation: transform.rotation,
        }
    }
}

#[derive(Resource, Clone, Copy)]
pub struct RenderPose {
    pub translation: Vec3,
    pub rotation: Quat,
}

#[derive(Component)]
pub struct PlayerVisual;

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub enum Motion {
    Idle,
    Run,
    Jump,
    Fall,
    /// The moment of touching down, held briefly so the landing clip is seen
    /// rather than skipped straight past into the run.
    Land,
    Skate,
    Fly,
    Swim,
    Attack,
}

/// What the water is doing to whoever is standing in it.
///
/// Deep water is not one behaviour, because the two characters do not have the
/// same clips. Ported from `sm64py/hero/constants.py`, which says it plainly:
/// the Hero has no swimming animation, so rather than draw a walk cycle
/// underwater he is held at the surface and slowed to a wade. It is the one
/// place his move set is short of Mario's, and it is a gap in the source
/// animation rather than something the controller could paper over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submersion {
    /// Dry, or wet only to the ankles.
    Dry,
    /// Deep water, walked through upright and slowly.
    Wading,
    /// Deep water, swum properly. Mario only.
    Swimming,
}

impl Submersion {
    /// In deep water, however it is being handled.
    pub fn in_water(self) -> bool {
        self != Self::Dry
    }

    pub fn swimming(self) -> bool {
        self == Self::Swimming
    }
}

/// Which of the three a character is in.
///
/// Split out from the controller so the rule is checked without a level, a
/// water box or a renderer -- it is the part that decides whether the Hero
/// swims, which he must never do.
pub fn submersion(character: ActiveCharacter, depth: Option<f32>) -> Submersion {
    if !depth.is_some_and(|depth| depth > SUBMERGED_DEPTH) {
        return Submersion::Dry;
    }
    match character {
        ActiveCharacter::Hero => Submersion::Wading,
        ActiveCharacter::Mario => Submersion::Swimming,
    }
}

/// Moves `value` toward `target` by at most `step`, the way SM64's
/// `approach_f32` does: a linear pull rather than a spring, so it settles
/// without overshooting and ringing.
fn approach(value: f32, target: f32, step: f32) -> f32 {
    if value < target {
        (value + step).min(target)
    } else {
        (value - step).max(target)
    }
}

#[derive(Component)]
pub struct Controller {
    pub velocity: Vec3,
    pub grounded: bool,
    pub motion: Motion,
    pub attack_left: f32,
    pub invulnerable_left: f32,
    pub health: u8,
    pub submersion: Submersion,
    /// Distance run since the last footfall, and seconds since the last swim
    /// stroke. Both only drive audio cadence.
    step_phase: f32,
    stroke_phase: f32,
    /// Seconds the landing clip still owns the body.
    pub land_left: f32,
    /// Which swing of the combo the next attack is, and how long since the
    /// last one, so a held button reads as a combo and a pause resets it.
    pub combo: u8,
    since_attack: f32,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            grounded: false,
            motion: Motion::Fall,
            attack_left: 0.0,
            invulnerable_left: 0.0,
            health: 3,
            submersion: Submersion::Dry,
            step_phase: 0.0,
            stroke_phase: 0.0,
            land_left: 0.0,
            combo: 0,
            since_attack: COMBO_WINDOW,
        }
    }
}

impl Controller {
    /// Puts the body back to a standing start.
    ///
    /// Everything about *where* the player is goes; everything about who he is
    /// -- his health, the weapon in his hands, whether he is mid-combo -- stays,
    /// because arriving on a new level is a change of place and not a death.
    pub fn reset(&mut self) {
        self.velocity = Vec3::ZERO;
        self.grounded = false;
        self.motion = Motion::Fall;
        self.submersion = Submersion::Dry;
        self.step_phase = 0.0;
        self.stroke_phase = 0.0;
        self.land_left = 0.0;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn movement(
    mut input_state: ResMut<InputState>,
    level: Res<LevelData>,
    gravity: Res<Gravity>,
    respawn: Res<Respawn>,
    state: Res<GameState>,
    tuning: Res<GameTuning>,
    loadout: Res<Loadout>,
    mut sounds: ResMut<SoundQueue>,
    mut player: Query<(&mut Transform, &mut PreviousPose, &mut Controller), With<Player>>,
    camera: Query<&Transform, (With<Camera3d>, Without<Player>)>,
) {
    let Ok((mut transform, mut previous, mut ctrl)) = player.single_mut() else {
        return;
    };
    previous.translation = transform.translation;
    previous.rotation = transform.rotation;
    let Ok(cam) = camera.single() else {
        return;
    };
    // Which way up is, once, for the whole tick. Everything below that used to
    // read `.y` reads this instead: on the castle it is `+Y` and the arithmetic
    // is the arithmetic it always was, and on a planet it is the direction out
    // of the ground the player is standing on.
    let up = gravity.up(transform.translation);
    // Latched edges are consumed here rather than polled, so each press acts
    // on exactly one fixed step however many steps this frame runs.
    let jump_pressed = InputState::take(&mut input_state.jump);
    // The trigger belongs to whichever weapon is out. With a gun in hand the
    // edge is left for `weapon::fire` to take, so one press either swings or
    // shoots and never does both.
    let attack_pressed =
        !loadout.equipped.is_ranged() && InputState::take(&mut input_state.attack);
    let input = input_state.move_axis;
    // `forward`/`right` hand back a `Direction3d` rather than a `Vec3`: a
    // vector the type system knows is unit length. Flattening one onto the
    // ground plane is exactly the operation that stops it being unit length,
    // so it is taken back to a plain `Vec3` first and renormalised after.
    // Flattening used to mean zeroing `y`; it now means dropping whatever part
    // of the vector points at the local sky.
    let forward = gravity::flatten(cam.forward().into(), up);
    let right = gravity::flatten(cam.right().into(), up);
    let wish = forward * input.y + right * input.x;
    // How far under the surface he is, measured along his own up, so the sea
    // wrapped round a planet asks the same question the castle's moat does.
    let depth = level.water_depth(transform.translation);
    let was_wet = ctrl.submersion.in_water();
    ctrl.submersion = submersion(state.active, depth);
    if ctrl.submersion.in_water() != was_wet {
        sounds.push(Sfx::Splash);
    }
    let boost = input_state.boost && state.active == ActiveCharacter::Hero;
    let skate = boost && ctrl.grounded && !ctrl.submersion.in_water();
    let speed = match ctrl.submersion {
        Submersion::Swimming => tuning.mario_swim,
        Submersion::Wading => tuning.hero_wade,
        Submersion::Dry if skate => tuning.skate_speed,
        Submersion::Dry if state.active == ActiveCharacter::Hero => tuning.hero_speed,
        Submersion::Dry => tuning.mario_speed,
    };
    let accel = if skate {
        tuning.skate_accel
    } else {
        tuning.walk_accel
    };
    let target = wish * speed;
    // The two halves of the velocity: how fast the player is climbing away
    // from the ground, and how fast he is running along it. `rise` is carried
    // by hand from here to the bottom of the tick and put back into
    // `ctrl.velocity` once, because between the two there is a wall
    // resolution that changes the run and must not touch the climb.
    let (mut rise, horizontal) = gravity.split(ctrl.velocity, transform.translation);
    let next = if wish.length_squared() > 0.0 {
        horizontal.lerp(target, (accel * FIXED_DT).min(1.0))
    } else {
        horizontal.lerp(
            Vec3::ZERO,
            (if skate { 0.35 } else { tuning.decel } * FIXED_DT).min(1.0),
        )
    };
    ctrl.velocity = next + up * rise;

    match ctrl.submersion {
        Submersion::Swimming => {
            // Positive while he is deeper than he floats, so buoyancy pushes
            // him along the local up -- towards the sky on a flat level and
            // away from the core on a planet, from the one number.
            let sunk = depth.unwrap() - SWIM_FLOAT_DEPTH;
            let buoyancy = (sunk * 2.0).clamp(-2.0, 3.0);
            rise += (buoyancy - rise) * 0.16;
            if jump_pressed {
                rise = (rise + 3.8).min(6.0);
                sounds.push(Sfx::Stroke);
            }
            ctrl.grounded = false;
        }
        Submersion::Wading => {
            if jump_pressed {
                // The Hero stays upright because he has no swim animation,
                // but deep water must not swallow the jump control. Treat it
                // as the same upward stroke Mario gets underwater.
                rise = (rise + 3.8).min(6.0);
                ctrl.grounded = false;
                sounds.push(Sfx::Stroke);
            } else {
                rise = approach(rise, 0.0, WADE_SETTLE * FIXED_DT);
            }
        }
        Submersion::Dry => {
            if jump_pressed && ctrl.grounded {
                rise = if skate { 9.0 } else { tuning.jump_velocity };
                ctrl.grounded = false;
                sounds.push(Sfx::Jump);
            }
        }
    }
    // Water supplies its own vertical motion either way -- buoyancy for a
    // swimmer, the pull toward the surface below for a wader -- so gravity and
    // the Hero's booster do not operate until the capsule leaves the box.
    if !ctrl.submersion.in_water() {
        if boost && !ctrl.grounded {
            rise = (rise + tuning.jet_thrust).min(tuning.jet_rise);
        } else if !ctrl.grounded {
            // The original stepped a flat -1.2 onto the speed every frame at
            // 30 Hz. Same number, said as the rate it always was, so that a
            // planet can point it somewhere else without also changing it.
            rise -= gravity.accel() * FIXED_DT;
        }
    }

    ctrl.since_attack += FIXED_DT;
    if attack_pressed {
        // A swing soon after the last one is the second half of a combo; a
        // pause starts over at the first.
        ctrl.combo = u8::from(ctrl.since_attack < COMBO_WINDOW && ctrl.combo == 0);
        ctrl.since_attack = 0.0;
        ctrl.attack_left = 0.55;
        ctrl.motion = Motion::Attack;
        sounds.push(Sfx::Attack);
    }
    ctrl.attack_left = (ctrl.attack_left - FIXED_DT).max(0.0);
    transform.translation += up * (rise * FIXED_DT);
    if ctrl.submersion == Submersion::Wading && rise <= 0.0 {
        // Held at the surface rather than sinking. Approached instead of
        // snapped, so running off a bank into deep water does not pop him up
        // to it; and only a pull, so a bottom shallower than the float depth
        // still wins below and he walks along it. An underwater jump is left
        // alone while rising instead of being pulled straight back down.
        // Along his own up rather than along `+Y`. `below` is how far under
        // the float line he is -- negative when he is over it -- and the step
        // that closes some of that gap is the same step on a planet as on a
        // flat level once it is taken in the direction the water pushes.
        let below = depth.unwrap() - WADE_FLOAT_DEPTH;
        transform.translation += up * approach(0.0, below, WADE_RISE * FIXED_DT);
    }
    let horizontal_step = (ctrl.velocity - up * ctrl.velocity.dot(up)) * FIXED_DT;
    let wanted = transform.translation + horizontal_step;
    let corrected = level.resolve_walls(wanted, up, PLAYER_RADIUS, PLAYER_HEIGHT);
    // Cancel velocity into a wall while retaining tangential motion. This
    // produces natural sliding and avoids accumulating speed against walls.
    let correction = corrected - wanted;
    if correction.length_squared() > 1e-8 {
        let normal = gravity::flatten(correction, up);
        let into_wall = ctrl.velocity.dot(normal);
        if into_wall < 0.0 {
            ctrl.velocity -= normal * into_wall;
        }
    }
    transform.translation = corrected;
    // A head bump stops the rise but not the run: the horizontal step above
    // has already been resolved against the walls, and a low arch should slow
    // a jump under it rather than stopping the player dead.
    if !ctrl.submersion.swimming() && rise > 0.0 {
        if let Some(ceiling) = level.ceiling_above(transform.translation, up, CEILING_CLEARANCE) {
            let head_room = (ceiling - transform.translation).dot(up);
            if head_room < PLAYER_HEIGHT {
                transform.translation += up * (head_room - PLAYER_HEIGHT);
                rise = 0.0;
            }
        }
    }
    let was_grounded = ctrl.grounded;
    let impact = rise;
    let floor = level.ground_below(transform.translation + up * 0.75, up);
    // A wader keeps the ordinary floor logic: in water shallower than the
    // float depth the bottom is under his feet and he walks along it, which is
    // the whole difference between wading and swimming.
    if ctrl.submersion.swimming() {
        ctrl.grounded = false;
    } else if let Some((ground, _)) = floor {
        let separation = (transform.translation - ground).dot(up);
        let walking_on_floor = ctrl.grounded && separation <= 0.75;
        let landed = separation <= 0.0 && rise <= 0.0;
        if walking_on_floor || landed {
            // Contact is a band to rest in rather than a place to be put.
            // Dropping the feet exactly onto the triangle under them every
            // step makes their height follow the mesh facet for facet, and a
            // mesh whose triangles share vertices is continuous in height but
            // not in slope -- so crossing an edge changes how fast the player
            // is rising, in one step, with nothing in between. That step
            // change is the chatter a planet's surface has and a flat floor
            // does not. Easing the gap shut low-passes the slope instead.
            //
            // Sinking is not eased. Being below the floor is a wrong the
            // player can see through, so `clamp` puts him back on it at once
            // -- the same asymmetry the camera boom makes for the same reason:
            // come in immediately, go back out gently.
            let gap = (separation * (1.0 - gravity::settle(FOOT_SETTLE_RATE, FIXED_DT)))
                .clamp(0.0, FOOT_SKIN);
            transform.translation = ground + up * gap;
            rise = 0.0;
            ctrl.grounded = true;
            if !was_grounded && impact < -LANDING_IMPACT {
                sounds.push(Sfx::Land);
                ctrl.land_left = LAND_SECONDS;
            }
        } else if ctrl.grounded {
            ctrl.grounded = false;
        }
    } else {
        // Leaving a ledge must enter the air state. Keeping the previous
        // grounded flag here was the source of the visibly floating player.
        ctrl.grounded = false;
    }
    // The climb goes back into the velocity here, once, now that everything
    // that changes it has run.
    gravity::set_rise(&mut ctrl.velocity, up, rise);
    if level.out_of_bounds(transform.translation) {
        transform.translation = respawn.0;
        ctrl.velocity = Vec3::ZERO;
        rise = 0.0;
        previous.translation = transform.translation;
        previous.rotation = transform.rotation;
    }
    if ctrl.health == 0 {
        transform.translation = respawn.0;
        ctrl.velocity = Vec3::ZERO;
        rise = 0.0;
        ctrl.health = 3;
        ctrl.invulnerable_left = 1.5;
        previous.translation = transform.translation;
        previous.rotation = transform.rotation;
    }
    // Stood upright first, then turned. On a flat level the first step is a
    // no-op -- up never moves, so re-deriving the rotation from it gives the
    // same rotation back, and easing towards a rotation already held is the
    // rotation already held -- and on a planet it is what keeps the character
    // perpendicular to the ground instead of leaning further over the further
    // he walks from where he set off.
    //
    // Eased rather than assigned. Setting the body to the local up every step
    // welds it to the geometry, so the surface is the only thing deciding how
    // it moves; a rate leaves a body between the two with some say in it. Note
    // that the turn below was always written this way -- it is the levelling
    // that was snapping while the turning had give.
    let facing = gravity::flatten(transform.rotation * Vec3::Z, up);
    if facing != Vec3::ZERO {
        let upright = Transform::default().looking_to(-facing, up).rotation;
        transform.rotation = transform
            .rotation
            .slerp(upright, gravity::settle(UPRIGHT_RATE, FIXED_DT));
    }
    if wish.length_squared() > 0.01 {
        // `wish` already lies flat against the ground: it is built out of the
        // camera's flattened axes. The character model faces `+Z`, which is
        // what `looking_to` is being handed the negative of.
        let turned = Transform::default().looking_to(-wish, up).rotation;
        transform.rotation = transform.rotation.slerp(turned, 0.28);
    }
    // Footfalls are paced by ground covered rather than by time, so they stay
    // in step with the run whatever the speed is tuned to; strokes are paced
    // by time, because swimming has no stride to measure.
    let ground_speed = (ctrl.velocity - up * ctrl.velocity.dot(up)).length();
    if ctrl.submersion.swimming() {
        ctrl.step_phase = 0.0;
        ctrl.stroke_phase += FIXED_DT;
        if ctrl.stroke_phase >= STROKE_SECONDS && ground_speed > 0.25 {
            ctrl.stroke_phase = 0.0;
            sounds.push(Sfx::Stroke);
        }
    } else {
        ctrl.stroke_phase = 0.0;
        // Skates roll rather than step, so only a run is footed.
        if ctrl.grounded && !skate && ground_speed > 0.25 {
            ctrl.step_phase += ground_speed * FIXED_DT;
            if ctrl.step_phase >= STEP_DISTANCE {
                ctrl.step_phase = 0.0;
                sounds.push(Sfx::Step);
            }
        } else {
            ctrl.step_phase = 0.0;
        }
    }

    ctrl.land_left = (ctrl.land_left - FIXED_DT).max(0.0);
    if !ctrl.grounded {
        ctrl.land_left = 0.0;
    }
    if ctrl.attack_left <= 0.0 {
        // A wade is drawn as a walk, standing up, whether or not the bottom is
        // under his feet -- `sync_graphics` in `sm64py/hero/state.py` makes
        // the same point, and it is why this is tested before `grounded`
        // rather than after it.
        ctrl.motion = if ctrl.submersion == Submersion::Wading {
            if horizontal.length() > 0.25 {
                Motion::Run
            } else {
                Motion::Idle
            }
        } else if ctrl.grounded {
            if skate {
                Motion::Skate
            } else if ctrl.land_left > 0.0 {
                Motion::Land
            } else if horizontal.length() > 0.25 {
                Motion::Run
            } else {
                Motion::Idle
            }
        } else if ctrl.submersion.swimming() {
            Motion::Swim
        } else if boost {
            Motion::Fly
        } else if rise > 0.0 {
            Motion::Jump
        } else {
            Motion::Fall
        };
    }
}

#[allow(clippy::type_complexity)]
pub fn sync_visual(
    fixed_time: Res<Time<Fixed>>,
    player: Query<(&Transform, &PreviousPose), (With<Player>, Without<PlayerVisual>)>,
    mut render_pose: ResMut<RenderPose>,
    mut visuals: Query<(&ActiveCharacter, &mut Transform), With<PlayerVisual>>,
) {
    let Ok((root, previous)) = player.single() else {
        return;
    };
    let alpha = fixed_time.overstep_fraction().clamp(0.0, 1.0);
    render_pose.translation = previous.translation.lerp(root.translation, alpha);
    render_pose.rotation = previous.rotation.slerp(root.rotation, alpha);
    for (kind, mut visual) in &mut visuals {
        visual.translation = render_pose.translation;
        visual.rotation = render_pose.rotation;
        visual.scale = match kind {
            ActiveCharacter::Hero => Vec3::splat(0.81),
            ActiveCharacter::Mario => Vec3::splat(0.00667),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{audio::Sfx, level};
    use bevy::{camera::Camera3d, ecs::system::RunSystemOnce};
    use std::f32::consts::FRAC_PI_2;

    /// The castle's Hero spawn, on the lawn in front of the gate.
    const SPAWN: Vec3 = Vec3::new(-13.28, 3.0, 46.64);

    /// A world holding just what `movement` reads: the real castle collision,
    /// a player, and a camera to take the movement basis from. No renderer and
    /// no window, so this runs in CI and under WSL without opening anything.
    fn world() -> World {
        let (collision, _) = level::load();
        world_with(collision, SPAWN)
    }

    fn world_with(collision: level::LevelData, spawn_at: Vec3) -> World {
        let mut world = World::new();
        world.insert_resource(collision);
        // Flat gravity and the castle's own spawn: these tests are about the
        // castle, and the arithmetic they check is the arithmetic they have
        // always checked.
        world.insert_resource(Gravity::default());
        world.insert_resource(Respawn(spawn_at));
        world.insert_resource(GameState::default());
        world.insert_resource(GameTuning::default());
        world.insert_resource(InputState::default());
        world.insert_resource(SoundQueue::default());
        world.insert_resource(Loadout::default());
        let spawn = Transform::from_translation(spawn_at);
        world.spawn((
            Player,
            PreviousPose::new(&spawn),
            Controller::default(),
            spawn,
        ));
        // Facing -Z, so "forward" input runs toward the castle.
        world.spawn((Camera3d::default(), Transform::from_xyz(-13.0, 6.0, 56.0)));
        world
    }

    fn tick(world: &mut World, ticks: usize) {
        for _ in 0..ticks {
            world.run_system_once(movement).expect("movement could not run");
        }
    }

    /// The player's position, and the parts of its controller the tests read.
    fn player(world: &mut World) -> (Vec3, Vec3, bool, Motion) {
        let mut query = world.query_filtered::<(&Transform, &Controller), With<Player>>();
        let (transform, controller) = query.single(world).unwrap();
        (
            transform.translation,
            controller.velocity,
            controller.grounded,
            controller.motion,
        )
    }

    fn sounds(world: &mut World) -> Vec<Sfx> {
        world
            .resource_mut::<SoundQueue>()
            .drain()
            .into_iter()
            .map(|event| event.sfx)
            .collect()
    }

    /// The body eases onto the local up instead of being set to it. Two
    /// assertions, and the first is the one that matters: a single step must
    /// *not* finish the job. Snapping passes every "does he end up upright"
    /// test there is, which is how it survived this long.
    #[test]
    fn standing_up_on_a_planet_is_a_swing_and_not_a_snap() {
        let mut world = world_with(
            level::LevelData::planet(&[], &[], Vec3::ZERO, 300.0, None),
            Vec3::X * 300.0,
        );
        world.insert_resource(Gravity::towards(Vec3::ZERO));
        let body_up = |world: &mut World| {
            let mut query = world.query_filtered::<&Transform, With<Player>>();
            query.single(world).unwrap().rotation * Vec3::Y
        };
        // He arrives holding the castle's `+Y`, which a quarter turn round the
        // planet means lying on his side.
        assert!((body_up(&mut world).angle_between(Vec3::X) - FRAC_PI_2).abs() < 1e-4);
        tick(&mut world, 1);
        let after_one = body_up(&mut world).angle_between(Vec3::X);
        assert!(after_one < FRAC_PI_2 - 0.05, "did not start standing up");
        assert!(
            after_one > 1.0,
            "a quarter turn in a thirtieth of a second: {after_one} rad left"
        );
        tick(&mut world, 29);
        assert!(
            body_up(&mut world).angle_between(Vec3::X) < 0.02,
            "a second later he is still not upright"
        );
    }

    /// And the flat level is untouched by that, because up never moves there:
    /// easing towards a rotation already held is the rotation already held.
    #[test]
    fn the_castle_still_stands_the_player_exactly_upright() {
        let mut world = world();
        tick(&mut world, 60);
        let mut query = world.query_filtered::<&Transform, With<Player>>();
        let up = query.single(&world).unwrap().rotation * Vec3::Y;
        assert!((up - Vec3::Y).length() < 1e-5, "leaning: {up}");
    }

    /// Contact is a band, but only upwards. Below the floor is a wrong that can
    /// be seen through, so it is corrected in the step it is noticed.
    #[test]
    fn sinking_into_the_floor_is_undone_in_one_step() {
        let mut world = world();
        tick(&mut world, 30);
        let (settled, ..) = player(&mut world);
        {
            let mut query = world.query_filtered::<&mut Transform, With<Player>>();
            query.single_mut(&mut world).unwrap().translation.y = settled.y - 0.33;
        }
        tick(&mut world, 1);
        let (after, ..) = player(&mut world);
        assert!(
            after.y >= settled.y - 1e-3,
            "left {} under the lawn",
            settled.y - after.y
        );
    }

    /// The other half: the gap the ease leaves open is bounded. Running across
    /// the castle's slopes opens it as fast as the filter shuts it, and without
    /// the cap the two settle several times higher than this.
    #[test]
    fn the_feet_never_float_further_than_the_skin() {
        let mut world = world();
        tick(&mut world, 30);
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        let mut walked = 0;
        for step in 0..90 {
            tick(&mut world, 1);
            let mut query = world.query_filtered::<(&Transform, &Controller), With<Player>>();
            let (transform, controller) = query.single(&world).unwrap();
            if !controller.grounded {
                continue;
            }
            let at = transform.translation;
            let level = world.resource::<level::LevelData>();
            let Some((ground, _)) = level.ground_below(at + Vec3::Y * 0.75, Vec3::Y) else {
                continue;
            };
            let gap = (at - ground).dot(Vec3::Y);
            assert!(
                (-1e-3..=FOOT_SKIN + 1e-3).contains(&gap),
                "step {step} left the feet {gap} off the floor"
            );
            walked += 1;
        }
        // Otherwise a run that never touched the ground passes this silently.
        assert!(walked > 60, "only {walked} of 90 steps were spent walking");
    }

    #[test]
    fn the_player_settles_onto_the_lawn_and_stays_there() {
        let mut world = world();
        tick(&mut world, 30);
        let (position, _, grounded, _) = player(&mut world);
        assert!(grounded, "never found the floor");
        assert!(
            (position.y - SPAWN.y).abs() < 1.0,
            "settled at {} rather than near the spawn height",
            position.y
        );
        assert!(
            !sounds(&mut world).contains(&Sfx::Step),
            "stood still, stepped anyway"
        );
    }

    #[test]
    fn a_latched_jump_fires_once_and_lands_with_a_thud() {
        let mut world = world();
        tick(&mut world, 30);
        sounds(&mut world);

        world.resource_mut::<InputState>().jump = true;
        tick(&mut world, 1);
        let (_, velocity, grounded, _) = player(&mut world);
        assert!(velocity.y > 0.0, "the jump did not take off");
        assert!(!grounded);
        assert_eq!(sounds(&mut world), vec![Sfx::Jump]);
        // The edge was consumed, so the held-down key cannot jump again from
        // mid-air on the next tick.
        assert!(!world.resource::<InputState>().jump);

        // Falling back down raises exactly one landing.
        tick(&mut world, 60);
        let (_, _, grounded, _) = player(&mut world);
        assert!(grounded, "never came back down");
        let heard = sounds(&mut world);
        assert_eq!(
            heard.iter().filter(|sfx| **sfx == Sfx::Land).count(),
            1,
            "{heard:?}"
        );
    }

    #[test]
    fn running_covers_ground_and_paces_footfalls_by_distance() {
        let mut world = world();
        tick(&mut world, 30);
        sounds(&mut world);

        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        tick(&mut world, 60);
        let (position, _, _, motion) = player(&mut world);
        let travelled = (position - SPAWN).length();
        assert!(travelled > 5.0, "only travelled {travelled}");
        assert_eq!(motion, Motion::Run);

        let steps = sounds(&mut world)
            .iter()
            .filter(|sfx| **sfx == Sfx::Step)
            .count();
        // Two seconds at the Hero's speed covers roughly 20 units, which is
        // several strides and nowhere near one per tick.
        assert!(
            (2..=12).contains(&steps),
            "{steps} footfalls in two seconds"
        );
    }

    #[test]
    fn an_attack_is_one_swing_per_press() {
        let mut world = world();
        tick(&mut world, 30);
        sounds(&mut world);

        world.resource_mut::<InputState>().attack = true;
        tick(&mut world, 10);
        let heard = sounds(&mut world);
        assert_eq!(
            heard.iter().filter(|sfx| **sfx == Sfx::Attack).count(),
            1,
            "{heard:?}"
        );
    }

    /// A floor and a roof over it, each a quad of two triangles, centred on
    /// the origin and large enough that the player cannot run out from under
    /// the roof during the test.
    fn room(roof: f32) -> level::LevelData {
        let quad = |y: f32| {
            [
                Vec3::new(-20., y, -20.),
                Vec3::new(20., y, -20.),
                Vec3::new(20., y, 20.),
                Vec3::new(-20., y, 20.),
            ]
        };
        let mut vertices = quad(0.0).to_vec();
        vertices.extend(quad(roof));
        let faces = vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]];
        level::LevelData::new(vertices, faces, Vec::new())
    }

    /// A flat bottom with a body of water over it, both wider than a test can
    /// walk out of. The surface is at zero and the bottom `depth` below it.
    fn pool(depth: f32) -> level::LevelData {
        let corners = [
            Vec3::new(-40., -depth, -40.),
            Vec3::new(40., -depth, -40.),
            Vec3::new(40., -depth, 40.),
            Vec3::new(-40., -depth, 40.),
        ];
        level::LevelData::new(
            corners.to_vec(),
            vec![[0, 1, 2], [0, 2, 3]],
            vec![level::WaterBox {
                min_x: -40.,
                min_z: -40.,
                max_x: 40.,
                max_z: 40.,
                surface_y: 0.0,
            }],
        )
    }

    fn submersion_of(world: &mut World) -> Submersion {
        let mut query = world.query_filtered::<&Controller, With<Player>>();
        query.single(world).unwrap().submersion
    }

    /// Distance covered over a second of running forward.
    fn ground_covered(world: &mut World) -> f32 {
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        let (from, ..) = player(world);
        tick(world, 30);
        let (to, ..) = player(world);
        Vec2::new(to.x - from.x, to.z - from.z).length()
    }

    /// The Hero has no swimming clips, so he does not swim: deep water holds
    /// him at the surface and slows him to a wade, drawn upright. This is
    /// `act_wading` in `sm64py/hero/actions.py`, and it is the one place his
    /// move set is deliberately short of Mario's.
    #[test]
    fn the_hero_wades_rather_than_swimming() {
        let mut world = world_with(pool(6.0), Vec3::ZERO);
        let mut deepest = 0.0f32;
        for _ in 0..90 {
            tick(&mut world, 1);
            let (position, _, _, motion) = player(&mut world);
            assert_ne!(motion, Motion::Swim, "the Hero is swimming");
            deepest = deepest.min(position.y);
        }
        assert_eq!(submersion_of(&mut world), Submersion::Wading);
        let (position, _, _, _) = player(&mut world);
        assert!(
            (position.y + WADE_FLOAT_DEPTH).abs() < 0.2,
            "settled at {} rather than at the surface",
            position.y
        );
        assert!(
            deepest > -2.0,
            "he sank to {deepest} on the way in rather than being held up"
        );
        // And he walks through it, which is the whole point of the clip he
        // does have.
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        tick(&mut world, 10);
        assert_eq!(player(&mut world).3, Motion::Run);
    }

    /// The same water, wrapped round a planet.
    ///
    /// Deep is deep whatever shape the world is: the surface he floats up to
    /// is a radius here rather than a height, and the buoyancy that carries
    /// him to it points away from the core rather than at the sky. Nothing in
    /// the controller knows which of those it is doing -- it is handed a depth
    /// and a local up -- and this is the test that says so.
    #[test]
    fn mario_swims_in_a_planet_sea() {
        let sea = 300.0;
        let mut world = world_with(
            // No collision: he is six metres under, and what is being measured
            // is the water holding him up rather than the seabed doing it.
            level::LevelData::planet(&[], &[], Vec3::ZERO, sea, Some(sea)),
            Vec3::X * (sea - 6.0),
        );
        world.insert_resource(Gravity::towards(Vec3::ZERO));
        world.resource_mut::<GameState>().active = ActiveCharacter::Mario;
        tick(&mut world, 120);
        assert_eq!(submersion_of(&mut world), Submersion::Swimming);
        let (position, ..) = player(&mut world);
        let depth = sea - position.length();
        assert!(
            (depth - SWIM_FLOAT_DEPTH).abs() < 0.5,
            "floating {depth} m under the surface rather than {SWIM_FLOAT_DEPTH}"
        );
        // And he came up along his own up rather than along the world's: a
        // swimmer pulled towards `+Y` here would have drifted off sideways.
        assert!(
            position.normalize().dot(Vec3::X) > 0.999,
            "he surfaced at {position}, which is not where he went under"
        );
    }

    /// The same water, the other character. Without this the test above would
    /// pass just as well if nothing in the game could swim at all.
    #[test]
    fn mario_still_swims_in_the_same_water() {
        let mut world = world_with(pool(6.0), Vec3::ZERO);
        world.resource_mut::<GameState>().active = ActiveCharacter::Mario;
        tick(&mut world, 90);
        assert_eq!(submersion_of(&mut world), Submersion::Swimming);
        assert_eq!(player(&mut world).3, Motion::Swim);
    }

    #[test]
    fn jump_moves_the_hero_up_in_deep_water() {
        let mut world = world_with(pool(6.0), Vec3::ZERO);
        tick(&mut world, 60);
        let (before, ..) = player(&mut world);
        world.resource_mut::<InputState>().jump = true;
        tick(&mut world, 6);
        let (after, velocity, grounded, _) = player(&mut world);
        assert!(
            after.y > before.y + 0.2,
            "the underwater jump moved from {} to {} with velocity {velocity:?}",
            before.y,
            after.y
        );
        assert!(velocity.y > 0.0, "the underwater jump lost its upward speed");
        assert!(!grounded, "the underwater jump stayed grounded");
    }

    /// Water shallower than he floats in is simply slow ground: the bottom is
    /// under his feet and he walks along it.
    #[test]
    fn the_hero_walks_the_bottom_of_shallow_water() {
        let bottom = WADE_FLOAT_DEPTH - 0.2;
        let mut world = world_with(pool(bottom), Vec3::ZERO);
        tick(&mut world, 60);
        let (position, _, grounded, _) = player(&mut world);
        assert_eq!(submersion_of(&mut world), Submersion::Wading);
        assert!(grounded, "floating over a bottom he could stand on");
        assert!(
            (position.y + bottom).abs() < 0.05,
            "standing at {} rather than on the bottom at {}",
            position.y,
            -bottom
        );
    }

    /// "Slower under water" is the requirement, so it is measured rather than
    /// assumed: the same second of running, in water and out of it.
    #[test]
    fn wading_is_slower_than_walking() {
        let mut dry = world_with(room(50.0), Vec3::ZERO);
        tick(&mut dry, 30);
        let on_land = ground_covered(&mut dry);

        let mut wet = world_with(pool(6.0), Vec3::ZERO);
        tick(&mut wet, 60);
        let in_water = ground_covered(&mut wet);

        assert!(
            in_water < on_land * 0.75,
            "wading covered {in_water} to walking's {on_land}, which is not \
             slower in any way a player would notice"
        );
        assert!(in_water > 1.0, "wading is not movement at all: {in_water}");
    }

    #[test]
    fn a_ceiling_stops_a_jump_without_stopping_the_run() {
        // Head room of a third of a metre: enough to stand in, not enough to
        // jump in.
        let roof = PLAYER_HEIGHT + 0.35;
        let mut world = world_with(room(roof), Vec3::new(0., 0., 0.));
        tick(&mut world, 10);

        world.resource_mut::<InputState>().jump = true;
        world.resource_mut::<InputState>().move_axis = Vec2::new(0.0, 1.0);
        let mut highest: f32 = 0.0;
        for _ in 0..20 {
            tick(&mut world, 1);
            let (position, _, _, _) = player(&mut world);
            highest = highest.max(position.y);
            assert!(
                position.y + PLAYER_HEIGHT <= roof + 1e-3,
                "the head passed through the roof at {}",
                position.y
            );
        }
        // It did leave the ground -- the assertion above must be catching a
        // real jump rather than a player that never rose at all.
        assert!(highest > 0.05, "the jump never happened: {highest}");
        // And the bump stopped the rise without stopping the run.
        let (position, _, _, _) = player(&mut world);
        assert!(position.z.abs() > 2.0, "the head bump halted the run");
    }
}
