//! What the Hero is holding, and what happens when he pulls the trigger.
//!
//! `docs/aim.md` sketches a `WeaponController` and lists the two ways a ranged
//! weapon can resolve -- "Projectile / hitscan" -- without picking one. Both are
//! here, chosen per weapon by its [`Spec`], because they are not the same
//! weapon feel and a game with one gun in it does not stay a game with one gun
//! in it. A target pistol is instant and exact; a launcher should be neither,
//! and when that launcher arrives it changes a line of its spec rather than
//! this module.
//!
//! ## How the gun gets into his hand
//!
//! `tools/aim_rig.py` puts a `WEAPON_SOCKET` joint under `DEF-hand.R`, and
//! until now nothing hung off it. The weapon is spawned as a *child entity* of
//! that joint, so the hand carries it for free and there is no per-frame
//! position to compute and no frame of lag in it.
//!
//! Its rotation is another matter. The Hero has no clip that holds a gun --
//! `docs/aim.md`'s "Not built yet" says so, and authoring one is a rig job
//! rather than a code one -- so the hand it is attached to is in whatever pose
//! the idle or the run put it in, which is by his side pointing at the floor. A
//! gun that inherited that would fire visibly sideways from where the shot
//! actually goes. So the socket's rotation is *cancelled* and the aim is
//! written in its place, which is precisely what `billboard::aim` does to a
//! quad whose joint chain leaves it rolled a quarter turn, and for the same
//! reason: `net = parent * local`, so the local wanted is `parent^-1 * world`.
//!
//! With `aim::drive` turning `AIM_TORSO` toward the shot, the hand is already
//! carried round to roughly the right side of his body, so what is left for
//! this cancellation to do is small and the result reads as a man pointing a
//! pistol rather than as a pistol flying alongside a man.
//!
//! ## Where the shot comes from
//!
//! The muzzle, not the camera. `assets/hero/target_pistol.blend` carries a
//! `MUZZLE` empty at the end of the bore -- one of the four things
//! `notes4LLMs.md` asks every weapon .blend to include -- and it survives the
//! glTF export as an ordinary childless node, so the runtime finds it by name
//! and reads its `GlobalTransform`. Shots are then aimed at [`aim::Aim::point`]
//! rather than fired along the muzzle's own forward, so they converge on what
//! the crosshair is over instead of running parallel to it and missing
//! everything by the width of the Hero's shoulders.

use crate::{
    aim::Aim,
    audio::{Sfx, SoundQueue},
    console::GameTuning,
    enemy::Enemy,
    input::InputState,
    level::LevelData,
    player::{Player, FIXED_DT},
    squad::Ally,
};
use bevy::{
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    prelude::*,
    transform::TransformSystems,
    world_serialization::WorldAssetRoot,
};

/// Everything a shot is allowed to hit.
///
/// The two `Without`s that are not about sides are about the ECS rather than
/// the game. A bullet carries a `Transform` too, and `fly` holds those mutably
/// while it reads these; nothing in the archetypes makes the two sets disjoint
/// on its own -- an enemy is not a bullet, but that cannot be known from
/// `&Enemy` alone -- so the exclusion is stated. Without it the whole
/// simulation schedule panics with B0001 on the first tick a bullet exists.
///
/// `Ally` is the one about sides: the Marios are hostile to the same things
/// the player is, and a shot must pass through them. `enemy::ally_combat`
/// excludes them from its own enemy query for the same reason.
type Targets<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static Enemy, &'static Transform),
    (Without<Player>, Without<Ally>, Without<Bullet>),
>;

/// The joint `tools/aim_rig.py` hangs a weapon from, and the empty
/// `target_pistol.blend` marks the end of the bore with.
///
/// Both by name, for the reason every other name in this port is: it is the
/// only thing a glTF export preserves.
const SOCKET: &str = "WEAPON_SOCKET";
const MUZZLE: &str = "MUZZLE";

/// How much fatter a bullet is drawn than a tracer, and how much longer than it
/// is thick, so it reads as a round travelling point-first rather than a cube.
///
/// Ratios rather than widths of their own, so `tracer_width` on the console
/// moves both together: they are the same shot drawn two ways, and a build
/// where one is legible and the other is not has been tuned twice.
///
/// How a shot *looks* is on the console rather than in constants here, and the
/// reason is worth keeping written down. A tracer is drawn very nearly end-on:
/// it runs from the muzzle to whatever the crosshair is over, which is roughly
/// where the camera is already looking, so its whole length projects into a
/// short streak beside the gun. Nothing about the numbers predicts how that
/// reads, and one wrong guess made from a still frame is what established it --
/// see `tracer_seconds` and `tracer_width` in `console::SPECS`.
const BULLET_FATTER: f32 = 3.0;
const BULLET_STRETCH: f32 = 3.0;

