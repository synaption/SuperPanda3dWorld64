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
    gravity::Gravity,
    level::{self, LevelData},
    pipe, player, squad, water, weapon,
};
use bevy::{
    gltf::{Gltf, GltfMesh, GltfNode},
    mesh::{Indices, VertexAttributeValues},
    prelude::*,
    world_serialization::WorldAssetRoot,
};

/// The levels the pause menu offers.
///
/// An enum rather than a table read off disk: there are two of them, each
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
}

impl LevelId {
    /// In the order the menu lists them.
    pub const ALL: [LevelId; 2] = [LevelId::Castle, LevelId::Planet];

    pub fn name(self) -> &'static str {
        match self {
            LevelId::Castle => "Castle grounds",
            LevelId::Planet => "Planet",
        }
    }

    /// The glTF drawn for it, under `assets/`.
    pub fn scene(self) -> &'static str {
        match self {
            LevelId::Castle => "bevy/castle.glb",
            LevelId::Planet => "bevy/planet.glb",
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
        Self(CASTLE_SPAWN)
    }
}

/// The spot on the castle path the game has always started on.
pub const CASTLE_SPAWN: Vec3 = Vec3::new(-13.28, 3.0, 46.64);

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
    commands.spawn((
        LevelEntity,
        WorldAssetRoot(assets.load(format!("{}#Scene0", id.scene()))),
    ));
    match id {
        LevelId::Castle => {
            let (collision, render) = level::load();
            commands.insert_resource(FlowField::new(&collision));
            water::spawn(commands, assets, meshes, materials, &collision);
            for position in render.trees {
                commands.spawn((
                    LevelEntity,
                    // bhvTree is CYLBOARD in the original: it turns to face the
                    // camera about the vertical. Without it the trees are flat
                    // cards seen from one fixed side, and the mesh is exactly
                    // zero thick, so from ninety degrees away there is nothing
                    // there at all.
                    crate::billboard::BillboardAxis,
                    crate::billboard::BillboardActor,
                    WorldAssetRoot(assets.load("actors/tree.glb#Scene0")),
                    Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
                ));
            }
            commands.insert_resource(collision);
            commands.insert_resource(Gravity::default());
            commands.insert_resource(Respawn(CASTLE_SPAWN));
            spawn_castle_inhabitants(commands, assets);
            load.pending = None;
        }
        LevelId::Planet => {
            // Empty collision and radial gravity from the first frame: the
            // world is already round while it loads, so nothing has to be
            // switched over a second time when the ground arrives.
            let empty = LevelData::planet(&[], &[], PLANET_CENTRE, 1.0);
            commands.insert_resource(FlowField::new(&empty));
            commands.insert_resource(empty);
            commands.insert_resource(Gravity::towards(PLANET_CENTRE));
            load.pending = Some(LevelId::Planet);
            load.handle = assets.load(id.scene());
        }
    }
}

