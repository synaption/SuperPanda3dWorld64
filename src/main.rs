#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Super Bevy World 64.
//!
//! Comments throughout cite paths under `app/` and `sm64py/`. Those are the
//! Panda3D implementation this game was ported from, which was removed once
//! the port took over; they are provenance for a constant or a rule rather
//! than files to open, and `git log` still has them if one needs reading.

mod animation;
mod audio;
mod billboard;
mod camera;
mod console;
mod enemy;
mod input;
mod level;
mod n64;
mod pipe;
mod player;
mod shadow;
mod squad;
mod water;

use bevy::{
    core_pipeline::tonemapping::Tonemapping,
    diagnostic::FrameTimeDiagnosticsPlugin,
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowResolution},
};
use camera::FollowCamera;
use input::InputState;
use player::{Controller, Player, PlayerVisual, PreviousPose, RenderPose};
use std::path::PathBuf;

#[derive(Resource, Default)]
struct GameState {
    active: ActiveCharacter,
    aiming: bool,
    debug: bool,
}

#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
enum ActiveCharacter {
    #[default]
    Hero,
    Mario,
}

#[derive(Component)]
struct Hud;

/// Writes panics to a file beside the executable as well as to stderr.
///
/// A Windows build has no console attached -- that is what
/// `windows_subsystem = "windows"` buys, and the price is that a panic is a
/// window that opens and shuts with nothing said. The schedule tests catch the
/// startup class of that before it ships; this catches everything else, after.
fn log_panics_to_a_file() {
    let log = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("crash.txt")));
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Some(path) = &log {
            use std::io::Write;
            if let Ok(mut file) = std::fs::File::create(path) {
                let _ = writeln!(file, "Super Bevy World 64 stopped with:\n\n{info}");
            }
        }
        previous(info);
    }));
}

fn main() {
    log_panics_to_a_file();
    App::new()
        .insert_resource(ClearColor(water::SKY_COLOUR))
        .insert_resource(GameState {
            active: ActiveCharacter::Hero,
            aiming: false,
            debug: true,
        })
        .init_resource::<console::GameTuning>()
        .init_resource::<console::ConsoleState>()
        .init_resource::<input::InputState>()
        .init_resource::<audio::SoundQueue>()
        .init_resource::<water::CameraMedium>()
        .init_resource::<animation::PlayerAnimation>()
        .init_resource::<animation::EnemyGraphs>()
        .init_resource::<squad::Squad>()
        .init_resource::<squad::Whistle>()
        .insert_resource(Time::<Fixed>::from_hz(30.0))
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Super Bevy World 64".into(),
                        resolution: WindowResolution::new(1280, 720),
                        present_mode: bevy::window::PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: asset_path().to_string_lossy().into_owned(),
                    ..default()
                }),
        )
        // The whole world is drawn by this one material, so it goes on directly
        // after the plugins it is built out of.
        .add_plugins(n64::N64Plugin)
        // Bevy's glTF loader puts these metadata components into each
        // WorldAsset, but GltfPlugin does not add them to the reflection
        // registry. WorldSerializationPlugin needs the registrations when it
        // clones the loaded world into the game world.
        .register_type::<bevy::gltf::GltfExtras>()
        .register_type::<bevy::gltf::GltfSceneExtras>()
        .register_type::<bevy::gltf::GltfSceneName>()
        .register_type::<bevy::gltf::GltfMeshExtras>()
        .register_type::<bevy::gltf::GltfMeshName>()
        .register_type::<bevy::gltf::GltfMaterialExtras>()
        .register_type::<bevy::gltf::GltfMaterialName>()
        .register_type::<Transform>()
        .register_type::<GlobalTransform>()
        .register_type::<TransformTreeChanged>()
        .register_type::<Visibility>()
        .register_type::<InheritedVisibility>()
        .register_type::<ViewVisibility>()
        .register_type::<Name>()
        .register_type::<ChildOf>()
        .register_type::<Children>()
        .register_type::<Mesh3d>()
        .register_type::<MeshMaterial3d<StandardMaterial>>()
        .register_type::<bevy::camera::primitives::Aabb>()
        .register_type::<bevy::camera::visibility::DynamicSkinnedMeshBounds>()
        .register_type::<bevy::camera::visibility::NoFrustumCulling>()
        .register_type::<bevy::mesh::skinning::SkinnedMesh>()
        .register_type::<bevy::mesh::morph::MeshMorphWeights>()
        .register_type::<bevy::mesh::morph::MorphWeights>()
        .register_type::<bevy::animation::AnimationTargetId>()
        .register_type::<bevy::animation::AnimatedBy>()
        .register_type::<AnimationPlayer>()
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .add_systems(Startup, setup)
        // The console claims the keyboard first, then every device is polled
        // into one snapshot, before any schedule that reads player intent.
        .add_systems(PreUpdate, (console::input, input::gather).chain())
        .add_systems(FixedUpdate, simulation())
        .add_systems(Update, presentation())
        .add_systems(Update, overlay())
        .add_systems(PostUpdate, (billboard::systems(), n64::systems()).chain())
        .run();
}