/// How a weapon's shot resolves.
///
/// The two are genuinely different weapons rather than two implementations of
/// one. Hitscan cannot be dodged and cannot miss by leading badly; a projectile
/// can do both, and can be watched. Keeping both means a weapon picks the one
/// that suits it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shot {
    /// Resolved the instant the trigger is pulled, along the whole ray.
    Hitscan { range: f32 },
    /// A body that flies, and is stepped and tested every tick until it hits
    /// something or runs out of range.
    Projectile { speed: f32, range: f32 },
}

/// Everything that distinguishes one weapon from another.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    /// What it is called in the HUD.
    pub name: &'static str,
    /// The glTF held in the hand, where the weapon has a model. The sword does
    /// not: it is part of the Hero's own mesh, sheathed on his back.
    pub model: Option<&'static str>,
    /// How the shot resolves, or `None` for a melee weapon, which is resolved
    /// by `enemy::combat` off the swing instead.
    pub shot: Option<Shot>,
    /// Seconds between shots.
    pub interval: f32,
}

/// What the Hero can be carrying.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Weapon {
    /// The sword, swung by `player::movement` and resolved by `enemy::combat`.
    #[default]
    Sword,
    /// The target pistol.
    Pistol,
}

impl Weapon {
    /// The order `Y` cycles through.
    pub const ALL: [Weapon; 2] = [Weapon::Sword, Weapon::Pistol];

