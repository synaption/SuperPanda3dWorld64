//! Which level is up, and everything that has to happen to change it.
//!
//! The port used to have one level, built once in `main::setup` and never
//! taken down again. That is why this module exists rather than a few more
//! lines there: a second level is not a second `spawn` call, it is the
//! discovery that the first one had no matching `despawn`, that the collision
//! and the navigation field and the water and the respawn point were all
//! constants, and that "up" was the letter `Y`.
//!
//! So a level here is three things: what to draw, what to collide with, and
//! which way gravity points. [`LevelId`] names them, [`switch`] takes one down
//! and puts the next one up, and everything a level owns carries
//! [`LevelEntity`] so that taking it down is one query.
//!
//! The planet is the awkward one and shapes the design. Its collision is its
//! render mesh -- 786,432 triangles across 96 tiles, which is far too much to
//! embed the way `assets/bevy/castle.bin` is embedded -- so it is read back out
//! of the glTF once Bevy has finished loading it. That cannot happen in the
//! frame the level was chosen, which is why loading is a state
//! ([`LevelLoad`]) rather than a function call, and why the pause menu stays up
//! saying so until the ground exists to stand on.

use crate::{
    console::ConsoleState,
    enemy,
    flow::FlowField,
    furniture,
    gravity::Gravity,
    health,
    level::{self, LevelData},
    nuclonium, pipe, player, pylon, squad, stellarator, structure, water, weapon,
};
use bevy::{
    gltf::{Gltf, GltfMesh, GltfNode},
    mesh::{Indices, VertexAttributeValues},
    prelude::*,
    world_serialization::WorldAssetRoot,
};

/// The levels the pause menu offers.
///
/// An enum rather than a table read off disk: there are three of them, each
/// needs its own spawning code anyway, and a level that is only data is a level
/// this port cannot yet describe.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LevelId {
    /// The castle grounds. Flat, and everything else in the game is here.
    #[default]
    Castle,
    /// The generated planet, from `experimental/planet_gen`. Round, and
    /// gravity points at the middle of it.
    Planet,
    /// The solar system: a sun you can fly to, and the same planet standing
    /// twice, each copy genuinely running its own circle round the sun and
    /// turning on its own axis -- [`crate::orbit`] is the clockwork, and the
    /// `planet1_*`/`planet2_*` console rows are its dials. Whoever is inside
    /// a world's gravity rides that world's frame; the space between is
    /// weightless, inertial, and crossed on the booster (`infinite_thrust 1`
    /// takes its limits off) or on the autopilot (Tab, then X to pick a
    /// destination).
    PlanetOrbit,
}

impl LevelId {
    /// In the order the menu lists them.
    pub const ALL: [LevelId; 3] = [LevelId::Castle, LevelId::Planet, LevelId::PlanetOrbit];

    pub fn name(self) -> &'static str {
        match self {
            LevelId::Castle => "Castle grounds",
            LevelId::Planet => "Planet",
            LevelId::PlanetOrbit => "Solar system",
        }
    }

    /// The glTF drawn for it, under `assets/`.
    pub fn scene(self) -> &'static str {
        match self {
            LevelId::Castle => "bevy/castle.glb",
            // One file for both planets: the orbiting one differs only in what
            // the sky does over it, and a second 14 MB copy of the same ground
            // would be the packaging step's most expensive way to say so.
            LevelId::Planet | LevelId::PlanetOrbit => "bevy/planet.glb",
        }
    }
}

/// Asked for a level. Written by the pause menu, read by [`switch`].
///
/// A message rather than the menu doing the work, because changing level
/// despawns most of the world and the menu is not the place to be holding that
/// many queries.
#[derive(Message)]
pub struct LoadLevel(pub LevelId);

/// Marks everything the current level owns.
///
/// Everything spawned by [`spawn`] carries it. What does *not* is everything
/// that outlives a level -- the player and his two models, the cameras, the
/// HUD, the console, the menu, the whistle ring -- which is the same list as
/// "things `main::setup` spawns and this module never touches".
#[derive(Component)]
pub struct LevelEntity;

/// Where the player starts on this level, and goes back to on falling out of
/// the world.
///
/// A resource because it is a property of the level rather than of the player,
/// and because it was three copies of the same literal in [`crate::player`]
/// before there was a second level to have a different one.
#[derive(Resource, Clone, Copy)]
pub struct Respawn(pub Vec3);

impl Default for Respawn {
    fn default() -> Self {
        Self(castle_spawn())
    }
}

/// The spot on the castle path the game starts on.
///
/// A function and not the constant it was, because the spot is an empty called
/// `spawn` in `assets/levels/castle.blend` now -- see [`crate::furniture`] --
/// and a level you can edit is worth more than a `const`.
pub fn castle_spawn() -> Vec3 {
    furniture::castle().spawn()
}

/// The planet's collision is read out of its render mesh, which takes as many
/// frames as Bevy needs to load 14 MB of glTF. This is that wait.
#[derive(Resource, Default)]
pub struct LevelLoad {
    /// The level being brought up, or `None` when one is up.
    pub pending: Option<LevelId>,
    /// The glTF the collision is being read out of.
    handle: Handle<Gltf>,
    /// Why the last attempt did not work, if it did not.
    ///
    /// Kept so the pause menu can say so. A level that fails to load is
    /// otherwise the quietest bug this game can have: the world it was going to
    /// replace is still there, the menu shuts as though nothing happened, and
    /// the row the player chose simply appears to do nothing. That is exactly
    /// what a packaged build with the glTF left out of it looks like.
    pub failed: Option<String>,
}

impl LevelLoad {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Why the last level did not come up, for the menu to show. Cleared by
    /// asking for another one.
    pub fn trouble(&self) -> Option<&str> {
        self.failed.as_deref()
    }
}

/// Where a planet's centre is. The generator writes its tiles about the
/// origin, and nothing moves them on the way in.
const PLANET_CENTRE: Vec3 = Vec3::ZERO;

/// How far above the ground the player is put down when a level comes up.
const DROP_IN: f32 = 0.5;

/// What the tree card is drawn at. Its mesh is in the units the decomp's
/// exporter wrote, 714 across and 809 tall, and this is the port's 1/100.
const TREE_SCALE: f32 = 0.01;

/// How tall a tree stands once it is drawn. Nothing about the disc is taken
/// from it except through [`crate::shadow::clearance`], which is the one thing
/// that reads a caster's height: it bounds how far the disc may be drawn in
/// front of where it lies, so that a shadow lifted clear of a hillside is
/// never lifted through the trunk standing on it.
const TREE_HEIGHT: f32 = 800.0 * TREE_SCALE;

/// How wide a tree's shadow is on the ground.
///
/// Measured off the decals it replaces -- see
/// [`crate::shadow::shed_baked`]. Each was a quad 8.2 metres across whose
/// picture is solid to four fifths of that and gone by the rim, so the dark
/// circle it drew was 3.9 metres from the trunk at its widest. This port draws
/// that diameter 30% smaller; its disc also fades over a wider band than the
/// original picture, keeping the reduced footprint soft at the edge.
const TREE_SHADOW_RADIUS: f32 = 3.9 * 0.70;