/// The fixed-step simulation, in the order one tick runs.
///
/// The three schedules are built here rather than inline in `main` so a test
/// can initialise exactly what the game runs. That is not cosmetic: Bevy
/// rejects two queries in one system whose access it cannot *prove* disjoint,
/// and it does so when the system is first initialised -- which, in a build
/// with no console attached, is a window that opens and shuts with nothing
/// said. Initialising these in a test turns that into a failing assertion.
fn simulation() -> ScheduleConfigs<ScheduleSystem> {
    (
        player::movement,
        squad::maintain_population,
        squad::update_goals,
        squad::move_allies,
        enemy::combat,
        enemy::update,
        // The arc first, so something thrown this tick starts flying on the
        // next one rather than being stepped on the tick it was created.
        pipe::fly,
        pipe::fire,
    )
        .chain()
        .run_if(console::is_closed)
}

/// Everything that runs per rendered frame while the console is closed.
fn presentation() -> ScheduleConfigs<ScheduleSystem> {
    (
        player::sync_visual,
        camera::update,
        animation::resolve_clips,
        animation::claim_players,
        animation::attach_graphs,
        animation::track_player,
        animation::update,
        squad::whistle,
        squad::animate_allies,
        water::drift,
        water::camera_medium,
        shadow::systems(),
        controls,
        update_hud,
    )
        .chain()
        .run_if(console::is_closed)
}

/// The overlay, which runs whether or not the console is open. Sound drains
/// here unconditionally: events raised on the tick the console opened should
/// still be heard.
fn overlay() -> ScheduleConfigs<ScheduleSystem> {
    (
        console::pause_animations,
        enemy::sync_animation_visibility,
        audio::play,
        console::draw,
    )
        .chain()
}

fn asset_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("assets")))
        .filter(|path| path.join("hero/hero.glb").is_file())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"))
}

fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let (collision, render) = level::load();
    shadow::prepare(&mut commands, &mut meshes, &mut images);
    water::spawn(
        &mut commands,
        &assets,
        &mut meshes,
        &mut materials,
        &collision,
    );
    squad::spawn_circle(&mut commands, &mut meshes, &mut materials);
    commands.insert_resource(animation::CharacterAnimations::load(&assets));
    audio::preload(&mut commands, &assets);
    commands.insert_resource(collision);
    commands.spawn(WorldAssetRoot(assets.load("bevy/castle.glb#Scene0")));
    for position in render.trees {
        commands.spawn((
            // bhvTree is CYLBOARD in the original: it turns to face the camera
            // about the vertical. Without it the trees are flat cards seen
            // from one fixed side, and the mesh is exactly zero thick, so from
            // ninety degrees away there is nothing there at all.
            billboard::BillboardAxis,
            billboard::BillboardActor,
            WorldAssetRoot(assets.load("actors/tree.glb#Scene0")),
            Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
        ));
    }
    let spawn = Transform::from_xyz(-13.28, 3.0, 46.64);
    commands.insert_resource(RenderPose {
        translation: spawn.translation,
        rotation: spawn.rotation,
    });
    commands.spawn((
        Player,
        // The disc under him is as wide as the body the walls push around,
        // which is the part of him actually standing on the ground.
        shadow::ShadowCaster::new(player::PLAYER_RADIUS),
        PreviousPose::new(&spawn),
        Controller::default(),
        // What `SpatialBundle` used to carry. `Transform` now brings
        // `GlobalTransform` with it as a required component, and `Visibility`
        // brings its own computed pair, so naming these two names all four.
        spawn,
        Visibility::default(),
    ));
    commands.spawn((
        PlayerVisual,
        ActiveCharacter::Hero,
        WorldAssetRoot(assets.load("hero/hero.glb#Scene0")),
        Transform::from_scale(Vec3::splat(0.81)),
    ));
    commands.spawn((
        PlayerVisual,
        ActiveCharacter::Mario,
        WorldAssetRoot(assets.load("mario/mario.glb#Scene0")),
        Visibility::Hidden,
        Transform::from_scale(Vec3::splat(0.00667)),
    ));

    let spawns = [
        (enemy::Kind::Goomba, Vec3::new(-3., 3., 26.)),
        (enemy::Kind::Goomba, Vec3::new(-24., 3., 29.)),
        (enemy::Kind::Goomba, Vec3::new(9., 3., 34.)),
        (enemy::Kind::Scuttlebug, Vec3::new(-29., 3., 21.)),
        (enemy::Kind::Scuttlebug, Vec3::new(4., 3., 19.)),
    ];
    for (i, (kind, position)) in spawns.into_iter().enumerate() {
        enemy::spawn(&mut commands, &assets, kind, position, i as f32);
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
            pipe::Spawn::Enemy(enemy::Kind::Goomba),
            Vec3::new(-55.1, 5.4, -39.2),
        ),
        (
            pipe::Spawn::Enemy(enemy::Kind::Scuttlebug),
            Vec3::new(46.8, 5.4, -68.1),
        ),
    ];
    for (index, (spawns, position)) in pipes.into_iter().enumerate() {
        commands.spawn((
            // The enemy pipes have their interval overwritten from the console
            // every tick; the Mario pipe keeps the one it is given.
            pipe::WarpPipe::new(spawns, pipe::MARIO_INTERVAL, index as f32),
            WorldAssetRoot(assets.load("actors/warp_pipe.glb#Scene0")),
            Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
        ));
    }
    // No light entity and no ambient resource: every surface in the world is
    // drawn by `n64::N64Material`, which carries its own key and ambient terms
    // and reads neither. `n64::N64Lighting` is where the sun lives now.
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-13.0, 10.0, 56.0)
            .looking_at(Vec3::new(-13.0, 4.0, 46.0), Vec3::Y),
        Projection::from(PerspectiveProjection {
            fov: 60_f32.to_radians(),
            near: 0.05,
            far: 1000.0,
            ..default()
        }),
        FollowCamera {
            yaw: 0.0,
            pitch: -0.2,
            distance: 9.5,
        },
        // N64 colours are already display-referred bytes. Filmic HDR grading
        // would alter their contrast, saturation, and hue a second time.
        Tonemapping::None,
        // Bevy fogs per camera rather than per scene, so the medium the camera
        // is in rides along with it.
        water::air_fog(),
    ));
    commands.spawn((
        Hud,
        // One text node is now four components rather than one bundle: the
        // string, the face and size, the colour, and the layout box. `Style`
        // was folded into `Node`, which is the box itself.
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(16.0),
            top: Val::Px(12.0),
            ..default()
        },
    ));
    commands.spawn((
        Text::new("+"),
        TextFont {
            font_size: FontSize::Px(28.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(49.5),
            top: Val::Percent(47.0),
            ..default()
        },
    ));
    commands.spawn(console::panel_bundle());
    commands.spawn(console::tuning_tray_bundle());
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
}