    /// The next weapon along, wrapping.
    pub fn next(self) -> Self {
        let at = Self::ALL.iter().position(|w| *w == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    pub fn spec(self) -> Spec {
        match self {
            Weapon::Sword => Spec {
                name: "sword",
                model: None,
                shot: None,
                // Unused: the swing's own cooldown is `player::Controller`'s
                // combo window, which is a different rule and stays there.
                interval: 0.0,
            },
            Weapon::Pistol => Spec {
                name: "pistol",
                model: Some("hero/target_pistol.glb#Scene0"),
                // Hitscan by default -- a target pistol is the one gun that
                // should never be leading its target. `gun_projectile` on the
                // console swaps it, which is how the other path stays honest
                // rather than becoming code nobody has run.
                shot: Some(Shot::Hitscan { range: 60.0 }),
                interval: 0.28,
            },
        }
    }

    /// Whether pulling the trigger fires rather than swings.
    pub fn is_ranged(self) -> bool {
        self.spec().shot.is_some()
    }
}

/// What the Hero is carrying, and how long until it will fire again.
#[derive(Resource, Debug, Default)]
pub struct Loadout {
    pub equipped: Weapon,
    /// Seconds left on the current weapon's cooldown.
    pub cooldown: f32,
}

/// The joint a weapon hangs from.
#[derive(Component)]
pub struct WeaponSocket;

/// A weapon model in the hand. Carries which weapon it is, so swapping shows
/// one and hides the rest rather than respawning scenes.
#[derive(Component)]
pub struct Held(pub Weapon);

/// The end of the bore, claimed from the empty in the .blend.
#[derive(Component)]
pub struct Muzzle;

/// A shot in flight.
#[derive(Component)]
pub struct Bullet {
    velocity: Vec3,
    /// Metres of range left before it gives up.
    left: f32,
}

/// The line a hitscan shot left behind, and how long it has left.
#[derive(Component)]
pub struct Tracer(f32);

/// The mesh and material every shot is drawn with.
///
/// Built once and shared. A tracer is a stretched unit cuboid rather than a
/// mesh per shot, so firing allocates nothing.
#[derive(Resource)]
pub struct ShotAssets {
    mesh: Handle<Mesh>,
    tracer: Handle<StandardMaterial>,
    bullet: Handle<StandardMaterial>,
}

/// Builds the shot mesh and its two materials.
///
/// Deliberately *not* under a [`WorldAssetRoot`]: `n64::convert` only restyles
/// meshes inside one, and a tracer is not level geometry that was lit offline.
/// It should be the flat bright line it is drawn as, so it keeps the standard
/// material and is left alone.
pub fn load_shot_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let glow = |colour: Color| StandardMaterial {
        base_color: colour,
        emissive: colour.to_linear() * 8.0,
        unlit: true,
        ..default()
    };
    commands.insert_resource(ShotAssets {
        mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        tracer: materials.add(glow(Color::srgb(1.0, 0.92, 0.55))),
        bullet: materials.add(glow(Color::srgb(1.0, 0.72, 0.20))),
    });
}

/// Tags the socket and the muzzle as their scenes arrive.
pub fn claim(mut commands: Commands, arrivals: Query<(Entity, &Name), Added<Name>>) {
    for (entity, name) in &arrivals {
        match name.as_str() {
            SOCKET => {
                commands.entity(entity).insert(WeaponSocket);
            }
            MUZZLE => {
                commands.entity(entity).insert(Muzzle);
            }
            _ => {}
        }
    }
}

/// Hangs every weapon that has a model off the socket, once, as it appears.
pub fn attach(
    mut commands: Commands,
    assets: Res<AssetServer>,
    sockets: Query<Entity, Added<WeaponSocket>>,
) {
    for socket in &sockets {
        for weapon in Weapon::ALL {
            let Some(model) = weapon.spec().model else {
                continue;
            };
            commands.entity(socket).with_child((
                Held(weapon),
                WorldAssetRoot(assets.load(model)),
                Transform::default(),
                // Shown by `carry` when it is the one in hand.
                Visibility::Hidden,
            ));
        }
    }
}

/// `Y` cycles the weapon.
pub fn swap(
    mut input: ResMut<InputState>,
    mut loadout: ResMut<Loadout>,
    mut sounds: ResMut<SoundQueue>,
) {
    if !InputState::take(&mut input.swap_weapon) {
        return;
    }
    loadout.equipped = loadout.equipped.next();
    // The cooldown does not carry across: drawing a weapon is not a way to
    // skip the last one's recovery, nor to inherit it.
    loadout.cooldown = 0.0;
    sounds.push(Sfx::Draw);
}

/// Carries out the console's `weapon` command.
///
/// Out in the overlay beside `enemy::crowd` rather than in the simulation, and
/// for the same reason that one is: the console is open at the moment the
/// command is typed, and a weapon that only appeared in his hand once you shut
/// the console is a weapon you never saw him draw.
pub fn equip(mut console: ResMut<crate::console::ConsoleState>, mut loadout: ResMut<Loadout>) {
    for request in console.take_requests() {
        match request {
            crate::console::Request::Equip(weapon) => {
                loadout.equipped = weapon;
                loadout.cooldown = 0.0;
            }
            other => console.defer(other),
        }
    }
}

/// Points the held weapon down the aim, and shows only the one in hand.
///
/// The scale is put back as well as the rotation, and that is not tidiness. The
/// socket's world scale is the Hero's own 0.81 times whatever the Rigify stretch
/// bones are doing to the arm this frame, and a pistol authored at 0.32 m in the
/// .blend has to come out 0.32 m long in the world whatever the elbow is up to.
/// Cancelling it here is what keeps the size question answered in the .blend --
/// where `docs/pipeline.md` insists it lives -- rather than as a fudge factor in
/// this file.
#[allow(clippy::type_complexity)]
pub fn carry(
    loadout: Res<Loadout>,
    aim: Res<Aim>,
    globals: Query<&GlobalTransform>,
    mut held: Query<(&Held, &mut Transform, &mut Visibility, &ChildOf)>,
) {
    for (held, mut transform, mut visibility, parent) in &mut held {
        let shown = held.0 == loadout.equipped;
        *visibility = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if !shown {
            continue;
        }
        let Ok(socket) = globals.get(parent.parent()) else {
            continue;
        };
        let (scale, rotation, _) = socket.to_scale_rotation_translation();
        // The model is authored with its bore down +Z and its grip on the
        // origin, so aiming it is one rotation and no offset. `looking_to`
        // points Bevy's -Z forward, and this port's forward is +Z -- the same
        // convention `player::movement` and `billboard::facing` use -- so the
        // direction goes in negated.
        let world = Transform::default()
            .looking_to(-aim.direction, Vec3::Y)
            .rotation;
        transform.rotation = rotation.inverse() * world;
        transform.scale = Vec3::new(
            1.0 / scale.x.abs().max(f32::EPSILON),
            1.0 / scale.y.abs().max(f32::EPSILON),
            1.0 / scale.z.abs().max(f32::EPSILON),
        );
    }
}

/// Where a shot starts and which way it goes.
///
/// Aimed at the point the camera's ray landed on rather than along the muzzle's
/// own forward, so a gun held off to one side still puts its shot on the
/// crosshair. If that point is behind the muzzle -- the camera's ray hit a wall
/// between the eye and the Hero -- there is nothing sensible to converge on and
/// the aim direction is used unchanged.
pub fn lay(muzzle: Vec3, aim: &Aim) -> (Vec3, Vec3) {
    let toward = aim.point - muzzle;
    if toward.dot(aim.direction) <= 0.0 {
        return (muzzle, aim.direction);
    }
    (muzzle, toward.normalize_or(aim.direction))
}

/// How near the ray passes an enemy's body, and how far along it that happens.
///
/// The body is the capsule between its feet and its head -- taken from the
/// direction its own model is stood on, so one crawling along a ceiling is
/// tested along its own axis rather than along a presumed vertical, exactly as
/// `enemy::combat` measures its vertical overlap. Returns the distance along
/// the ray, or `None` if the ray misses or the body is behind or beyond it.
pub fn ray_hits_body(
    origin: Vec3,
    direction: Vec3,
    range: f32,
    base: Vec3,
    up: Vec3,
    radius: f32,
    height: f32,
) -> Option<f32> {
    // Closest approach between the ray and the body's axis, as two clamped
    // parameters. The standard segment-segment routine, with the ray's
    // parameter clamped to 0..range and the body's to its own length.
    let axis = up * height;
    let offset = origin - base;
    let axis_len2 = axis.length_squared();
    let along = direction.dot(offset);
    let (mut t, s);
    if axis_len2 <= f32::EPSILON {
        // A body with no height at all is a sphere on its base.
        s = 0.0;
        t = -along;
    } else {
        let axis_dot = direction.dot(axis);
        let axis_offset = axis.dot(offset);
        let denominator = axis_len2 - axis_dot * axis_dot;
        t = if denominator.abs() > f32::EPSILON {
            (axis_dot * axis_offset - along * axis_len2) / denominator
        } else {
            -along
        };
        s = ((axis_dot * t + axis_offset) / axis_len2).clamp(0.0, 1.0);
        // Re-solve the ray against the clamped point on the body, so a hit
        // near an end cap is measured to the cap rather than to the infinite
        // line through it.
        t = (base + axis * s - origin).dot(direction);
    }
    t = t.clamp(0.0, range);
    let on_ray = origin + direction * t;
    let on_body = base + axis * s;
    ((on_ray - on_body).length_squared() <= radius * radius).then_some(t)
}

/// The nearest enemy a ray reaches before the level stops it.
///
/// The level is tested too, and that is the difference between a gun and a
/// wallhack: a shot has to be stopped by the castle it is fired inside.
fn nearest_hit(
    origin: Vec3,
    direction: Vec3,
    range: f32,
    level: &LevelData,
    enemies: &Targets,
) -> (Option<Entity>, Vec3) {
    // Whatever the level does to the shot bounds everything else, so it is
    // measured once and used as the range for the bodies.
    let (wall, mut best) = match level.surface_hit(origin, origin + direction * range) {
        Some((hit, _)) => (Some(hit), (hit - origin).length()),
        None => (None, range),
    };
    let mut struck = None;
    for (entity, enemy, transform) in enemies {
        let (radius, height) = enemy.kind.body();
        let up = transform.rotation * Vec3::Y;
        if let Some(distance) = ray_hits_body(
            origin,
            direction,
            best,
            transform.translation,
            up,
            radius,
            height,
        ) {
            if distance < best {
                best = distance;
                struck = Some(entity);
            }
        }
    }
    let landed = match (struck, wall) {
        (None, Some(hit)) => hit,
        _ => origin + direction * best,
    };
    (struck, landed)
}

/// Pulls the trigger.
///
/// Fixed step, and it consumes the same latched `attack` edge the sword swing
/// does -- `player::movement` leaves it alone whenever the equipped weapon is
/// ranged, so exactly one of the two acts on any press. Doing it with the latch
/// rather than with `just_pressed` is what keeps a shot from being fired twice
/// on a slow frame or swallowed on a fast one; see `input::InputState`.
#[allow(clippy::too_many_arguments)]
pub fn fire(
    mut commands: Commands,
    mut input: ResMut<InputState>,
    mut loadout: ResMut<Loadout>,
    mut sounds: ResMut<SoundQueue>,
    tuning: Res<GameTuning>,
    aim: Res<Aim>,
    level: Res<LevelData>,
    assets: Option<Res<ShotAssets>>,
    muzzles: Query<&GlobalTransform, With<Muzzle>>,
    player: Query<&Transform, With<Player>>,
    enemies: Targets,
) {
    loadout.cooldown = (loadout.cooldown - FIXED_DT).max(0.0);
    let spec = loadout.equipped.spec();
    let Some(shot) = spec.shot else {
        return;
    };
    // The edge is only this system's to take while a ranged weapon is out.
    if !InputState::take(&mut input.attack) || loadout.cooldown > 0.0 {
        return;
    }
    // The muzzle is a node of the weapon's own scene, so before that scene has
    // loaded there is nowhere for a shot to come from. Falling back to the
    // player's chest keeps the gun firing on the first frames rather than
    // silently eating the press.
    let from = match muzzles.iter().next() {
        Some(muzzle) => muzzle.translation(),
        None => match player.single() {
            Ok(body) => body.translation + Vec3::Y * 1.2,
            Err(_) => return,
        },
    };
    let (origin, direction) = lay(from, &aim);
    loadout.cooldown = spec.interval;
    sounds.push(Sfx::Shoot);

    match as_fired(shot, &tuning) {
        Shot::Projectile { speed, range } => {
            // The body first and the picture of it second. What a bullet *is*
            // must not depend on whether there is anything to draw it with:
            // `ShotAssets` is missing in a headless run, and a gun that stops
            // killing when nobody is looking is not a gun that can be tested.
            let bullet = commands
                .spawn((
                    Bullet {
                        velocity: direction * speed,
                        left: range,
                    },
                    Transform::from_translation(origin)
                        .looking_to(-direction, Vec3::Y)
                        .with_scale({
                            let width = tuning.tracer_width * BULLET_FATTER;
                            Vec3::new(width, width, width * BULLET_STRETCH)
                        }),
                ))
                .id();
            if let Some(assets) = assets {
                commands.entity(bullet).insert((
                    Mesh3d(assets.mesh.clone()),
                    MeshMaterial3d(assets.bullet.clone()),
                ));
            }
        }
        Shot::Hitscan { range } => {
            let (struck, landed) = nearest_hit(origin, direction, range, &level, &enemies);
            if let Some(entity) = struck {
                commands.entity(entity).despawn();
                // At the far end of the beam rather than at the gun: a hitscan
                // shot kills where it lands, and that is where it is heard.
                sounds.push_at(Sfx::Defeat, landed);
            }
            if let Some(assets) = assets {
                spawn_tracer(&mut commands, &assets, &tuning, origin, landed);
            }
        }
    }
}

/// The shot a weapon actually fires, after the console has had its say.
///
/// `gun_projectile` exists because a build that ships one gun otherwise ships
/// one resolution, and the other becomes code that compiles and has never run.
/// Flipping it re-reads the weapon's own range -- how far it carries is a
/// property of the gun, not of how the shot is resolved -- and takes the speed
/// from `bullet_speed`, which a hitscan weapon has no reason to carry.
pub fn as_fired(shot: Shot, tuning: &GameTuning) -> Shot {
    if tuning.gun_projectile <= 0.5 {
        return shot;
    }
    Shot::Projectile {
        speed: tuning.bullet_speed,
        range: shot_range(shot),
    }
}

/// The range a shot carries, whichever way it resolves.
pub fn shot_range(shot: Shot) -> f32 {
    match shot {
        Shot::Hitscan { range } | Shot::Projectile { range, .. } => range,
    }
}

/// Draws the line a hitscan shot took.
fn spawn_tracer(
    commands: &mut Commands,
    assets: &ShotAssets,
    tuning: &GameTuning,
    from: Vec3,
    to: Vec3,
) {
    let along = to - from;
    let length = along.length();
    if length < 1e-3 {
        return;
    }
    commands.spawn((
        Tracer(tuning.tracer_seconds),
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(assets.tracer.clone()),
        Transform::from_translation(from + along * 0.5)
            .looking_to(-along.normalize(), Vec3::Y)
            .with_scale(Vec3::new(tuning.tracer_width, tuning.tracer_width, length)),
    ));
}

/// Steps every bullet, and resolves what it ran into.
///
/// Swept rather than sampled: the segment from where it was to where it is
/// arriving is what gets tested, so a fast bullet cannot step straight over a
/// slime between one tick and the next. That is the same tunnelling problem
/// `docs/aim.md` raises for melee sweeps, and it is worse here because a bullet
/// covers metres a tick.
#[allow(clippy::too_many_arguments)]
pub fn fly(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    level: Res<LevelData>,
    mut bullets: Query<(Entity, &mut Bullet, &mut Transform), Without<Player>>,
    enemies: Targets,
) {
    for (entity, mut bullet, mut transform) in &mut bullets {
        let step = bullet.velocity * FIXED_DT;
        let distance = step.length();
        if distance < 1e-6 {
            commands.entity(entity).despawn();
            continue;
        }
        let direction = step / distance;
        let travelled = distance.min(bullet.left);
        let (struck, landed) = nearest_hit(
            transform.translation,
            direction,
            travelled,
            &level,
            &enemies,
        );
        if let Some(hit) = struck {
            commands.entity(hit).despawn();
            sounds.push_at(Sfx::Defeat, landed);
            commands.entity(entity).despawn();
            continue;
        }
        // Short of the full step means the level stopped it.
        if (landed - transform.translation).length() < travelled - 1e-3 {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation += direction * travelled;
        bullet.left -= travelled;
        if bullet.left <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Ages tracers off the screen.
pub fn fade(mut commands: Commands, time: Res<Time>, mut tracers: Query<(Entity, &mut Tracer)>) {
    for (entity, mut tracer) in &mut tracers {
        tracer.0 -= time.delta_secs();
        if tracer.0 <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// Claiming, attaching and carrying, in the window where a pose may be written.
///
/// `carry` reads the socket's `GlobalTransform` to cancel it, so it wants the
/// same slot `billboard::aim` and `aim::drive` take: after the clip has posed
/// the skeleton, before the transforms are propagated.
pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (claim, attach, carry)
        .chain()
        .after(bevy::animation::animate_targets)
        .before(TransformSystems::Propagate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::enemy::{Kind, Side};
    use bevy::ecs::system::RunSystemOnce;

    /// Open ground with nothing on it to stop a shot: one quad well below the
    /// shooting, so the level is real without being in the way.
    fn ground() -> LevelData {
        let corner = |x: f32, z: f32| Vec3::new(x, -4.0, z);
        LevelData::new(
            vec![
                corner(-60., -60.),
                corner(60., -60.),
                corner(60., 60.),
                corner(-60., 60.),
            ],
            vec![[0, 1, 2], [0, 2, 3]],
            Vec::new(),
        )
    }

    /// A player stood at the origin with a gun out, and one enemy `ahead`
    /// metres down +Z, sat squarely in the shot's path.
    ///
    /// No renderer, no window and no [`ShotAssets`]: `fire` takes that as an
    /// `Option` precisely so the resolution can be tested without one, and what
    /// is under test is who dies rather than what it looked like.
    fn range(weapon: Weapon, ahead: f32) -> (World, Entity) {
        let mut world = World::new();
        world.insert_resource(ground());
        world.insert_resource(GameTuning::default());
        world.insert_resource(SoundQueue::default());
        world.insert_resource(InputState::default());
        world.insert_resource(Loadout {
            equipped: weapon,
            cooldown: 0.0,
        });
        // The muzzle falls back to the player's chest at 1.2 m when no weapon
        // scene has loaded, so the body is centred on that to be shot at.
        let (_, height) = Kind::Slime.body();
        world.insert_resource(Aim::at(Vec3::Z, Vec3::new(0.0, 1.2, ahead)));
        world.spawn((Player, Transform::from_xyz(0.0, 0.0, 0.0)));
        let enemy = world
            .spawn((
                Enemy {
                    kind: Kind::Slime,
                    animation: Handle::default(),
                },
                Side::Hostile,
                Transform::from_xyz(0.0, 1.2 - height * 0.5, ahead),
            ))
            .id();
        (world, enemy)
    }

    fn pull_trigger(world: &mut World) {
        world.resource_mut::<InputState>().attack = true;
        world.run_system_once(fire).expect("fire could not run");
    }

    fn heard(world: &mut World) -> Vec<Sfx> {
        world
            .resource_mut::<SoundQueue>()
            .drain()
            .into_iter()
            .map(|event| event.sfx)
            .collect()
    }

    /// The whole point of the feature: point the pistol at something and pull
    /// the trigger, and it dies.
    #[test]
    fn a_shot_kills_what_it_is_pointed_at() {
        let (mut world, enemy) = range(Weapon::Pistol, 8.0);
        pull_trigger(&mut world);
        assert!(
            world.get_entity(enemy).is_err(),
            "the slime survived being shot"
        );
        let sounds = heard(&mut world);
        assert!(
            sounds.contains(&Sfx::Shoot),
            "the gun was silent: {sounds:?}"
        );
        assert!(sounds.contains(&Sfx::Defeat), "nothing died audibly");
    }

    /// And misses what it is not pointed at, which is the half that stops the
    /// test above passing for a gun that kills the whole field.
    #[test]
    fn a_shot_misses_what_it_is_not_pointed_at() {
        let (mut world, enemy) = range(Weapon::Pistol, 8.0);
        // Aim well off to the side, past the body's radius.
        world.insert_resource(Aim::at(
            Vec3::new(1.0, 0.0, 1.0).normalize(),
            Vec3::new(8.0, 1.2, 8.0),
        ));
        pull_trigger(&mut world);
        assert!(
            world.get_entity(enemy).is_ok(),
            "the slime died from a shot aimed elsewhere"
        );
    }

    /// Out past the pistol's range is out of the fight.
    #[test]
    fn a_shot_does_not_carry_past_its_range() {
        let range_of = shot_range(Weapon::Pistol.spec().shot.unwrap());
        let (mut world, enemy) = range(Weapon::Pistol, range_of + 20.0);
        pull_trigger(&mut world);
        assert!(
            world.get_entity(enemy).is_ok(),
            "the pistol reached past its own range"
        );
    }

    /// A held trigger fires at the weapon's rate rather than every tick.
    #[test]
    fn the_cooldown_paces_the_shots() {
        let (mut world, _) = range(Weapon::Pistol, 8.0);
        // Nothing to shoot at, so only the shots themselves are counted.
        world.insert_resource(Aim::at(Vec3::Z, Vec3::new(0.0, 1.2, 400.0)));
        let mut shots = 0;
        for _ in 0..30 {
            pull_trigger(&mut world);
            shots += heard(&mut world)
                .iter()
                .filter(|sfx| **sfx == Sfx::Shoot)
                .count();
        }
        // One second of holding it down, at an interval of 0.28 s.
        let interval = Weapon::Pistol.spec().interval;
        let expected = (1.0 / interval).floor() as usize;
        assert!(
            (expected..=expected + 1).contains(&shots),
            "{shots} shots in a second at a {interval}s interval"
        );
    }

    /// With the sword out the trigger is not this module's to take: the press
    /// must survive `fire` untouched for `player::movement` to swing on.
    #[test]
    fn a_drawn_sword_neither_fires_nor_eats_the_press() {
        let (mut world, enemy) = range(Weapon::Sword, 8.0);
        pull_trigger(&mut world);
        assert!(
            world.get_entity(enemy).is_ok(),
            "the sword shot something eight metres away"
        );
        assert!(
            world.resource::<InputState>().attack,
            "the sword's press was eaten by the gun"
        );
        assert!(heard(&mut world).is_empty(), "the sword made a gun noise");
    }

    /// The projectile path kills too, and takes time doing it -- which is the
    /// only reason to have it as well as hitscan.
    #[test]
    fn a_projectile_flies_to_its_target_and_kills_it() {
        let (mut world, enemy) = range(Weapon::Pistol, 20.0);
        world.resource_mut::<GameTuning>().gun_projectile = 1.0;
        pull_trigger(&mut world);
        assert!(
            world.get_entity(enemy).is_ok(),
            "the projectile arrived on the tick it was fired"
        );
        let mut ticks = 0;
        while world.get_entity(enemy).is_ok() && ticks < 90 {
            world.run_system_once(fly).expect("fly could not run");
            ticks += 1;
        }
        assert!(
            world.get_entity(enemy).is_err(),
            "the bullet never arrived after {ticks} ticks"
        );
        assert!(ticks > 1, "the bullet did not take any time to travel");
    }

    #[test]
    fn the_weapons_cycle_and_come_back_round() {
        let mut weapon = Weapon::default();
        for _ in 0..Weapon::ALL.len() {
            weapon = weapon.next();
        }
        assert_eq!(weapon, Weapon::default(), "cycling did not come back round");
        assert!(!Weapon::Sword.is_ranged(), "the sword shoots");
        assert!(Weapon::Pistol.is_ranged(), "the pistol does not shoot");
    }

    /// Every weapon with a model names one the pipeline actually builds. A
    /// typo here is a weapon that is invisible in the hand and says nothing
    /// about why.
    #[test]
    fn every_weapon_model_is_a_file_that_exists() {
        for weapon in Weapon::ALL {
            let Some(model) = weapon.spec().model else {
                continue;
            };
            let file = model.split('#').next().expect("a path before the #");
            let path = crate::asset_path().join(file);
            assert!(path.is_file(), "{:?} has no {}", weapon, path.display());
        }
    }

    /// The shot leaves down the barrel: straight at a body dead ahead, and
    /// past one off to the side.
    #[test]
    fn a_ray_hits_the_body_it_is_pointed_at() {
        let origin = Vec3::ZERO;
        let direction = Vec3::Z;
        // A slime-sized body four metres ahead.
        let hit = ray_hits_body(
            origin,
            direction,
            60.0,
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::Y,
            0.7,
            1.0,
        );
        assert!(hit.is_some(), "missed a body straight ahead");
        assert!(
            (hit.unwrap() - 4.0).abs() < 0.75,
            "hit reported at {:?} rather than about four metres",
            hit
        );
        // The same body, moved two metres to the side of a 0.7 m radius.
        assert!(
            ray_hits_body(
                origin,
                direction,
                60.0,
                Vec3::new(2.0, 0.0, 4.0),
                Vec3::Y,
                0.7,
                1.0
            )
            .is_none(),
            "hit a body it should have gone past"
        );
    }

    /// Behind the shooter is not in front of him, and neither is past the end
    /// of the shot's range.
    #[test]
    fn a_ray_reaches_neither_backwards_nor_past_its_range() {
        for body in [Vec3::new(0.0, 0.0, -4.0), Vec3::new(0.0, 0.0, 90.0)] {
            assert!(
                ray_hits_body(Vec3::ZERO, Vec3::Z, 60.0, body, Vec3::Y, 0.7, 1.0).is_none(),
                "reached {body:?}"
            );
        }
    }

    /// A body's height counts, not just the point its transform sits on: a
    /// shot at head height on a tall enemy has to connect.
    #[test]
    fn a_ray_hits_a_body_along_its_whole_height() {
        // Aimed level at 1.4 m, at a two-metre body whose base is on the floor.
        let hit = ray_hits_body(
            Vec3::new(0.0, 1.4, 0.0),
            Vec3::Z,
            60.0,
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::Y,
            0.4,
            2.0,
        );
        assert!(hit.is_some(), "shot over a body it should have hit");
        // And clear over the top of it is still a miss.
        assert!(
            ray_hits_body(
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::Z,
                60.0,
                Vec3::new(0.0, 0.0, 5.0),
                Vec3::Y,
                0.4,
                2.0
            )
            .is_none(),
            "hit a body it passed well above"
        );
    }

    /// A crawler on a ceiling hangs *below* its own base, so its body is
    /// measured down its own up-vector. Tested the same way `enemy::combat`
    /// reasons about the case.
    #[test]
    fn a_bodys_height_follows_the_way_its_model_is_stood() {
        let base = Vec3::new(0.0, 4.0, 5.0);
        let hanging = -Vec3::Y;
        // Level with where it hangs to, a metre under its base.
        let hit = ray_hits_body(
            Vec3::new(0.0, 3.2, 0.0),
            Vec3::Z,
            60.0,
            base,
            hanging,
            0.4,
            1.5,
        );
        assert!(hit.is_some(), "missed a body hanging from a ceiling");
    }

    /// Both resolutions are reachable, and flipping between them keeps the
    /// weapon's own range. Without this the projectile half of the module is
    /// code that compiles and has never run.
    #[test]
    fn the_console_can_put_a_hitscan_gun_on_the_projectile_path() {
        let pistol = Weapon::Pistol.spec().shot.expect("the pistol shoots");
        let mut tuning = GameTuning::default();
        assert!(
            matches!(as_fired(pistol, &tuning), Shot::Hitscan { .. }),
            "the pistol does not default to hitscan"
        );
        tuning.gun_projectile = 1.0;
        tuning.bullet_speed = 90.0;
        match as_fired(pistol, &tuning) {
            Shot::Projectile { speed, range } => {
                assert_eq!(speed, 90.0);
                assert_eq!(
                    range,
                    shot_range(pistol),
                    "swapping the resolution changed how far the gun carries"
                );
            }
            other => panic!("stayed on {other:?}"),
        }
    }

    /// The shot converges on what the crosshair is over rather than running
    /// parallel to the camera from a muzzle held off to one side.
    #[test]
    fn a_shot_is_laid_on_the_aim_point() {
        let aim = Aim::at(Vec3::Z, Vec3::new(0.0, 0.0, 30.0));
        // The gun is held half a metre to the right of the camera's line.
        let (origin, direction) = lay(Vec3::new(0.5, 0.0, 0.0), &aim);
        assert_eq!(origin, Vec3::new(0.5, 0.0, 0.0));
        let landed = origin + direction * (aim.point - origin).length();
        assert!(
            (landed - aim.point).length() < 1e-3,
            "the shot landed at {landed:?} rather than on the aim point"
        );
    }

    /// A camera ray that hit a wall between the eye and the Hero leaves an aim
    /// point *behind* the muzzle, and firing backwards at it would be worse
    /// than ignoring it.
    #[test]
    fn a_shot_never_turns_round_to_reach_the_aim_point() {
        let aim = Aim::at(Vec3::Z, Vec3::new(0.0, 0.0, -3.0));
        let (_, direction) = lay(Vec3::ZERO, &aim);
        assert!(
            direction.dot(aim.direction) > 0.999,
            "the shot went backwards: {direction:?}"
        );
    }
}