/// And how dark, which is 163 of 255: the alpha the decal's picture tops out
/// at, and well short of the 200 [`crate::shadow::SOLID`] gives a unit. A tree
/// with a Mario's shadow under it is a much darker thing than the original
/// ever put on that lawn.
const TREE_SHADOW_SOLIDITY: f32 = 163.0 / 255.0;

/// Puts a level up: its collision, its gravity, its scenery and its
/// inhabitants.
///
/// The castle is complete when this returns. The planet is not -- it has its
/// scene and its gravity, and [`finish_planet`] fills in the ground when the
/// glTF has finished loading.
pub fn spawn(
    id: LevelId,
    commands: &mut Commands,
    assets: &AssetServer,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    load: &mut LevelLoad,
) {
    commands.insert_resource(id);
    // A level that has no authored surfaces must not be handed the last
    // level's. `water::expect_surfaces` puts this back for the ones that do.
    commands.remove_resource::<water::PendingSurfaces>();
    let scenery = commands
        .spawn((
            LevelEntity,
            WorldAssetRoot(assets.load(format!("{}#Scene0", id.scene()))),
        ))
        .id();
    match id {
        LevelId::Castle => {
            let furniture = furniture::castle();
            let (collision, _) = level::load();
            commands.insert_resource(FlowField::new(&collision));
            water::spawn(commands, assets, meshes, materials, &collision);
            // The drawn surfaces -- the waterfall -- are meshes in a .glb, so
            // they arrive when it does. Everything else the furniture says is
            // known now, which is the whole reason the placements travel as
            // JSON and only the geometry travels as glTF.
            water::expect_surfaces(commands, assets, &furniture);
            for position in furniture.trees() {
                commands.spawn((
                    LevelEntity,
                    // bhvTree is CYLBOARD in the original: it turns to face the
                    // camera about the vertical. Without it the trees are flat
                    // cards seen from one fixed side, and the mesh is exactly
                    // zero thick, so from ninety degrees away there is nothing
                    // there at all.
                    crate::billboard::BillboardAxis,
                    crate::billboard::BillboardActor,
                    // The same disc every unit gets, in place of the shadow
                    // the level used to carry baked into its own mesh. See
                    // [`crate::shadow::shed_baked`], which drops that one.
                    crate::shadow::ShadowCaster {
                        radius: TREE_SHADOW_RADIUS,
                        height: TREE_HEIGHT,
                        solidity: TREE_SHADOW_SOLIDITY,
                    },
                    WorldAssetRoot(assets.load("actors/tree.glb#Scene0")),
                    Transform::from_translation(position).with_scale(Vec3::splat(TREE_SCALE)),
                ));
            }
            commands.insert_resource(collision);
            commands.insert_resource(furniture.gravity());
            commands.insert_resource(Respawn(furniture.spawn()));
            spawn_inhabitants(&furniture, commands, assets);
            load.pending = None;
        }
        LevelId::Planet | LevelId::PlanetOrbit => {
            // Empty collision and radial gravity from the first frame: the
            // world is already round while it loads, so nothing has to be
            // switched over a second time when the ground arrives.
            let empty = LevelData::planet(&[], &[], PLANET_CENTRE, 1.0, None);
            commands.insert_resource(FlowField::new(&empty));
            commands.insert_resource(empty);
            commands.insert_resource(Gravity::towards(PLANET_CENTRE));
            if id == LevelId::PlanetOrbit {
                // The clockwork starts over: fresh orbit angles, and no seat
                // for the respawn until the ground exists to sit it on.
                let system = crate::orbit::SolarSystem::default();
                // Both scene roots are named for the body they draw, so
                // [`crate::orbit::advance`] can keep each under its planet as
                // the planets go round. The second is the same glTF drawn
                // again, not a second 14 MB of anything; both collisions are
                // one filed set of triangles answered through each body's
                // frame ([`LevelData::place_planets`]).
                commands.entity(scenery).insert(crate::orbit::PlanetBody(0));
                commands.spawn((
                    LevelEntity,
                    crate::orbit::PlanetBody(1),
                    WorldAssetRoot(assets.load(format!("{}#Scene0", id.scene()))),
                    Transform::from_translation(system.bodies[1].centre),
                ));
                commands.insert_resource(system);
            }
            load.pending = Some(id);
            load.handle = assets.load(id.scene());
        }
    }
}

/// The enemies standing about and the warp pipes producing more of them:
/// everything the level has living on it.
///
/// The placements were an array here until they became empties in
/// `assets/levels/castle.blend`. What is left is what the game does with them,
/// which was never the part worth editing.
///
/// The pipes are drawn but not collided with: the level's own collision is
/// what the physics reads and nothing here adds to it, so a pipe is scenery
/// that you can walk through and that things come out of. Every pipe's
/// countdown runs at any distance, so a crowd is waiting when the player
/// arrives rather than only starting to fill then.
fn spawn_inhabitants(
    furniture: &furniture::Furniture,
    commands: &mut Commands,
    assets: &AssetServer,
) {
    // The phase is what keeps two of anything from moving in step, and it is
    // an index rather than a random number so a whole run stays reproducible
    // in a test. Counted across both lists for the same reason.
    for (phase, (kind, position)) in furniture.actors().into_iter().enumerate() {
        let phase = phase as f32;
        match kind {
            pipe::Spawn::Enemy(kind) => {
                enemy::spawn(commands, assets, kind, position, phase);
            }
            pipe::Spawn::Mario => {
                squad::spawn_ally(
                    commands,
                    assets,
                    crate::ActiveCharacter::Mario,
                    position,
                    phase,
                );
            }
        }
    }
    // The structures. A machine the level put here is the same machine the
    // build button puts here -- same spawn, same field, same footprint -- so it
    // is picked up by the console's `stellarator_*` sliders and cleared away by
    // a level change like any other. What the .blend adds is that one can be
    // standing there before anybody has pressed anything.
    for prop in furniture.props() {
        match prop.kind {
            furniture::PropKind::Stellarator => {
                stellarator::spawn(commands, assets, prop.at, prop.yaw, prop.scale);
            }
        }
    }
    for (index, pipe) in furniture.pipes().into_iter().enumerate() {
        // As a thing that can be fought: the size the .blend drew it at, which
        // is what the swing has to cover, and whose side it is on, which is what
        // decides who comes to swing.
        let (radius, height) = pipe::body(pipe.at.scale.x);
        commands.spawn((
            LevelEntity,
            // The enemy pipes have their interval overwritten from the console
            // every tick; the Mario pipe keeps the one the .blend gave it.
            pipe::WarpPipe::new(pipe.spawns, pipe.interval, index as f32),
            // A nest is an objective now rather than scenery. The squad comes
            // for a hostile pipe and the crowd comes for the Mario one, both
            // through `enemy::alert`, which asks nothing of a target but what
            // side it is on -- see [`pipe::Spawn::side`].
            pipe.spawns.side(),
            structure::Structure::new(radius, height),
            health::Health::new(health::WARP_PIPE_HEALTH),
            WorldAssetRoot(assets.load(format!("{}#Scene0", pipe::MODEL))),
            // The whole transform the .blend drew it with, scale included.
            // Nothing here corrects the model's size: `pipe::MODEL` is a three
            // metre warp pipe in its own file, so a pipe placed at Blender's
            // scale of one is a pipe the size a pipe is.
            pipe.at,
        ));
    }
}