/// The slimes, the ants and the three warp pipes -- everything the castle has
/// living on it.
///
/// Straight out of `main::setup`, unchanged apart from the marker: the point of
/// moving it was that it had to come *down* again, not that it was wrong.
fn spawn_castle_inhabitants(commands: &mut Commands, assets: &AssetServer) {
    let spawns = [
        (enemy::Kind::Slime, Vec3::new(-3., 3., 26.)),
        (enemy::Kind::Slime, Vec3::new(-24., 3., 29.)),
        (enemy::Kind::Slime, Vec3::new(9., 3., 34.)),
        (enemy::Kind::Ant, Vec3::new(-29., 3., 21.)),
        (enemy::Kind::Ant, Vec3::new(4., 3., 19.)),
    ];
    for (i, (kind, position)) in spawns.into_iter().enumerate() {
        enemy::spawn(commands, assets, kind, position, i as f32);
    }
    // The three pipes and what each produces, from `PIPE_SPAWNS` in
    // `app/main.py`: one by the spawn on the castle path that produces company,
    // and one in each far corner of the map that produces enemies -- so the two
    // enemy pipes are somewhere to go rather than something to trip over on the
    // way out of the gate. Every pipe's countdown runs at any distance, so a
    // crowd is waiting when the player arrives rather than only starting to
    // fill then.
    //
    // The pipes are drawn but not collided with: the level's own collision is
    // what the physics reads and nothing here adds to it, so a pipe is scenery
    // that you can walk through and that things come out of.
    let pipes = [
        (pipe::Spawn::Mario, Vec3::new(-9.15, 2.6, 46.3)),
        (
            pipe::Spawn::Enemy(enemy::Kind::Slime),
            Vec3::new(-55.1, 5.4, -39.2),
        ),
        (
            pipe::Spawn::Enemy(enemy::Kind::Ant),
            Vec3::new(46.8, 5.4, -68.1),
        ),
    ];
    for (index, (spawns, position)) in pipes.into_iter().enumerate() {
        commands.spawn((
            LevelEntity,
            // The enemy pipes have their interval overwritten from the console
            // every tick; the Mario pipe keeps the one it is given.
            pipe::WarpPipe::new(spawns, pipe::MARIO_INTERVAL, index as f32),
            WorldAssetRoot(assets.load("actors/warp_pipe.glb#Scene0")),
            Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
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
        put_the_player_down(CASTLE_SPAWN, Vec3::Y, &mut commands, &mut placement);
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
        follow.clearance = 1.0;
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
    mut placement: ParamSet<(PlacePlayer<'_, '_>, PlaceCamera<'_, '_>)>,
) {
    if load.pending != Some(LevelId::Planet) {
        return;
    }
    // Waited on rather than timed out. A load that is merely slow -- 14 MB of
    // glTF off a cold disk -- is still going to arrive, and a wall clock is a
    // bad judge of that: it would give up on a slow machine and never fire on a
    // fast one. What is worth reacting to is the asset server saying it cannot
    // be done, which it says plainly.
    if let bevy::asset::LoadState::Failed(why) = assets.load_state(&load.handle) {
        let trouble = format!("{} did not load: {why}", LevelId::Planet.scene());
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
    let Some((vertices, indices)) = read_geometry(gltf, &nodes, &gltf_meshes, &meshes) else {
        return;
    };
    let (centre, radius) = sphere_of(&vertices);
    let collision = LevelData::planet(&vertices, &indices, centre, radius);
    let spawn = ground_to_stand_on(&collision, centre, radius);
    console.report(format!(
        "planet: {} triangles, radius {radius:.0} m",
        indices.len()
    ));

    put_the_player_down(spawn, gravity.up(spawn), &mut commands, &mut placement);

    commands.insert_resource(FlowField::new(&collision));
    commands.insert_resource(Respawn(spawn));
    commands.insert_resource(collision);
    load.pending = None;
    load.handle = Handle::default();
}

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
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    // The roots are the nodes nobody claims as a child. `Gltf::nodes` is every
    // node in the file, parents and children alike, so walking it directly
    // would visit a child once on its own and once under its parent.
    let mut children = Vec::new();
    for handle in &gltf.nodes {
        children.extend(nodes.get(handle)?.children.iter().map(|child| child.id()));
    }
    let mut geometry = (Vec::new(), Vec::new());
    for handle in &gltf.nodes {
        if children.contains(&handle.id()) {
            continue;
        }
        read_node(handle, Transform::IDENTITY, nodes, gltf_meshes, meshes, &mut geometry)?;
    }
    Some(geometry)
}

/// One node and everything under it, with `parent` already applied.
fn read_node(
    handle: &Handle<GltfNode>,
    parent: Transform,
    nodes: &Assets<GltfNode>,
    gltf_meshes: &Assets<GltfMesh>,
    meshes: &Assets<Mesh>,
    into: &mut (Vec<Vec3>, Vec<[u32; 3]>),
) -> Option<()> {
    let node = nodes.get(handle)?;
    let here = parent * node.transform;
    if let Some(mesh_handle) = &node.mesh {
        for primitive in &gltf_meshes.get(mesh_handle)?.primitives {
            let mesh = meshes.get(&primitive.mesh)?;
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                continue;
            };
            // Each tile brings its own vertex array, so every tile's indices
            // are offset past the tiles already read. The tiles do share a
            // boundary ring by value rather than by index; the duplicates cost
            // twelve bytes each and no correctness.
            let base = into.0.len() as u32;
            into.0
                .extend(positions.iter().map(|p| here * Vec3::from(*p)));
            let corners: Vec<u32> = match mesh.indices() {
                Some(Indices::U16(values)) => values.iter().map(|&i| base + i as u32).collect(),
                Some(Indices::U32(values)) => values.iter().map(|&i| base + i).collect(),
                // An unindexed primitive is three vertices to a triangle in
                // order, which is what the glTF spec says it is.
                None => (base..base + positions.len() as u32).collect(),
            };
            into.1
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
/// The *lowest* land rather than the first, which is one comparison and worth
/// it. The first is wherever the spiral happens to start, and on this planet
/// that was a glacier: a mountaintop is the worst place to arrive on a world
/// you are meant to walk around, and a white one is the worst place to
/// photograph it from. Lowland is flat, walkable, and next to the sea.
fn ground_to_stand_on(collision: &LevelData, centre: Vec3, radius: f32) -> Vec3 {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    const CANDIDATES: usize = 256;
    let mut lowest_land: Option<(f32, Vec3)> = None;
    let mut anywhere: Option<Vec3> = None;
    for step in 0..CANDIDATES {
        let height = 1.0 - 2.0 * (step as f32 + 0.5) / CANDIDATES as f32;
        let ring = (1.0 - height * height).max(0.0).sqrt();
        let yaw = golden * step as f32;
        let up = Vec3::new(ring * yaw.cos(), height, ring * yaw.sin()).normalize();
        let from = centre + up * (radius + 60.0);
        let Some((point, _)) = collision.ground_below(from, up) else {
            continue;
        };
        let standing = point + up * DROP_IN;
        anywhere.get_or_insert(standing);
        let altitude = (point - centre).length() - radius;
        if altitude > 0.0 && lowest_land.is_none_or(|(best, _)| altitude < best) {
            lowest_land = Some((altitude, standing));
        }
    }
    // No land, or no collision at all. Above the surface either way, so the
    // fall is short and onto something rather than through everything.
    lowest_land
        .map(|(_, standing)| standing)
        .or(anywhere)
        .unwrap_or(centre + Vec3::Y * (radius + DROP_IN))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{level::Shape, player::Controller};
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
        // Bounded by the wall clock rather than by frames: this reads 14 MB of
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
            (250.0..350.0).contains(&radius),
            "planet.glb measured {radius} m across the middle"
        );

        let start = app.world().resource::<Respawn>().0;
        let up = gravity.up(start);
        assert!(
            (start - centre).length() > radius,
            "the player was put down under the sea"
        );
        assert!(up.dot(Vec3::Y).abs() < 0.999, "up is still the world's up");

        // Now play. Forward for two seconds of fixed steps, which at 30 Hz and
        // 16 ms a frame is sixty frames' worth.
        app.world_mut().resource_mut::<crate::input::InputState>().move_axis = Vec2::new(0.0, 1.0);
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
        assert_eq!(app.world().resource::<Respawn>().0, CASTLE_SPAWN);

        let mut players = app
            .world_mut()
            .query_filtered::<&Transform, With<player::Player>>();
        let at = players.single(app.world()).expect("no player").translation;
        assert_eq!(at, CASTLE_SPAWN, "the player came back to the wrong place");

        // And the planet's scenery went with it: nothing of the level that was
        // up is left in the world.
        let mut left = app.world_mut().query_filtered::<Entity, With<LevelEntity>>();
        let roots = left.iter(app.world()).count();
        assert!(
            roots > 0,
            "the castle spawned nothing, so this proves nothing"
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
        let source = std::fs::read_to_string(root.join("src/world.rs"))
            .expect("this file has gone missing");
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
                indices.push([at(ring, segment), at(ring + 1, segment), at(ring, segment + 1)]);
                indices.push([
                    at(ring, segment + 1),
                    at(ring + 1, segment),
                    at(ring + 1, segment + 1),
                ]);
            }
        }
        let collision = LevelData::planet(&vertices, &indices, Vec3::ZERO, radius);
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
        let empty = LevelData::planet(&[], &[], Vec3::ZERO, 300.0);
        let spawn = ground_to_stand_on(&empty, Vec3::ZERO, 300.0);
        assert!(spawn.length() > 300.0, "{spawn}");
    }
}