#[allow(clippy::too_many_arguments)]
fn controls(
    mut input: ResMut<InputState>,
    mut state: ResMut<GameState>,
    mut squad: ResMut<squad::Squad>,
    mut visuals: Query<(&ActiveCharacter, &mut Visibility), With<PlayerVisual>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    console: Res<console::ConsoleState>,
) {
    if console.open || console.closed_this_frame {
        return;
    }
    if InputState::take(&mut input.swap) {
        // The squad is made of Marios, and the player has just become one of
        // them or stopped being one. Either way it is not a squad any more.
        squad.disband();
        state.active = if state.active == ActiveCharacter::Hero {
            ActiveCharacter::Mario
        } else {
            ActiveCharacter::Hero
        };
        for (kind, mut visibility) in &mut visuals {
            *visibility = if *kind == state.active {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
    if InputState::take(&mut input.debug) {
        state.debug = !state.debug;
    }
    if InputState::take(&mut input.cursor) {
        if let Ok(mut cursor) = cursor.single_mut() {
            let locked = cursor.grab_mode != CursorGrabMode::None;
            cursor.grab_mode = if locked {
                CursorGrabMode::None
            } else {
                CursorGrabMode::Locked
            };
            cursor.visible = locked;
        }
    }
}

fn update_hud(
    state: Res<GameState>,
    input: Res<InputState>,
    squad: Res<squad::Squad>,
    player: Query<&Controller, With<Player>>,
    mut text: Query<&mut Text, With<Hud>>,
) {
    let Ok(ctrl) = player.single() else {
        return;
    };
    let Ok(mut hud) = text.single_mut() else {
        return;
    };
    // A single-run text node is written through as a whole string now that
    // extra runs are child entities rather than a `sections` vector.
    **hud = if state.debug {
        let device = if input.pad { "gamepad" } else { "keyboard" };
        let following = squad.members.len();
        let marching = squad.marching();
        let holding = squad.sent.len() - marching;
        format!("Super Bevy World 64\n{:?}  ·  {:?}  ·  Health {}  ·  {device}\nSquad {following} following · {marching} marching · {holding} holding\nWASD move · mouse look · Space jump · V jetpack/skate\nShift attack · X squad (hold to whistle, tap to send)\nF/right mouse aim · ` console · F2 switch · Esc cursor", state.active, ctrl.motion, ctrl.health)
    } else {
        String::new()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::AssetPlugin, ecs::schedule::Schedule, world_serialization::WorldAsset,
    };
    use enemy::Enemy;

    /// Initialises a schedule the way the game does at startup.
    ///
    /// System parameters are validated here, before anything runs, so this
    /// catches the class of mistake that cannot be seen any other way in a
    /// windowed build: two queries in one system that Bevy cannot prove touch
    /// different entities. It panics with `B0001` when they conflict, and a
    /// panic in a test is a failure with a name attached rather than a game
    /// that opens and shuts.
    fn initialise(systems: ScheduleConfigs<ScheduleSystem>) {
        let mut world = World::new();
        let mut schedule = Schedule::default();
        schedule.add_systems(systems);
        schedule
            .initialize(&mut world)
            .expect("the schedule could not be built");
    }

    /// The game's own resources, entities and schedules, running with no
    /// window and no GPU.
    ///
    /// Everything gameplay touches is here: the real castle collision, the
    /// player, the camera, the water, the whistle ring and the HUD, spawned by
    /// the game's own `setup`. What is *not* here is the renderer, so scenes
    /// and clips never finish loading and the systems that wait on them take
    /// their not-ready path -- which is the same path they take on the first
    /// frames of the real game.
    fn headless() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            // Input has no window behind it here; the resources exist and read
            // as nothing pressed, which is what the systems consume.
            .add_plugins(bevy::input::InputPlugin)
            .add_plugins(AssetPlugin {
                file_path: asset_path().to_string_lossy().into_owned(),
                ..default()
            })
            .register_type::<bevy::gltf::GltfExtras>()
            .register_type::<bevy::gltf::GltfSceneExtras>()
            .register_type::<bevy::gltf::GltfSceneName>()
            .register_type::<bevy::gltf::GltfMeshExtras>()
            .register_type::<bevy::gltf::GltfMeshName>()
            .register_type::<bevy::gltf::GltfMaterialExtras>()
            .register_type::<bevy::gltf::GltfMaterialName>()
            .register_type::<Transform>()
            .register_type::<GlobalTransform>()
            .register_type::<TransformTreeChanged>()
            .register_type::<Visibility>()
            .register_type::<InheritedVisibility>()
            .register_type::<ViewVisibility>()
            .register_type::<Name>()
            .register_type::<ChildOf>()
            .register_type::<Children>()
            .register_type::<Mesh3d>()
            .register_type::<MeshMaterial3d<StandardMaterial>>()
            .register_type::<bevy::camera::primitives::Aabb>()
            .register_type::<bevy::camera::visibility::DynamicSkinnedMeshBounds>()
            .register_type::<bevy::camera::visibility::NoFrustumCulling>()
            .register_type::<bevy::mesh::skinning::SkinnedMesh>()
            .register_type::<bevy::mesh::morph::MeshMorphWeights>()
            .register_type::<bevy::mesh::morph::MorphWeights>()
            .register_type::<bevy::animation::AnimationTargetId>()
            .register_type::<bevy::animation::AnimatedBy>()
            .register_type::<AnimationPlayer>()
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            // The material swap and the light sync both run headlessly; what
            // is missing without a renderer is the pipeline that draws them,
            // which `MaterialPlugin` would bring along with a render world
            // this app does not have.
            .init_asset::<n64::N64Material>()
            .init_resource::<n64::N64Lighting>()
            .init_resource::<n64::Converted>()
            .init_asset::<WorldAsset>()
            .init_asset::<AnimationClip>()
            .init_asset::<AnimationGraph>()
            .init_asset::<Image>()
            .init_asset::<bevy::gltf::Gltf>()
            .insert_resource(ClearColor(water::SKY_COLOUR))
            .insert_resource(GameState {
                active: ActiveCharacter::Hero,
                aiming: false,
                debug: true,
            })
            .init_resource::<console::GameTuning>()
            .init_resource::<console::ConsoleState>()
            .init_resource::<input::InputState>()
            .init_resource::<audio::SoundQueue>()
            .init_resource::<water::CameraMedium>()
            .init_resource::<animation::PlayerAnimation>()
            .init_resource::<animation::EnemyGraphs>()
            .init_resource::<squad::Squad>()
            .init_resource::<squad::Whistle>()
            .insert_resource(Time::<Fixed>::from_hz(30.0))
            // A test loop runs far faster than real time, so without this the
            // clock would barely advance and the fixed step would never tick:
            // the simulation would go unexercised while the test still passed.
            // Sixteen milliseconds a frame is a 60 Hz session.
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ))
            .add_systems(Startup, setup)
            .add_systems(FixedUpdate, simulation())
            .add_systems(Update, presentation())
            .add_systems(Update, overlay())
            .add_systems(PostUpdate, (billboard::systems(), n64::systems()).chain());
        app
    }

    /// Runs the real loop for a while. A panic anywhere in it fails the test,
    /// which is the whole point: in a windowed build the same panic is a
    /// window that opens and shuts.
    #[test]
    fn the_game_runs_without_a_window() {
        let mut app = headless();
        for _ in 0..8 {
            app.update();
        }
        // It got far enough to build the world rather than dying in startup.
        let mut players = app.world_mut().query_filtered::<Entity, With<Player>>();
        assert_eq!(
            players.iter(app.world()).count(),
            1,
            "no player in the world"
        );
        let mut water = app.world_mut().query::<&water::WaterSurface>();
        assert!(water.iter(app.world()).count() > 0, "no water was spawned");
    }

    /// The same, with the player actually doing things: the input a real
    /// session produces, driven through the fixed step.
    #[test]
    fn a_played_session_runs_without_a_window() {
        let mut app = headless();
        app.update();
        for frame in 0..40 {
            {
                let mut input = app.world_mut().resource_mut::<input::InputState>();
                input.move_axis = Vec2::new(0.0, 1.0);
                input.jump = frame % 12 == 0;
                input.attack = frame % 7 == 0;
                input.boost = frame % 5 == 0;
                // Hold the whistle, then let it go: both squad commands.
                input.squad = frame % 20 < 8;
                input.squad_released = frame % 20 == 8;
                input.swap = frame == 25;
            }
            app.update();
        }
        // Marios were spawned by the console's population count and are being
        // stepped without anything falling over.
        let mut allies = app.world_mut().query::<&squad::Ally>();
        let count = allies.iter(app.world()).count();
        let wanted = app.world().resource::<console::GameTuning>().ally_count as usize;
        assert_eq!(
            count, wanted,
            "the field holds {count} Marios, not {wanted}"
        );
    }

    /// Every `TextFont` here leaves `font` unset and so draws with
    /// `Handle::<Font>::default()`. That handle resolves to a real face only
    /// when Bevy's `default_font` feature embeds one; without it each text
    /// node still lays out and still paints its background, and every glyph
    /// silently goes missing -- the console, the HUD and the crosshair all
    /// disappear at once while nothing anywhere reports an error.
    #[test]
    fn text_has_a_font_to_draw_with() {
        let mut app = App::new();
        app.add_plugins((
            TaskPoolPlugin::default(),
            AssetPlugin::default(),
            bevy::text::TextPlugin,
        ));
        assert!(
            app.world()
                .resource::<Assets<Font>>()
                .get(&Handle::<Font>::default())
                .is_some(),
            "no default font is loaded, so no text in this game can be read"
        );
    }

    /// The warp pipes have to actually populate the world.
    ///
    /// Each pipe fires every few seconds and its goomba walks straight at the
    /// player. Before the combat rules were ported in full, that goomba threw
    /// a player who was doing nothing at all into the air and was then stomped
    /// by his own descent -- so every pipe emptied itself within seconds and
    /// the pipes looked like they spawned nothing. Twenty-five seconds is
    /// three firings of the pipe by the castle gate.
    #[test]
    fn warp_pipes_populate_the_world() {
        let mut app = headless();
        app.update();
        let count = |app: &mut App| {
            let mut enemies = app.world_mut().query_filtered::<Entity, With<Enemy>>();
            enemies.iter(app.world()).count()
        };
        let placed = count(&mut app);
        // The player does nothing at all for the whole run, so nothing in it
        // has any business defeating an enemy: the population may only grow.
        let mut lowest = placed;
        for _ in 0..1500 {
            app.update();
            lowest = lowest.min(count(&mut app));
        }
        let now = count(&mut app);
        assert_eq!(
            lowest, placed,
            "the population fell to {lowest} from {placed}, so an enemy was \
             destroyed by a player who never pressed anything"
        );
        assert!(
            now >= placed + 3,
            "after twenty-five seconds the field holds {now} enemies against \
             the {placed} it started with, so the pipes produced nothing that \
             lived"
        );
    }

    /// enemy_limit is the enemy pipes' population control, not a second cap
    /// hidden behind the much smaller Mario-pipe brood setting.
    #[test]
    fn enemy_limit_is_the_exact_field_cap() {
        let mut app = headless();
        app.update();
        {
            let mut tuning = app.world_mut().resource_mut::<console::GameTuning>();
            tuning.enemy_rate = player::FIXED_DT;
            tuning.enemy_limit = 9.0;
            tuning.pipe_brood = 0.0;
        }
        for _ in 0..120 {
            app.update();
        }
        let mut enemies = app.world_mut().query_filtered::<Entity, With<Enemy>>();
        let count = enemies.iter(app.world()).count();
        assert_eq!(
            count, 9,
            "enemy_limit asked for 9 live enemies but the field holds {count}"
        );
    }

    /// Each pipe produces its own thing, and throws it clear of itself.
    ///
    /// Three pipes and three different broods: goombas out of one, scuttlebugs
    /// out of another, and Marios out of the one on the castle path. And every
    /// one of them is *thrown* -- it goes up out of the barrel and comes down
    /// somewhere else -- so a brood that never left the ground, or one that
    /// came down in a stack on the pipe's own lip, fails here.
    #[test]
    fn each_pipe_throws_its_own_brood_clear_of_itself() {
        let mut app = headless();
        app.update();
        let mut before = std::collections::HashMap::new();
        {
            let mut enemies = app.world_mut().query::<&Enemy>();
            for enemy in enemies.iter(app.world()) {
                *before.entry(enemy.kind).or_insert(0) += 1;
            }
        }
        // Where each pipe stands, to measure how far its brood was thrown.
        let mut pipes = app.world_mut().query::<(&Transform, &pipe::WarpPipe)>();
        let mouths: Vec<Vec3> = pipes
            .iter(app.world())
            .map(|(transform, _)| transform.translation)
            .collect();
        assert_eq!(mouths.len(), 3, "the field does not hold three pipes");

        // Long enough for the slowest pipe to fire twice. The rim of a pipe is
        // 205 units up -- 2.05 here -- so how high above its own mouth a brood
        // gets over the run is the whole question: something that never clears
        // that has not come out of the top of anything.
        let mut apex: f32 = 0.0;
        for _ in 0..1800 {
            app.update();
            let mut brood = app.world_mut().query_filtered::<&Transform, With<pipe::Brood>>();
            for transform in brood.iter(app.world()) {
                let here = transform.translation;
                let mouth = mouths
                    .iter()
                    .min_by(|a, b| {
                        let flat = |p: &Vec3| Vec2::new(here.x - p.x, here.z - p.z).length();
                        flat(a).total_cmp(&flat(b))
                    })
                    .unwrap();
                apex = apex.max(here.y - mouth.y);
            }
        }
        assert!(
            apex > 2.05,
            "the highest anything out of a pipe got above its mouth was \
             {apex:.2}, which does not clear the rim"
        );

        let mut after = std::collections::HashMap::new();
        let mut enemies = app.world_mut().query::<&Enemy>();
        for enemy in enemies.iter(app.world()) {
            *after.entry(enemy.kind).or_insert(0) += 1;
        }
        for kind in [enemy::Kind::Goomba, enemy::Kind::Scuttlebug] {
            let (was, now) = (before.get(&kind).copied(), after.get(&kind).copied());
            assert!(
                now > was,
                "{kind:?}: the field held {was:?} and now holds {now:?}, so no \
                 pipe produced one"
            );
        }
        // The Mario pipe's brood: allies the console's population count did not
        // put there.
        let mut brood = app
            .world_mut()
            .query_filtered::<&Transform, (With<squad::Ally>, With<pipe::Brood>)>();
        let thrown: Vec<Vec3> = brood
            .iter(app.world())
            .map(|transform| transform.translation)
            .collect();
        assert!(
            !thrown.is_empty(),
            "the pipe on the castle path produced no Marios"
        );
    }

    #[test]
    fn the_simulation_schedule_has_no_conflicting_queries() {
        initialise(simulation());
    }

    #[test]
    fn the_presentation_schedule_has_no_conflicting_queries() {
        initialise(presentation());
    }

    #[test]
    fn the_overlay_schedule_has_no_conflicting_queries() {
        initialise(overlay());
    }

    #[test]
    fn the_billboard_schedule_has_no_conflicting_queries() {
        initialise(billboard::systems());
    }

    #[test]
    fn the_material_swap_schedule_has_no_conflicting_queries() {
        initialise(n64::systems());
    }

    /// The shadow pass reaches `Transform` and `Visibility` through two queries
    /// at once -- the casters and the discs -- and Bevy will not take on trust
    /// that a disc is never its own caster.
    #[test]
    fn the_shadow_schedule_has_no_conflicting_queries() {
        initialise(shadow::systems());
    }

    #[test]
    fn the_input_schedule_has_no_conflicting_queries() {
        initialise((console::input, input::gather).chain().into_configs());
    }

    #[test]
    fn startup_has_no_conflicting_queries() {
        initialise(setup.into_configs());
    }
}