/// Everything a level put into the world, so that changing level can take it
/// all out again.
///
/// Two halves, and the second is the one that is easy to forget. [`spawn`]
/// marks what it spawns, but most of what ends up in a level was not spawned by
/// it: enemies come out of pipes, allies come out of the squad, bullets come
/// out of the gun. Those are found by what they are instead.
#[allow(clippy::type_complexity)]
type LevelContents = Or<(
    With<LevelEntity>,
    With<enemy::Enemy>,
    With<squad::Ally>,
    With<stellarator::Stellarator>,
    // And what the player planted between the machines. The beams would in
    // fact clear themselves -- `pylon::draw` redraws the whole set the moment
    // the network changes, and every mast going away is a change -- but a mast
    // left standing is a mast standing in the sky over the next level, and
    // naming both here is one rule rather than two.
    With<pylon::Pylon>,
    With<pylon::Beam>,
    // And what the squad was carrying between the two. A ball is spawned by a
    // kill rather than by the level, so like the enemies it is found by what it
    // is; leaving one behind is a green ball hanging in the sky over the next
    // level, exactly as a mast would be.
    With<nuclonium::Nuclonium>,
    With<nuclonium::Shipment>,
    With<weapon::Bullet>,
    With<weapon::Tracer>,
    With<pipe::Launched>,
    With<water::WaterSurface>,
)>;

/// Takes the current level down and puts the asked-for one up.
///
/// Runs in the overlay rather than the simulation, because the menu that asks
/// for it is open at the time and the simulation is held still while it is.
#[allow(clippy::too_many_arguments)]
pub fn switch(
    mut requests: MessageReader<LoadLevel>,
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut load: ResMut<LevelLoad>,
    mut squad: ResMut<squad::Squad>,
    current: Res<LevelId>,
    contents: Query<Entity, LevelContents>,
    mut placement: ParamSet<(PlacePlayer<'_, '_>, PlaceCamera<'_, '_>)>,
) {
    // Only the last one asked for, and never while one is already coming up:
    // two levels half-loaded at once is two sets of scenery and one collision.
    let Some(LoadLevel(wanted)) = requests.read().last() else {
        return;
    };
    let wanted = *wanted;
    if load.busy() || wanted == *current {
        return;
    }
    load.failed = None;
    for entity in &contents {
        commands.entity(entity).despawn();
    }
    // The squad is a list of entity ids, and every one of them was just
    // despawned. Left alone it would spend the next level chasing the dead.
    squad.disband();
    spawn(
        wanted,
        &mut commands,
        &assets,
        &mut meshes,
        &mut materials,
        &mut load,
    );
    // A level that is ready now puts the player down now. The planet cannot --
    // there is nowhere to stand until its collision exists -- so it does it in
    // [`finish_planet`] instead. Without this the player arrives on the castle
    // still standing where he was on the planet, three hundred metres over a
    // level that is eighty across, and spends the next few seconds falling into
    // the void and being caught by the respawn.
    if !load.busy() {
        put_the_player_down(castle_spawn(), Vec3::Y, &mut commands, &mut placement);
    }
}

/// Moves the player, his interpolated pose and the camera behind him to `at`,
/// standing on ground whose up is `up`.
///
/// The camera is placed rather than eased. `camera::update` smooths its way to
/// where it wants to be, which is the right behaviour for a step and the wrong
/// one for a level change: eased across a planet it is several seconds of
/// flying over the terrain before the level starts.
fn put_the_player_down(
    at: Vec3,
    up: Vec3,
    commands: &mut Commands,
    placement: &mut ParamSet<(PlacePlayer<'_, '_>, PlaceCamera<'_, '_>)>,
) {
    // Any direction along the ground will do for a facing: on a planet there is
    // no north for a default to mean, and the player turns the moment he moves.
    let facing = up.any_orthonormal_vector();
    if let Ok((mut transform, mut previous, mut controller)) = placement.p0().single_mut() {
        *transform = Transform::from_translation(at).looking_to(facing, up);
        *previous = player::PreviousPose::new(&transform);
        controller.reset();
        commands.insert_resource(player::RenderPose {
            translation: transform.translation,
            rotation: transform.rotation,
        });
    }
    for (mut camera, mut follow) in &mut placement.p1() {
        follow.frame = Quat::from_rotation_arc(Vec3::Y, up);
        // Set, not eased. `view` normally lags `frame` on purpose, and the one
        // moment it must not is the moment it has no history worth keeping:
        // arriving on a planet with the view still holding the castle's `+Y`
        // would spend the first second of the level rolling the horizon over.
        follow.view = follow.frame;
        follow.clearance = 1.0;
        // The ride's memory goes with the history: a rotation remembered off
        // the previous level's clockwork, compared against a fresh one, would
        // spend the arrival frame un-turning a turn nobody made.
        follow.ridden = None;
        // Forgotten, this is the second-worst arrival the camera can make:
        // the focus is eased toward the player from wherever it last was,
        // and "wherever it last was" is the previous level -- which from a
        // planet's spawn is a point deep inside the planet. The camera spent
        // the first seconds of every visit flying through the core, underwater
        // as far as the fog could tell, with the whole sky hidden. Cleared,
        // the next `camera::update` starts it where it belongs.
        follow.focus = None;
        camera.translation = at + up * 3.0 - facing * follow.distance;
        camera.look_at(at + up, up);
    }
}

/// The player, and the camera behind him, put down where the level says.
///
/// A [`ParamSet`] because both want `&mut Transform` and Bevy will not take on
/// trust that the camera is never the player.
type PlacePlayer<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut player::PreviousPose,
        &'static mut player::Controller,
    ),
    With<player::Player>,
>;
type PlaceCamera<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut crate::camera::FollowCamera,
    ),
>;

/// Reads a planet's collision out of the glTF Bevy has finished loading, then
/// puts the player down on it.
///
/// Nothing happens until every mesh is in hand. A half-loaded planet would give
/// collision with holes in it, and a hole in a planet's collision is the player
/// falling to the core.
#[allow(clippy::too_many_arguments)]
pub fn finish_planet(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut load: ResMut<LevelLoad>,
    mut console: ResMut<ConsoleState>,
    mut levels: MessageWriter<LoadLevel>,
    gltfs: Res<Assets<Gltf>>,
    nodes: Res<Assets<GltfNode>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    meshes: Res<Assets<Mesh>>,
    gravity: Res<Gravity>,
    mut system: ResMut<crate::orbit::SolarSystem>,
    mut placement: ParamSet<(PlacePlayer<'_, '_>, PlaceCamera<'_, '_>)>,
) {
    let Some(id @ (LevelId::Planet | LevelId::PlanetOrbit)) = load.pending else {
        return;
    };
    // Waited on rather than timed out. A load that is merely slow -- 14 MB of
    // glTF off a cold disk -- is still going to arrive, and a wall clock is a
    // bad judge of that: it would give up on a slow machine and never fire on a
    // fast one. What is worth reacting to is the asset server saying it cannot
    // be done, which it says plainly.
    if let bevy::asset::LoadState::Failed(why) = assets.load_state(&load.handle) {
        let trouble = format!("{} did not load: {why}", id.scene());
        console.report(trouble.clone());
        load.failed = Some(trouble);
        load.pending = None;
        // Back to somewhere with ground in it, rather than leaving the player
        // hanging over a planet that is never going to exist.
        levels.write(LoadLevel(LevelId::Castle));
        return;
    }
    if !assets.is_loaded_with_dependencies(&load.handle) {
        return;
    }
    let Some(gltf) = gltfs.get(&load.handle) else {
        return;
    };
    let Some(geometry) = read_geometry(gltf, &nodes, &gltf_meshes, &meshes) else {
        return;
    };
    let (centre, radius) = sphere_of(&geometry.vertices);
    let sea = sea_level_of(&geometry.ocean, centre);
    let mut collision =
        LevelData::planet(&geometry.vertices, &geometry.indices, centre, radius, sea);
    // Sea level rather than the measured mean wherever there is a sea. The
    // mean sits four metres above the water on this planet -- it is an average
    // over land and seabed both -- and "the lowest dry land" measured against
    // it is land that is four metres dry, which is a beach the player never
    // arrives on.
    //
    // Searched in the geometry's own filed coordinates, before any orbital
    // placement: on the solar system the planets are already out on their
    // circles, and a search around the authored centre would be a search of
    // empty space.
    let spawn = ground_to_stand_on(&collision, centre, sea.unwrap_or(radius));
    // The orbiting level is a system: the same ground standing twice, each
    // copy where its orbit has it, gravity answering to whichever world is
    // nearer and to neither in the middle. Installed here rather than in
    // `spawn` because everything wants the measured radius, which did not
    // exist until this frame.
    let spawn = match id {
        LevelId::PlanetOrbit => {
            let placed: Vec<(Vec3, Quat)> = system
                .bodies
                .iter()
                .map(|body| (body.centre, body.rotation))
                .collect();
            collision.place_planets(&placed);
            commands.insert_resource(Gravity::binary(
                system.bodies[0].centre,
                system.bodies[1].centre,
                radius,
            ));
            // The respawn is a seat on the first planet rather than a
            // coordinate: `orbit::advance` re-resolves it as the planet goes
            // round.
            system.respawn_local = Some(spawn);
            let home = system.bodies[0];
            home.centre + home.rotation * (spawn - centre)
        }
        _ => spawn,
    };
    console.report(match sea {
        Some(sea) => format!(
            "planet: {} triangles, radius {radius:.0} m, sea level {sea:.0} m",
            geometry.indices.len()
        ),
        None => format!(
            "planet: {} triangles, radius {radius:.0} m, no sea",
            geometry.indices.len()
        ),
    });

    // Which way is up where he lands. The system's new gravity was inserted
    // through `Commands` and is not readable yet, so the answer is taken off
    // the body he is standing on rather than off the stale resource -- which
    // still points at the authored centre, a spot the planet has left.
    let up = match id {
        LevelId::PlanetOrbit => (spawn - system.bodies[0].centre).normalize_or(Vec3::Y),
        _ => gravity.up(spawn),
    };
    put_the_player_down(spawn, up, &mut commands, &mut placement);

    commands.insert_resource(FlowField::new(&collision));
    commands.insert_resource(Respawn(spawn));
    commands.insert_resource(collision);
    load.pending = None;
    load.handle = Handle::default();
}

/// What a planet's glTF is read for: the ground, and the sea over it.
#[derive(Default)]
struct Geometry {
    vertices: Vec<Vec3>,
    indices: Vec<[u32; 3]>,
    /// The sea-level sphere's vertices, kept apart from the collision they
    /// must never join. It is in the same file as the terrain because it is
    /// part of the same planet and the generator owns where sea level is; it
    /// is a separate node so that reading it as ground is a decision somebody
    /// has to make rather than the default.
    ocean: Vec<Vec3>,
}

/// The name a planet's sea goes by, in `planetgen`'s exporter and here.
///
/// Matched on the front of the name so that the LOD1 sphere -- `ocean_lod1` --
/// is the sea too. Getting this wrong is not a subtle bug in either direction:
/// a sea read as ground is a glass floor over the whole world, and ground read
/// as sea is a planet with no collision at all.
const OCEAN_NODE: &str = "ocean";

/// Every triangle in a glTF, welded end to end into one mesh in world space.
///
/// `None` while anything is still missing, which is what makes the caller wait
/// rather than build collision out of the tiles that happen to have arrived.
///
/// Walked as a node tree rather than as the flat list of meshes, so that a
/// node's transform is applied to what it holds. The planet the generator
/// writes has 96 nodes with no transform between them, so today this is the
/// same answer either way -- and the day one tile is moved, offset or scaled,
/// the difference is collision that no longer matches what is drawn, which is
/// the least debuggable class of bug this could have.
fn read_geometry(
    gltf: &Gltf,
    nodes: &Assets<GltfNode>,
    gltf_meshes: &Assets<GltfMesh>,
    meshes: &Assets<Mesh>,
) -> Option<Geometry> {
    // The roots are the nodes nobody claims as a child. `Gltf::nodes` is every
    // node in the file, parents and children alike, so walking it directly
    // would visit a child once on its own and once under its parent.
    let mut children = Vec::new();
    for handle in &gltf.nodes {
        children.extend(nodes.get(handle)?.children.iter().map(|child| child.id()));
    }
    let mut geometry = Geometry::default();
    for handle in &gltf.nodes {
        if children.contains(&handle.id()) {
            continue;
        }
        read_node(
            handle,
            Transform::IDENTITY,
            nodes,
            gltf_meshes,
            meshes,
            &mut geometry,
        )?;
    }
    Some(geometry)
}

/// Where the water's surface is, out of the sea's own vertices.
///
/// Every one of them is the same distance from the centre -- the generator
/// builds a sphere -- so the mean is that distance and not an estimate of it.
/// This is why the sea travels as geometry: `sea_level` is a number in
/// `planet.json`, the game does not read `planet.json`, and a shoreline drawn
/// four metres out is a shoreline in the wrong place.
fn sea_level_of(ocean: &[Vec3], centre: Vec3) -> Option<f32> {
    if ocean.is_empty() {
        return None;
    }
    let sum: f32 = ocean.iter().map(|&vertex| (vertex - centre).length()).sum();
    Some(sum / ocean.len() as f32)
}

/// One node and everything under it, with `parent` already applied.
fn read_node(
    handle: &Handle<GltfNode>,
    parent: Transform,
    nodes: &Assets<GltfNode>,
    gltf_meshes: &Assets<GltfMesh>,
    meshes: &Assets<Mesh>,
    into: &mut Geometry,
) -> Option<()> {
    let node = nodes.get(handle)?;
    let here = parent * node.transform;
    if let Some(mesh_handle) = &node.mesh {
        let sea = node.name.starts_with(OCEAN_NODE);
        for primitive in &gltf_meshes.get(mesh_handle)?.primitives {
            let mesh = meshes.get(&primitive.mesh)?;
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                continue;
            };
            if sea {
                // Read for its radius alone. The sea is drawn by the scene
                // this same glTF spawns, and stood on by nobody.
                into.ocean
                    .extend(positions.iter().map(|p| here * Vec3::from(*p)));
                continue;
            }
            // Each tile brings its own vertex array, so every tile's indices
            // are offset past the tiles already read. The tiles do share a
            // boundary ring by value rather than by index; the duplicates cost
            // twelve bytes each and no correctness.
            let base = into.vertices.len() as u32;
            into.vertices
                .extend(positions.iter().map(|p| here * Vec3::from(*p)));
            let corners: Vec<u32> = match mesh.indices() {
                Some(Indices::U16(values)) => values.iter().map(|&i| base + i as u32).collect(),
                Some(Indices::U32(values)) => values.iter().map(|&i| base + i).collect(),
                // An unindexed primitive is three vertices to a triangle in
                // order, which is what the glTF spec says it is.
                None => (base..base + positions.len() as u32).collect(),
            };
            into.indices
                .extend(corners.chunks_exact(3).map(|tri| [tri[0], tri[1], tri[2]]));
        }
    }
    for child in &node.children {
        read_node(child, here, nodes, gltf_meshes, meshes, into)?;
    }
    Some(())
}

/// The middle of a planet and how big it is, measured off its own geometry.
///
/// The centre is the middle of the bounding box and the radius is the mean
/// distance out to the surface, which on terrain that rises and falls either
/// side of sea level lands near sea level. Measured rather than read out of
/// `planet.json` so that the game needs one file for the planet and not two,
/// and so that a regenerated planet at a different scale needs no edit here.
/// Nothing depends on it being exact: it is what "above sea level" is judged
/// against when choosing where to put the player down, and where the bottom of
/// the world is for the fell-out-of-it test.
fn sphere_of(vertices: &[Vec3]) -> (Vec3, f32) {
    if vertices.is_empty() {
        return (PLANET_CENTRE, 1.0);
    }
    let (low, high) = vertices.iter().fold(
        (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
        |(low, high), &vertex| (low.min(vertex), high.max(vertex)),
    );
    let centre = (low + high) * 0.5;
    let radius = vertices
        .iter()
        .map(|&vertex| (vertex - centre).length())
        .sum::<f32>()
        / vertices.len() as f32;
    (centre, radius.max(1.0))
}

/// Somewhere on a planet to put the player down: the lowest dry land it can
/// find.
///
/// Directions are taken off a Fibonacci spiral rather than a latitude grid, so
/// the candidates are spread evenly over the sphere instead of bunching at the
/// poles.
///
/// Land is measured against `sea_level` rather than against the mean radius,
/// because on a planet with an ocean they are not the same number and the
/// difference is the beach.
///
/// The *lowest* land rather than the first, which is one comparison and worth
/// it. The first is wherever the spiral happens to start, and on this planet
/// that was a glacier: a mountaintop is the worst place to arrive on a world
/// you are meant to walk around, and a white one is the worst place to
/// photograph it from. Lowland is flat, walkable, and next to the sea.
fn ground_to_stand_on(collision: &LevelData, centre: Vec3, sea_level: f32) -> Vec3 {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    const CANDIDATES: usize = 256;
    let mut lowest_land: Option<(f32, Vec3)> = None;
    let mut anywhere: Option<Vec3> = None;
    for step in 0..CANDIDATES {
        let height = 1.0 - 2.0 * (step as f32 + 0.5) / CANDIDATES as f32;
        let ring = (1.0 - height * height).max(0.0).sqrt();
        let yaw = golden * step as f32;
        let up = Vec3::new(ring * yaw.cos(), height, ring * yaw.sin()).normalize();
        let from = centre + up * (sea_level + 60.0);
        let Some((point, _)) = collision.ground_below(from, up) else {
            continue;
        };
        let standing = point + up * DROP_IN;
        anywhere.get_or_insert(standing);
        let altitude = (point - centre).length() - sea_level;
        if altitude > 0.0 && lowest_land.is_none_or(|(best, _)| altitude < best) {
            lowest_land = Some((altitude, standing));
        }
    }
    // No land, or no collision at all. Above the surface either way, so the
    // fall is short and onto something rather than through everything.
    lowest_land
        .map(|(_, standing)| standing)
        .or(anywhere)
        .unwrap_or(centre + Vec3::Y * (sea_level + DROP_IN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{level::Shape, player::Controller};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::gltf::GltfPlugin;

    /// The real game, headless, with the glTF loader bolted on.
    ///
    /// [`crate::tests::headless`] deliberately has no loader: every system that
    /// waits on an asset is exercised down its not-ready path there, which is
    /// the path the real game takes on its first frames. This test is the
    /// opposite one -- its whole subject is what happens when the asset does
    /// arrive -- so it takes that same game and adds what the loader needs.
    fn with_a_loader() -> App {
        let mut app = crate::tests::headless();
        app.add_plugins((
            bevy::scene::ScenePlugin,
            bevy::world_serialization::WorldSerializationPlugin,
            bevy::image::ImagePlugin::default(),
            bevy::animation::AnimationPlugin,
            GltfPlugin::default(),
        ))
        // Reached for by the glTF loader whenever a mesh has a skeleton, which
        // every actor in this game does. `bevy_render` provides it in the real
        // build; here it would be an unexplained panic on an IO thread.
        .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>();
        // `GltfPlugin` registers its loader in `finish` rather than in `build`,
        // and `App::update` does not call it -- only `App::run` does. Without
        // this every asset sits at `Loading` for ever with nothing to say why.
        app.finish();
        app.cleanup();
        app
    }

    /// A machine placed in the .blend is standing on the level when it comes
    /// up, at the size the level drew it -- footprint included, which is the
    /// half that is not merely visual: two machines that overlap is the one
    /// thing the build button refuses to do, and a placed one has to be in that
    /// conversation.
    #[test]
    fn a_machine_placed_in_a_level_is_standing_there_when_it_comes_up() {
        let level: furniture::Furniture = serde_json::from_str(
            r#"{"level":"test","spawn":[0,0,0],"gravity":{"mode":"down"},
                "props":[{"kind":"stellarator","at":[7,2,-3],
                          "yaw":1.5707963,"scale":0.75}]}"#,
        )
        .expect("that should parse");
        let mut app = crate::tests::headless();
        app.world_mut()
            .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                spawn_inhabitants(&level, &mut commands, &assets);
            })
            .expect("that should run");
        app.update();
        let mut machines = app
            .world_mut()
            .query::<(&stellarator::Stellarator, &Transform)>();
        let found: Vec<_> = machines
            .iter(app.world())
            .map(|(machine, at)| (machine.radius, *at))
            .collect();
        assert_eq!(found.len(), 1, "one machine, not {}", found.len());
        let (radius, at) = found[0];
        assert!((at.translation - Vec3::new(7.0, 2.0, -3.0)).length() < 1e-4);
        assert!((at.scale - Vec3::splat(0.75)).length() < 1e-6);
        // A quarter turn about the vertical takes the model's +Z to +X.
        assert!((at.rotation * Vec3::Z - Vec3::X).length() < 1e-4);
        assert!(
            (radius - stellarator::footprint(0.75)).abs() < 1e-4,
            "it stands on {radius} m rather than the {} it was drawn at",
            stellarator::footprint(0.75)
        );
    }

    /// What a player built goes with the level he built it on.
    ///
    /// `LevelContents` is the one list that says what a level change takes
    /// away, and the way it goes wrong is by omission: a thing spawned by a
    /// system rather than by `spawn` is not marked, so it is only taken out if
    /// somebody remembered to name it here. A mast that was forgotten is a mast
    /// standing in the sky over the planet, several hundred metres from the
    /// ground, with its beams still strung between where the castle used to
    /// be.
    #[test]
    fn what_the_player_planted_is_taken_down_with_the_level() {
        let mut world = World::new();
        let mast = world.spawn(pylon::Pylon { radius: 1.0 }).id();
        let beam = world.spawn(pylon::Beam).id();
        let machine = world.spawn(stellarator::Stellarator { radius: 2.0 }).id();
        // Something that belongs to nobody, to prove the filter is a filter.
        let bystander = world.spawn(Transform::default()).id();
        let taken: Vec<Entity> = world
            .query_filtered::<Entity, LevelContents>()
            .iter(&world)
            .collect();
        for entity in [mast, beam, machine] {
            assert!(taken.contains(&entity), "{entity} would survive the switch");
        }
        assert!(!taken.contains(&bystander), "the filter takes everything");
    }

    /// The whole feature, end to end, on the real `assets/bevy/planet.glb`: ask
    /// for the planet from a running game, wait for it, and then stand on it.
    ///
    /// This is the test that would have caught every mistake worth making
    /// here -- collision read out of the wrong buffer, a face grid with holes in
    /// it, gravity still pointing at `-Y` -- and none of them are visible in a
    /// unit test of any one piece.
    #[test]
    fn the_planet_loads_and_the_player_stands_on_it() {
        let mut app = with_a_loader();
        app.update();
        assert_eq!(*app.world().resource::<LevelId>(), LevelId::Castle);

        app.world_mut().write_message(LoadLevel(LevelId::Planet));
        // Bounded by the wall clock rather than by frames: this reads 33 MB of
        // glTF off disk, and how many frames that takes is a property of the
        // machine. A cap on a hang, not a deadline -- it takes tens of
        // milliseconds.
        let started = std::time::Instant::now();
        let mut frames = 0;
        while app.world().resource::<LevelLoad>().busy() || frames == 0 {
            app.update();
            frames += 1;
            assert!(
                started.elapsed().as_secs() < 60,
                "the planet never finished loading"
            );
        }
        assert_eq!(*app.world().resource::<LevelId>(), LevelId::Planet);

        // Gravity points at the middle of it, which is the thing asked for.
        let gravity = *app.world().resource::<Gravity>();
        assert_eq!(gravity, Gravity::towards(PLANET_CENTRE));

        // And the collision is the real generated terrain rather than the empty
        // stand-in the level came up with.
        let Shape::Planet { centre, radius } = app.world().resource::<LevelData>().shape() else {
            panic!("the planet's collision is still flat");
        };
        assert!(
            (550.0..650.0).contains(&radius),
            "planet.glb measured {radius} m across the middle"
        );

        let start = app.world().resource::<Respawn>().0;
        let up = gravity.up(start);

        // The planet has a sea, and the player is on the dry side of it.
        //
        // Measured against the water rather than against `radius`, which is
        // the mean distance to the surface and sits eight metres above the
        // waterline on this planet -- an average taken over the mountains and
        // the seabed alike.
        let level = app.world().resource::<LevelData>();
        let sea = level
            .sea_radius()
            .expect("planet.glb has no ocean node, so there is no sea");
        assert!(
            (550.0..650.0).contains(&sea),
            "the sea came out at r={sea} m round a planet of r={radius} m"
        );
        let depth = level.water_depth(start).unwrap();
        assert!(
            depth < 0.0,
            "the player was put down {depth} m under the sea"
        );
        assert!(
            depth > -10.0,
            "the player was put down {} m above the water, which is a hillside \
             and not the shore the spawn search is for",
            -depth
        );

        // The sea is drawn and not stood on. Were the ocean node read as
        // collision, the lowest ground anywhere on the planet would be the
        // water's surface: a glass floor over every basin, and the seabed
        // sealed underneath it.
        let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
        let mut deepest = f32::INFINITY;
        for step in 0..256 {
            let height = 1.0 - 2.0 * (step as f32 + 0.5) / 256.0;
            let ring = (1.0 - height * height).max(0.0).sqrt();
            let yaw = golden * step as f32;
            let probe = Vec3::new(ring * yaw.cos(), height, ring * yaw.sin()).normalize();
            if let Some((point, _)) = level.ground_below(centre + probe * (sea + 60.0), probe) {
                deepest = deepest.min((point - centre).length());
            }
        }
        assert!(
            deepest < sea - 5.0,
            "the deepest ground on the planet is at r={deepest} m against a sea \
             at r={sea} m, which is the ocean being walked on"
        );
        assert!(up.dot(Vec3::Y).abs() < 0.999, "up is still the world's up");

        // Now play. Forward for two seconds of fixed steps, which at 30 Hz and
        // 16 ms a frame is sixty frames' worth.
        app.world_mut()
            .resource_mut::<crate::input::InputState>()
            .move_axis = Vec2::new(0.0, 1.0);
        for _ in 0..120 {
            app.update();
        }
        let mut players = app
            .world_mut()
            .query_filtered::<(&Transform, &Controller), With<player::Player>>();
        let (transform, controller) = players.single(app.world()).expect("no player");
        let at = transform.translation;
        let out = (at - centre).length();
        assert!(
            (out - (start - centre).length()).abs() < 30.0,
            "the player left the surface: {out} m from the middle, having started at {}",
            (start - centre).length()
        );
        assert!(controller.grounded, "the player never landed on the planet");
        // He walked, and he walked *around* it rather than along a straight
        // line through it: the up under his feet has turned.
        let travelled = (at - start).length();
        assert!(travelled > 3.0, "the player only moved {travelled} m");
        let turned = gravity.up(at).dot(up);
        assert!(turned < 1.0 - 1e-7, "up did not turn under him at all");
        // And he is standing up in the new frame rather than leaning over.
        assert!(
            (transform.rotation * Vec3::Y).dot(gravity.up(at)) > 0.99,
            "the player is not perpendicular to the ground he is standing on"
        );

        // The sea is in the scene and moving. Both halves matter: the node has
        // to have been found by its name, and the drift has to have turned it,
        // a turn being the only motion an ocean has.
        let mut seas = app
            .world_mut()
            .query_filtered::<&Transform, With<crate::water::Ocean>>();
        let sea_transform = seas
            .single(app.world())
            .expect("the planet's scene has no ocean in it");
        assert!(
            sea_transform.rotation.angle_between(Quat::IDENTITY) > 1e-5,
            "the sea never drifted: {:?}",
            sea_transform.rotation
        );
    }

    /// The orbiting planet is the same ground down the same load path -- what
    /// it adds is all in the sky, which `sky::tests` holds. What this holds is
    /// that adding it broke nothing on the way to standing on it: the widened
    /// gate in [`finish_planet`] has to recognise the new id, or the level
    /// hangs on the pause menu's "loading" for ever.
    #[test]
    fn the_orbiting_planet_loads_down_the_same_path() {
        let mut app = with_a_loader();
        app.update();
        app.world_mut().write_message(LoadLevel(LevelId::PlanetOrbit));
        let started = std::time::Instant::now();
        let mut frames = 0;
        while app.world().resource::<LevelLoad>().busy() || frames == 0 {
            app.update();
            frames += 1;
            assert!(
                started.elapsed().as_secs() < 60,
                "the orbiting planet never finished loading"
            );
        }
        assert_eq!(*app.world().resource::<LevelId>(), LevelId::PlanetOrbit);
        // A system's gravity, not a lone planet's: one centre per orbiting
        // world, exactly where the clockwork has it, and weightlessness in
        // the middle of the crossing.
        let system_centres = app.world().resource::<crate::orbit::SolarSystem>().centres();
        let gravity = *app.world().resource::<Gravity>();
        let Gravity::System { centres, .. } = gravity else {
            panic!("the orbiting level's gravity is {gravity:?}, not a system");
        };
        assert_eq!(centres, system_centres);
        assert!(
            (centres[0] - crate::orbit::SUN_CENTRE).length() > 1000.0,
            "the first planet is not out on its orbit"
        );
        let midway = (centres[0] + centres[1]) * 0.5;
        assert!(
            gravity.weightless(midway),
            "the middle of the crossing still pulls"
        );
        let Shape::Planet { radius, .. } = app.world().resource::<LevelData>().shape() else {
            panic!("the orbiting planet's collision is still flat");
        };
        assert!(
            (550.0..650.0).contains(&radius),
            "planet.glb measured {radius} m across the middle"
        );

        // The second world is really there, twice over: its ground answers
        // where its orbit has it, and its scene is drawn there. Probed from
        // sixty metres up, the same clearance the spawn search uses: `radius`
        // is a mean and the terrain rises tens of metres over it, so a probe
        // from one metre up can begin inside a mountain.
        let level = app.world().resource::<LevelData>();
        let over_the_copy = centres[1] + Vec3::Y * (radius + 60.0);
        let (ground, _) = level
            .ground_below(over_the_copy, Vec3::Y)
            .expect("no ground on the second planet");
        assert!(
            ((ground - centres[1]).length() - radius).abs() < 60.0,
            "the copy's ground is {} m from its middle",
            (ground - centres[1]).length()
        );
        let mut roots = app
            .world_mut()
            .query_filtered::<&Transform, With<bevy::world_serialization::WorldAssetRoot>>();
        assert!(
            roots
                .iter(app.world())
                .any(|at| (at.translation - centres[1]).length() < radius),
            "no scene stands under the second planet"
        );
    }

    /// The player arrives on the system with a sky over him and a camera
    /// behind him -- immediately, not after a scenic tour of the core.
    ///
    /// The regression this pins down: `put_the_player_down` reset everything
    /// about the camera except its eased focus, so arriving from the castle
    /// left the focus at the castle's coordinates -- which, from a planet's
    /// surface, is a point deep inside the planet. The camera then spent
    /// several seconds easing out through rock and sea, the fog read it as
    /// underwater the whole way, and the medium check hid the sun, the moon
    /// and the stars. From the player's chair: no sky, and a camera that
    /// arrives in his head.
    #[test]
    fn arriving_on_the_system_keeps_the_camera_out_of_the_core() {
        let mut app = with_a_loader();
        app.update();
        app.world_mut().write_message(LoadLevel(LevelId::PlanetOrbit));
        let started = std::time::Instant::now();
        let mut frames = 0;
        while app.world().resource::<LevelLoad>().busy() || frames == 0 {
            app.update();
            frames += 1;
            assert!(started.elapsed().as_secs() < 60, "the system never loaded");
        }
        // Against the player rather than against the spawn point: the planet
        // is genuinely moving now, and it carries him with it.
        for step in 0..10 {
            app.update();
            let mut players = app
                .world_mut()
                .query_filtered::<&Transform, With<player::Player>>();
            let standing = players.single(app.world()).unwrap().translation;
            let mut cameras = app
                .world_mut()
                .query_filtered::<&Transform, With<Camera3d>>();
            let camera = cameras.single(app.world()).unwrap().translation;
            assert!(
                (camera - standing).length() < 25.0,
                "frame {step}: the camera is {} m from the player, touring the core",
                (camera - standing).length()
            );
        }
        assert!(
            !app.world()
                .resource::<crate::water::CameraMedium>()
                .submerged(),
            "the camera arrived underwater, which is the hidden-sky bug"
        );
        // And the sun is *there*: over the system the billboard disc stands
        // down and the physical sphere at the middle of the system stands
        // up, which is the body a player can actually fly to.
        let mut parts = app
            .world_mut()
            .query::<(&crate::sky::SkyPart, &Visibility)>();
        let mut seen = std::collections::HashMap::new();
        for (part, visibility) in parts.iter(app.world()) {
            seen.insert(*part, *visibility);
        }
        assert_eq!(
            seen.get(&crate::sky::SkyPart::SunBody),
            Some(&Visibility::Visible),
            "the real sun is not in the sky"
        );
        assert_eq!(
            seen.get(&crate::sky::SkyPart::Sun),
            Some(&Visibility::Hidden),
            "the billboard sun is still up beside the real one"
        );
    }

    /// Going back is the other half of the feature, and the half where the
    /// player is left standing three hundred metres in the air if nobody moves
    /// him.
    #[test]
    fn going_back_to_the_castle_puts_the_player_back_on_it() {
        let mut app = with_a_loader();
        app.update();
        app.world_mut().write_message(LoadLevel(LevelId::Planet));
        let started = std::time::Instant::now();
        let mut frames = 0;
        while app.world().resource::<LevelLoad>().busy() || frames == 0 {
            app.update();
            frames += 1;
            assert!(started.elapsed().as_secs() < 60, "the planet never loaded");
        }

        app.world_mut().write_message(LoadLevel(LevelId::Castle));
        app.update();
        assert_eq!(*app.world().resource::<LevelId>(), LevelId::Castle);
        assert!(!app.world().resource::<LevelLoad>().busy());
        assert_eq!(*app.world().resource::<Gravity>(), Gravity::default());
        assert_eq!(app.world().resource::<LevelData>().shape(), Shape::Flat);
        assert_eq!(app.world().resource::<Respawn>().0, castle_spawn());

        let mut players = app
            .world_mut()
            .query_filtered::<&Transform, With<player::Player>>();
        let at = players.single(app.world()).expect("no player").translation;
        assert_eq!(
            at,
            castle_spawn(),
            "the player came back to the wrong place"
        );

        // And the planet's scenery went with it: nothing of the level that was
        // up is left in the world.
        let mut left = app
            .world_mut()
            .query_filtered::<Entity, With<LevelEntity>>();
        let roots = left.iter(app.world()).count();
        assert!(
            roots > 0,
            "the castle spawned nothing, so this proves nothing"
        );
    }

    /// The waterfall came out of the Rust and into `assets/levels/castle.blend`,
    /// and the entire claim of that move is that it still turns up in the game.
    ///
    /// Every piece of it is tested elsewhere -- the exporter's arithmetic
    /// against the movtex strip, the JSON's parse, the drift rate -- and none
    /// of that would notice the .glb never being asked for, the level being
    /// named something the asset path does not match, or the meshes arriving
    /// after the frame that looked for them. So this runs the real castle
    /// against the real files and waits for the water to show up.
    #[test]
    fn the_castle_adopts_the_waterfall_it_was_given() {
        let mut app = with_a_loader();
        app.update();
        assert!(
            app.world()
                .get_resource::<water::PendingSurfaces>()
                .is_some(),
            "the castle never asked for its surfaces"
        );
        let started = std::time::Instant::now();
        while app
            .world()
            .get_resource::<water::PendingSurfaces>()
            .is_some()
        {
            app.update();
            assert!(
                started.elapsed().as_secs() < 60,
                "the furniture .glb never loaded"
            );
        }

        // Two sheets over the water boxes, and the waterfall that arrived out
        // of the file. The sheets are spawned in the frame the level comes up
        // and the waterfall is not, which is the difference this whole split
        // exists to allow.
        let mut surfaces = app
            .world_mut()
            .query_filtered::<&Transform, With<water::WaterSurface>>();
        let places: Vec<Vec3> = surfaces
            .iter(app.world())
            .map(|transform| transform.translation)
            .collect();
        assert_eq!(places.len(), 3, "expected two sheets and a waterfall");
        // The strip's own centroid, which is where its origin was put so that
        // Bevy sorts it against its neighbours rather than against the map
        // origin.
        let centroid = Vec3::new(-63.4953, 13.336, -60.0127);
        assert!(
            places.iter().any(|at| (*at - centroid).length() < 0.02),
            "no surface at the waterfall: {places:?}"
        );
    }

    /// Every level's glTF is in the tree, and the packaging script copies it.
    ///
    /// This is the guard the project guide asks for on anything new under
    /// `assets/`, and the trap it guards against had already been sprung twice
    /// before the planet sprang it a third time: the packaged Windows build
    /// shipped the level in its menu and no glTF for it. Choosing the planet
    /// loaded nothing, put the castle back, shut the menu, and reported it to a
    /// stderr that a `windows_subsystem = "windows"` build has nobody attached
    /// to. From the outside that is a menu row that does nothing.
    ///
    /// Every test in this file passed at the time, because they all read the
    /// source tree. So the guard has to be on the script.
    #[test]
    fn the_windows_package_ships_every_level() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = std::fs::read_to_string(root.join("build_windows.sh"))
            .expect("build_windows.sh has gone missing");
        for id in LevelId::ALL {
            assert!(
                root.join("assets").join(id.scene()).is_file(),
                "{:?} loads assets/{} at runtime and it is not in the tree",
                id,
                id.scene()
            );
        }
        // The script greps *this file* for the paths rather than listing them,
        // so what has to hold is that the grep still finds each one. A level
        // whose path was built up out of pieces would satisfy every other
        // assertion here and ship nothing.
        let source =
            std::fs::read_to_string(root.join("src/world.rs")).expect("this file has gone missing");
        let quoted: Vec<&str> = source
            .split('"')
            .skip(1)
            .step_by(2)
            .filter(|word| word.ends_with(".glb") && word.contains('/'))
            .collect();
        for id in LevelId::ALL {
            assert!(
                quoted.contains(&id.scene()),
                "build_windows.sh greps src/world.rs for level scenes and will \
                 not find {id:?}'s as a literal: the packaged game would have \
                 {id:?} in its menu and no glTF for it"
            );
        }
        assert!(
            script.contains("src/world.rs"),
            "build_windows.sh no longer reads the level list out of this file, \
             so nothing now keeps the package and the menu in step"
        );

        // The furniture .glb is the one runtime asset whose path is composed
        // rather than written down -- `water::expect_surfaces` builds it out of
        // the level's name -- so the grep above cannot see it and the script
        // has to copy it by pattern. Left out, the packaged castle has no
        // waterfall and says nothing about it.
        let furniture: Vec<_> = std::fs::read_dir(root.join("assets/bevy"))
            .expect("assets/bevy has gone missing")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with("_furniture.glb"))
            .collect();
        assert!(
            !furniture.is_empty(),
            "no furniture .glb in the tree at all"
        );
        assert!(
            script.contains("*_furniture.glb"),
            "build_windows.sh does not copy {furniture:?}, and nothing else \
             will: those paths are built at runtime out of the level's name"
        );
    }

    #[test]
    fn every_level_names_itself_and_a_scene_to_draw() {
        for id in LevelId::ALL {
            assert!(!id.name().is_empty());
            assert!(id.scene().ends_with(".glb"), "{:?}", id.scene());
        }
        assert_eq!(LevelId::default(), LevelId::Castle);
    }

    /// The measurement the planet's void test and its spawn search are both
    /// judged against, on geometry whose answer is known.
    #[test]
    fn a_sphere_is_measured_off_its_own_vertices() {
        let vertices: Vec<Vec3> = (0..500)
            .map(|step| {
                let height = 1.0 - 2.0 * (step as f32 + 0.5) / 500.0;
                let ring = (1.0 - height * height).max(0.0).sqrt();
                let yaw = 2.4 * step as f32;
                Vec3::new(ring * yaw.cos(), height, ring * yaw.sin()) * 300.0 + Vec3::X * 7.0
            })
            .collect();
        let (centre, radius) = sphere_of(&vertices);
        assert!((centre - Vec3::X * 7.0).length() < 1.0, "{centre}");
        assert!((radius - 300.0).abs() < 1.0, "{radius}");
    }

    #[test]
    fn measuring_nothing_does_not_divide_by_zero() {
        let (centre, radius) = sphere_of(&[]);
        assert_eq!(centre, PLANET_CENTRE);
        assert!(radius > 0.0);
    }

    /// The spawn search puts the player on land, and on the *lowest* land it
    /// can find rather than the first it trips over.
    #[test]
    fn the_planet_spawn_lands_on_the_lowest_ground() {
        let radius = 300.0;
        let peak = 40.0;
        let (mut vertices, mut indices) = (Vec::new(), Vec::new());
        let (rings, segments) = (40usize, 80usize);
        for ring in 0..=rings {
            let pitch = std::f32::consts::PI * ring as f32 / rings as f32;
            for segment in 0..=segments {
                let yaw = std::f32::consts::TAU * segment as f32 / segments as f32;
                let direction = Vec3::new(
                    pitch.sin() * yaw.cos(),
                    pitch.cos(),
                    pitch.sin() * yaw.sin(),
                );
                // A world tilted from a peak at the north pole to an ocean
                // floor at the south, so "the lowest land" is a real place --
                // the shore just north of the equator -- and not a tie.
                vertices.push(direction * (radius + peak * direction.y));
            }
        }
        let at = |ring: usize, segment: usize| (ring * (segments + 1) + segment) as u32;
        for ring in 0..rings {
            for segment in 0..segments {
                indices.push([
                    at(ring, segment),
                    at(ring + 1, segment),
                    at(ring, segment + 1),
                ]);
                indices.push([
                    at(ring, segment + 1),
                    at(ring + 1, segment),
                    at(ring + 1, segment + 1),
                ]);
            }
        }
        let collision = LevelData::planet(&vertices, &indices, Vec3::ZERO, radius, Some(radius));
        let spawn = ground_to_stand_on(&collision, Vec3::ZERO, radius);
        let altitude = spawn.length() - radius - DROP_IN;
        assert!(altitude > 0.0, "spawned {altitude} m below sea level");
        assert!(
            altitude < peak * 0.25,
            "spawned {altitude} m up, which is a mountain and not a shore"
        );
    }

    #[test]
    fn a_planet_with_no_ground_still_has_a_spawn() {
        let empty = LevelData::planet(&[], &[], Vec3::ZERO, 300.0, None);
        let spawn = ground_to_stand_on(&empty, Vec3::ZERO, 300.0);
        assert!(spawn.length() > 300.0, "{spawn}");
    }
}
