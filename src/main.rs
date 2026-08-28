#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Super Bevy World 64.
//!
//! Comments throughout cite paths under `app/` and `sm64py/`. Those are the
//! Panda3D implementation this game was ported from, which was removed once
//! the port took over; they are provenance for a constant or a rule rather
//! than files to open, and `git log` still has them if one needs reading.

mod aim;
mod animation;
mod audio;
mod billboard;
mod camera;
mod console;
mod display;
mod enemy;
mod flow;
mod frame_chart;
mod furniture;
mod gravity;
mod health;
mod impostor;
mod input;
mod level;
mod menu;
mod n64;
mod pipe;
mod player;
mod shadow;
mod shot;
mod sky;
#[cfg(feature = "spike")]
mod spike;
mod squad;
mod water;
mod weapon;
mod world;

use bevy::{
    app::{TaskPoolOptions, TaskPoolPlugin},
    core_pipeline::tonemapping::Tonemapping,
    diagnostic::{DiagnosticsStore, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin},
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    input::InputSystems,
    prelude::*,
    render::view::Msaa,
    window::{
        CursorGrabMode, CursorOptions, MonitorSelection, PrimaryWindow, WindowMode,
        WindowResolution,
    },
};
use camera::FollowCamera;
use display::SceneTarget;
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

/// The frame-rate readout in the corner.
///
/// Its own node rather than a line of the debug HUD, and always drawn, because
/// what it is for is telling whether a change costs anything -- and a reading
/// that goes away with the rest of the debug text is one you cannot check
/// against a clean screen.
#[derive(Component)]
struct FpsText;

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

/// Reflection registrations every world in this game needs before a glTF can
/// be spawned into it.
///
/// Bevy's glTF loader puts these metadata components into each `WorldAsset`,
/// but `GltfPlugin` does not add them to the reflection registry, and
/// `WorldSerializationPlugin` needs them when it clones a loaded world into the
/// game world. A missing one is not a compile error and not a warning: it is a
/// panic inside the spawner the first time an actor is loaded.
///
/// Shared by `main`, the headless test harness and the crowd benchmark, because
/// it is a property of the assets rather than of any one of those -- and because
/// keeping three copies of it in step is exactly the sort of thing that is
/// discovered by a benchmark falling over.
fn register_world_asset_types(app: &mut App) {
    app.register_type::<bevy::gltf::GltfExtras>()
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
        .register_type::<AnimationPlayer>();
}

/// How many threads Bevy's task pools are allowed.
///
/// Bevy's default gives the compute pool every core left over after the IO and
/// async-compute pools have taken theirs, which on a 24-thread machine is
/// sixteen workers. For a field of some fifteen hundred entities that is not
/// parallelism, it is sixteen threads each given a few microseconds of work --
/// and, on the evidence, sixteen threads that go cold whenever the frame rate
/// is paced and are slow to come back. The measurement that points here is a
/// worker thread's span around `propagate_parent_transforms`: 63.8 ms for a
/// system whose mean is 0.31 ms, on the frame after an idle gap. Measured on a
/// 24-thread hybrid CPU with the frame cap on, spikes over 8 ms fell from
/// roughly one frame in 140 at sixteen threads to one in 3778 at four.
///
/// A cap rather than a count: `percent` still takes what is left after the
/// other two pools, so a small machine gets fewer than this and only a large
/// one is held back to it. `COMPUTE_THREADS` overrides it for measuring:
///
///   COMPUTE_THREADS=8 ./SuperBevyWorld64.exe
const COMPUTE_THREADS: usize = 4;

fn task_pool_options() -> TaskPoolOptions {
    let mut options = TaskPoolOptions::default();
    options.compute.max_threads = compute_thread_cap(env_compute_threads().as_deref());
    options
}

fn env_compute_threads() -> Option<String> {
    std::env::var("COMPUTE_THREADS").ok()
}

/// Anything that is not a positive number -- unset, blank, zero, a typo --
/// leaves the shipped cap in place rather than sizing the pool by accident.
fn compute_thread_cap(request: Option<&str>) -> usize {
    request
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or(COMPUTE_THREADS)
}

/// Asks Windows for a millisecond of timer resolution instead of the default
/// fifteen and a half.
///
/// Windows rounds a thread's wake-up to the system timer tick, which is 15.625
/// ms unless somebody asks for better, and Bevy's task pool parks its workers
/// when there is nothing to run. So a frame that has to wake a parked worker --
/// which is any frame with a parallel system in it, and `PostUpdate` has
/// several -- can pay a whole tick for the wake. It is visible in this game's
/// own frame chart as a tight cluster of stalls at 15.0 to 15.4 ms, just under
/// the tick, and it is the residue left after the present block was taken out
/// of the way: with vsync off the rate falls to a third of a percent of frames,
/// but the ones that remain are all still that same fifteen milliseconds.
///
/// Nothing in the stack asks on this game's behalf -- not winit, not
/// `bevy_winit`, not `bevy_tasks` -- so it is asked for here. Since Windows 10
/// 2004 the request applies to the calling process rather than the whole
/// machine, and it is released when the process ends, which is why there is no
/// matching `timeEndPeriod`: there is nowhere to put one that a crash would not
/// skip anyway.
#[cfg(target_os = "windows")]
fn ask_windows_for_a_sharper_clock() {
    // Declared by hand rather than by taking on the `windows` crate, which is
    // a large dependency to add for one symbol out of winmm.
    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
    }
    // SAFETY: `timeBeginPeriod` takes a plain integer and touches nothing this
    // program owns. One is the smallest period the call accepts, and a refusal
    // is reported in the return rather than by doing something else, so there
    // is nothing to check: the game runs either way, a little jerkier if the
    // request was turned down.
    unsafe {
        timeBeginPeriod(1);
    }
}

#[cfg(not(target_os = "windows"))]
fn ask_windows_for_a_sharper_clock() {}

fn main() {
    log_panics_to_a_file();
    ask_windows_for_a_sharper_clock();
    // The impostor baker runs inside the game rather than beside it, so that
    // the sprites it draws are lit by the same material the skinned models are.
    // `cargo run --release -- bake-impostors [slime|ant]`.
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("bake-impostors") {
        let named: Vec<enemy::Kind> = arguments[1..]
            .iter()
            .filter_map(|word| match word.to_ascii_lowercase().as_str() {
                "slime" => Some(enemy::Kind::Slime),
                "ant" => Some(enemy::Kind::Ant),
                other => {
                    eprintln!("bake-impostors: {other:?} is not an actor with a sheet");
                    None
                }
            })
            .collect();
        let all = [enemy::Kind::Slime, enemy::Kind::Ant];
        impostor::bake::run(if named.is_empty() { &all } else { &named });
        return;
    }
    // `cargo run --release -- screenshot out.png [crowd] [x,y,z] [look x,y,z]`,
    // which is the only way to see this game on a machine with no display.
    if arguments.first().map(String::as_str) == Some("screenshot") {
        // `None` rather than a default, because where the default *is* depends
        // on the level: the castle's is a fixed view down the path, and a
        // planet's is wherever the load happened to put the player down.
        let triple = |word: Option<&String>| -> Option<Vec3> {
            let parts: Vec<f32> = word?
                .split(',')
                .filter_map(|part| part.trim().parse().ok())
                .collect();
            match parts[..] {
                [x, y, z] => Some(Vec3::new(x, y, z)),
                _ => None,
            }
        };
        let path = arguments
            .get(1)
            .cloned()
            .unwrap_or_else(|| "screenshot.png".into());
        let crowd = arguments.get(2).and_then(|n| n.parse().ok()).unwrap_or(0);
        let eye = triple(arguments.get(3));
        let at = triple(arguments.get(4));
        shot::run(std::path::Path::new(&path), crowd, eye, at);
        return;
    }
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(TaskPoolPlugin {
                task_pool_options: task_pool_options(),
            })
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Super Bevy World 64".into(),
                    // Borderless rather than exclusive fullscreen: it takes
                    // the monitor's own resolution and refresh rate instead
                    // of asking for a mode switch, so alt-tabbing away and
                    // back does not black the screen while the display
                    // renegotiates. F11 goes back to a window.
                    mode: WindowMode::BorderlessFullscreen(MonitorSelection::Current),
                    // The size the window takes when F11 leaves fullscreen.
                    resolution: WindowResolution::new(1280, 720),
                    present_mode: display::DEFAULT_PRESENT_MODE,
                    ..default()
                }),
                ..default()
            })
            .set(AssetPlugin {
                file_path: asset_path().to_string_lossy().into_owned(),
                ..default()
            }),
    );
    add_game(&mut app);
    app.add_systems(PreUpdate, input_pipeline()).run();
}

/// Everything that makes an `App` this game, short of the plugins and the
/// window.
///
/// Shared by `main`, the crowd benchmark and the screenshot tool, so that all
/// three run the same game. Keeping three hand-written copies of this list in
/// step is not a thing anyone succeeds at: the benchmark was already a
/// half-copy that fell over on a missing reflection registration the first time
/// it was run, which is what prompted pulling it out.
///
/// The input pipeline is *not* here. It reads real devices, and the two
/// headless callers have none; `main` adds it separately.
pub fn add_game(app: &mut App) {
    game_resources(app);
    // The whole world is drawn by this one material, so it goes on directly
    // after the plugins it is built out of.
    app.add_plugins(n64::N64Plugin)
        // Enough raw history for the chart's four-second window at 240 Hz.
        .add_plugins(FrameTimeDiagnosticsPlugin::new(960))
        // The other half of the benchmark readout: an enemy is a whole scene of
        // entities rather than one, and that multiplier is what the crowd work
        // is trying to bring down.
        .add_plugins(EntityCountDiagnosticsPlugin::default());
    register_world_asset_types(app);
    game_systems(app);
}

/// Every resource the game's systems expect to find.
///
/// Its own function because the headless test harness cannot use
/// [`add_game`] -- it runs on `MinimalPlugins` with the render plugins stubbed
/// out -- but must have exactly this list, and a resource added to one copy and
/// not the other is not a compile error. It is every schedule test failing at
/// once with "Resource does not exist" and no name attached, which is precisely
/// how this function came to exist.
pub fn game_resources(app: &mut App) {
    app.insert_resource(ClearColor(water::SKY_COLOUR))
        .insert_resource(GameState {
            active: ActiveCharacter::Hero,
            aiming: false,
            debug: true,
        })
        .init_resource::<console::GameTuning>()
        // The queue every kill is posted to and `enemy::alert` drains.
        .init_resource::<enemy::Threats>()
        .init_resource::<console::ConsoleState>()
        .init_resource::<input::InputState>()
        .init_resource::<audio::SoundQueue>()
        .init_resource::<water::CameraMedium>()
        .init_resource::<animation::PlayerAnimation>()
        .init_resource::<animation::EnemyGraphs>()
        .init_resource::<squad::Squad>()
        .init_resource::<squad::Whistle>()
        .init_resource::<menu::MenuState>()
        .init_resource::<display::DisplaySettings>()
        .init_resource::<display::FramePacing>()
        // How far through its fade each unit wearing a health bar is.
        .init_resource::<health::BarFades>()
        .init_resource::<sky::Sky>()
        .init_resource::<impostor::ImpostorStats>()
        .init_resource::<frame_chart::CalculationTimes>()
        .init_resource::<weapon::Loadout>()
        .init_resource::<aim::Aim>()
        // Which level is up, which one is on its way, where the player starts
        // on it, and which way is down. All four used to be constants, and
        // three of them were literals repeated at their use sites.
        .init_resource::<world::LevelId>()
        .init_resource::<world::LevelLoad>()
        .init_resource::<world::Respawn>()
        .init_resource::<gravity::Gravity>()
        .add_message::<world::LoadLevel>()
        .insert_resource(Time::<Fixed>::from_hz(30.0));
}

/// The schedules, in the order a frame runs them. Shared for the same reason
/// [`game_resources`] is.
///
/// The input pipeline is not here: it reads real devices, and the headless
/// callers have none.
pub fn game_systems(app: &mut App) {
    #[cfg(feature = "spike")]
    {
        app.add_plugins(spike::plugin);
        app.add_plugins(spike::harness);
    }
    app.add_systems(First, frame_chart::begin)
        .add_systems(Last, frame_chart::finish)
        // After the chart has stopped its clock, so a frame held back to the
        // player's cap is not drawn as a frame that took that long to think.
        .add_systems(Last, display::cap_frames.after(frame_chart::finish))
        .add_systems(Startup, (setup, weapon::load_shot_assets))
        .add_systems(FixedUpdate, simulation())
        .add_systems(Update, presentation())
        .add_systems(Update, overlay())
        // Chained: `weapon::carry` points the gun down `aim::Aim`, which
        // `aim::drive` writes. Unordered they would still both run in this
        // window, but the gun would spend half its frames a step behind the
        // torso carrying it.
        .add_systems(PostUpdate, (aim::systems(), weapon::systems()).chain())
        .add_systems(PostUpdate, drawing())
        // In `PostUpdate` rather than with the rest of the overlay, and pinned
        // between two of Bevy's own sets: a bar is a world position projected
        // through the camera and written into a UI node, so it has to run after
        // the camera's `GlobalTransform` exists and before the layout that
        // reads what it wrote. See `health::draw_unit_bars`.
        .add_systems(
            PostUpdate,
            health::draw_unit_bars
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::ui::UiSystems::Layout),
        );
}

/// Everything that happens to geometry after the world has moved: billboards
/// aimed at the camera, the far crowd's sprites rebuilt, and every surface
/// swapped onto the N64 material.
///
/// A named chain rather than three calls at each site, because **the impostor
/// baker has to run exactly this and getting it wrong is invisible.** It did:
/// the baker was drawing actors without the billboard half, so the
/// scuttlebug's three billboard joints came out at a quarter of
/// their size -- `billboard::aim` is what puts back the 0.25 the exporter baked
/// onto the skeleton -- and single-sided, so they were culled from half the
/// angles. The sheets that came out covered 52% of the pixels the real models
/// did, which is what a swap distance looks like when enemies visibly shrink
/// as they cross it. Sharing the chain is what stops that happening again.
///
/// The order inside it is load-bearing and documented at each of the three.
pub fn drawing() -> ScheduleConfigs<ScheduleSystem> {
    (billboard::systems(), impostor::systems(), n64::systems()).chain()
}

/// Reading the keyboard, the mouse and the pad into one snapshot for the frame.
///
/// The console claims the keyboard first, then every device is polled into one
/// snapshot, before any schedule that reads player intent.
///
/// The `after` is the part that is not obvious, and leaving it off is a real
/// bug rather than untidiness. `ButtonInput` is not a fact that sits still: at
/// the top of every `PreUpdate`, Bevy's own `keyboard_input_system` *clears*
/// last frame's just-pressed set and refills it from this frame's events.
/// Sharing a schedule with it is not enough, because these two systems only
/// take turns -- they both touch `ButtonInput<KeyCode>`, so the executor will
/// not run them at once, but nothing said which goes first, and which one wins
/// is down to whichever thread is free.
///
/// Land on opposite sides of the clear on two consecutive frames and one
/// physical key press is read as two. Every edge here is a toggle or latches
/// with `|=`, so reading it twice is reading it zero times: the console opens
/// and shuts inside a single frame, F1 turns the HUD off and back on, F11
/// leaves and re-enters fullscreen, and Escape drops the cursor and grabs it
/// again. The key looks dead. It was pressed exactly once and acted on twice.
fn input_pipeline() -> ScheduleConfigs<ScheduleSystem> {
    (console::input, menu::input, input::gather)
        .chain()
        .after(InputSystems)
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
        // Before anything that reads it. The field is what the crowd tier
        // navigates by, and one built from last tick's player position would
        // send two thousand enemies a step behind him.
        flow::rebuild,
        // And before every system that asks how much simulation an enemy is
        // getting this tick.
        enemy::assign_detail,
        // After the sweep, whose step counts it spreads along.
        enemy::rouse_crowd,
        squad::maintain_population,
        squad::update_goals,
        squad::move_allies,
        // Before `enemy::combat`, so a shot and a swing in the same tick are
        // resolved in the order the trigger was pulled rather than the swing
        // silently winning. Both take the same latched edge and only one of
        // them is allowed to, so in practice they never both act -- but the
        // order is the cheap half of making that true.
        weapon::swap,
        weapon::fire,
        enemy::combat,
        // After the walk step: a Mario mid-punch is punching, whatever the walk
        // made of it.
        enemy::ally_combat,
        // And the other side of the same fight, straight after it, so a Mario
        // that killed what it was hitting is not then hurt by the thing it just
        // removed. Both raise threats, and `alert` below drains them together.
        enemy::maul,
        enemy::alert,
        enemy::update,
        // Straight after the step that decided what is near enough to be drawn
        // as a model, so the scene is built or shed against this tick's answer.
        enemy::shed_scenes,
        // After the step that moved them, so a crowd that walked into itself
        // this tick is untangled before it is drawn rather than a tick later.
        enemy::spread,
        // The arc first, so something thrown this tick starts flying on the
        // next one rather than being stepped on the tick it was created.
        pipe::fly,
        pipe::fire,
        // After everything that moved an enemy this tick, so a bullet is
        // tested against where its target actually ended up.
        weapon::fly,
        // The feet come round after the walk step that decided where he was
        // facing, for the same reason `ally_combat` follows the walk.
        aim::turn_body,
    )
        .chain()
        .run_if(console::is_closed)
        .run_if(menu::is_closed)
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
        water::adopt_surfaces,
        water::find_ocean,
        water::drift_ocean,
        water::camera_medium,
        // Reads the light the sky wrote *last* frame, which is what lets it sit
        // ahead of the sky rather than in the middle of it -- a frame of lag on
        // a colour that takes a minute to cross the sunset is not a thing that
        // can be seen.
        water::dim,
        // After it, and that order is the point: `camera_medium` owns the fog
        // and the clear colour underwater and the sky owns both above it, so
        // the frame the camera surfaces has to end with the sky's answer.
        sky::systems(),
        shadow::systems(),
        weapon::fade,
        // Nested rather than three more entries in the tuple: Bevy's system
        // tuples stop at twenty, and a nested one that chains itself keeps the
        // order exactly as it reads.
        (controls, update_hud, health::draw_player_bar).chain(),
    )
        .chain()
        .run_if(console::is_closed)
        .run_if(menu::is_closed)
}

/// The overlay, which runs whether or not the console is open. Sound drains
/// here unconditionally: events raised on the tick the console opened should
/// still be heard.
fn overlay() -> ScheduleConfigs<ScheduleSystem> {
    (
        console::pause_animations,
        // After the console's, which resumes everything the frame it closes:
        // a menu open over a closed console still holds the world still.
        menu::pause_animations,
        // Both out here rather than in `simulation`, because the menu that
        // asks for a level is open at the time and `simulation` is held still
        // while it is. `finish_planet` follows `switch` so that a planet asked
        // for this frame is one frame further along by the end of it.
        world::switch,
        world::finish_planet,
        // Out here rather than in `simulation` because the console is open at
        // the moment a `crowd` command is typed, and a field that only arrived
        // once you shut the console is a field you never saw arrive.
        enemy::crowd,
        // Straight after it, because the two share the console's request queue
        // and each hands back what the other one wanted. See `ConsoleState::defer`.
        weapon::equip,
        enemy::sync_animation_visibility,
        audio::play,
        console::draw,
        // Before the menu is drawn rather than after, so the resolution the
        // menu reads back out of the target is the one the row above it just
        // asked for rather than last frame's.
        display::resize,
        menu::draw,
        // Both of these belong out here rather than in `presentation`: a frame
        // drawn while the console is open is still a frame, and being stuck
        // fullscreen because the console is up is exactly the trap F11 exists
        // to avoid.
        toggle_fullscreen,
        update_fps,
        frame_chart::update,
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

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sprites: ResMut<Assets<n64::N64Material>>,
    mut images: ResMut<Assets<Image>>,
    mut console: ResMut<console::ConsoleState>,
    mut load: ResMut<world::LevelLoad>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    // Made before anything that refers to it: the world camera draws into it,
    // the full-screen node below shows it, and `display::resize` sizes it to
    // the window on the first frame.
    let scene_target = display::create_target(&mut images);
    commands.insert_resource(SceneTarget(scene_target.clone()));
    let shadow_art = shadow::prepare(&mut meshes, &mut images, &mut materials);
    impostor::prepare(
        &mut commands,
        &assets,
        &mut meshes,
        &mut sprites,
        // The far crowd draws its own shadows into its own mesh, so it needs the
        // same disc material every other shadow in the game uses -- at full
        // strength, since everything it draws is stood on the ground.
        shadow_art.fade(shadow::SOLID),
        &mut console,
        &asset_path(),
    );
    commands.insert_resource(shadow_art);
    // The sky rides the camera and outlives every level, so it is put up here
    // beside the other two `prepare`s rather than by whichever level is up
    // first. `sky::advance` is what turns it off on a level that is not the
    // castle grounds.
    sky::prepare(&mut commands, &mut meshes, &mut images, &mut sprites);
    squad::spawn_circle(&mut commands, &mut meshes, &mut materials);
    commands.insert_resource(animation::CharacterAnimations::load(&assets));
    audio::preload(&mut commands, &assets);
    // The level itself -- its collision, its gravity, its scenery and its
    // inhabitants -- and nothing else here knows which one it is. Everything
    // below this line outlives a level and is never respawned when one changes.
    world::spawn(
        world::LevelId::default(),
        &mut commands,
        &assets,
        &mut meshes,
        &mut materials,
        &mut load,
    );
    let spawn = Transform::from_translation(world::castle_spawn());
    commands.insert_resource(RenderPose {
        translation: spawn.translation,
        rotation: spawn.rotation,
    });
    commands.spawn((
        Player,
        PreviousPose::new(&spawn),
        Controller::default(),
        // A hundred points, which is a different kind of number from the three
        // hearts this used to be: what threatens him is a crowd's worth of
        // small hits rather than any one enemy. See [`health`].
        health::Health::new(health::PLAYER_HEALTH),
        // He does not notice anything -- that is what the player is for -- but
        // he is very much noticeable, and a side is what makes him so.
        enemy::Side::Friendly,
        // What `SpatialBundle` used to carry. `Transform` now brings
        // `GlobalTransform` with it as a required component, and `Visibility`
        // brings its own computed pair, so naming these two names all four.
        spawn,
        Visibility::default(),
    ));
    // The disc goes under the *visual* rather than under the `Player` entity,
    // and that is not an arbitrary choice of parent. `Player` carries the
    // simulation's transform, rewritten thirty times a second; the visuals
    // carry `RenderPose`, which `player::sync_visual` interpolates between two
    // of those steps once per drawn frame. A shadow hung off the simulation
    // transform snaps between fixed steps while the character it belongs to
    // glides, which reads as the shadow juddering under his feet.
    //
    // It also settles which shadow: only one of the two visuals is ever shown,
    // and `project` already hides the disc of a caster nobody is drawing.
    let shadow = shadow::ShadowCaster::new(player::PLAYER_RADIUS, player::PLAYER_HEIGHT);
    commands.spawn((
        PlayerVisual,
        ActiveCharacter::Hero,
        shadow,
        WorldAssetRoot(assets.load("hero/hero.glb#Scene0")),
        Transform::from_scale(Vec3::splat(0.81)),
    ));
    commands.spawn((
        PlayerVisual,
        ActiveCharacter::Mario,
        shadow,
        WorldAssetRoot(assets.load("mario/mario.glb#Scene0")),
        Visibility::Hidden,
        Transform::from_scale(Vec3::splat(0.00667)),
    ));

    // No light entity and no ambient resource: every surface in the world is
    // drawn by `n64::N64Material`, which carries its own key and ambient terms
    // and reads neither. `n64::N64Lighting` is where the sun lives now.
    commands.spawn((
        Camera3d::default(),
        // The world is drawn at whatever internal resolution the display
        // settings ask for rather than the window's, and `display` stretches
        // the result over the window afterwards.
        display::world_camera_target(&scene_target),
        // The ears go where the eyes are, so what is on the left of the screen
        // is on the left of the mix. Nothing in a build without an audio
        // backend, which is why it is named unconditionally.
        audio::listener(),
        Transform::from_xyz(-13.0, 10.0, 56.0).looking_at(Vec3::new(-13.0, 4.0, 46.0), Vec3::Y),
        Projection::from(PerspectiveProjection {
            fov: 60_f32.to_radians(),
            near: 0.05,
            far: 1000.0,
            ..default()
        }),
        FollowCamera::default(),
        // N64 colours are already display-referred bytes. Filmic HDR grading
        // would alter their contrast, saturation, and hue a second time.
        Tonemapping::None,
        // Named rather than left to the default, which is *not* off:
        // `bevy_render` registers `Msaa` as a required component of every
        // `Camera` and `Msaa::default()` is `Sample4`, so a camera that says
        // nothing is a camera running four-times multisampling. That is four
        // times the fragment work and four times the render-target bandwidth --
        // the cost that hurts most on exactly the weak integrated GPU this is
        // meant to run on -- and it buys nothing here. The world is vertex-lit
        // with hard-edged N64 texture work and is then resampled onto the
        // window nearest-neighbour by `display`, which throws away smoothed
        // edges anyway. `display::presentation_camera` has always said this;
        // the camera that draws the expensive pass had not.
        Msaa::Off,
        // Bevy fogs per camera rather than per scene, so the medium the camera
        // is in rides along with it.
        water::air_fog(),
    ));
    // The second camera: it draws nothing of its own, only the node holding
    // the world's image and every other piece of UI on top of it. Being the
    // one camera left whose target is the window is also what makes it the
    // camera Bevy hands the UI to, with nothing marked.
    commands.spawn(display::presentation_camera());
    commands.spawn(display::scene_view_bundle(&scene_target));
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
    commands.spawn((
        FpsText,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::srgb(0.55, 1.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(16.0),
            top: Val::Px(12.0),
            ..default()
        },
    ));
    frame_chart::spawn(&mut commands);
    // The player's bar and the pool of floating ones. Built once here rather
    // than per creature -- see `health::UNIT_BARS`.
    health::spawn(&mut commands);
    commands.spawn(console::panel_bundle());
    commands.spawn(console::tuning_tray_bundle());
    menu::spawn(&mut commands);
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
}

/// Writes the corner frame-rate readout.
///
/// Smoothed rather than instantaneous: the raw per-frame number swings far too
/// fast to read, and what anyone looking at it wants to know is what the frame
/// rate *is*, not what one particular frame cost. The frame time comes along
/// with it because it is the number that scales linearly with work -- twice the
/// milliseconds is twice the cost, where a drop from 240 to 200 fps and one
/// from 60 to 55 look alike and are not.
/// The entity and enemy counts ride along with it because this readout is what
/// a `crowd` benchmark is read off, and a frame time means nothing without the
/// size of the field that produced it. The entity count is the one that catches
/// the cost that is not obvious: an enemy is not one entity but a whole scene of
/// them, and the difference between those two numbers is most of the frame.
fn update_fps(
    diagnostics: Res<DiagnosticsStore>,
    enemies: Query<(), With<enemy::Enemy>>,
    crowd: Res<impostor::ImpostorStats>,
    mut text: Query<&mut Text, With<FpsText>>,
) {
    let Ok(mut readout) = text.single_mut() else {
        return;
    };
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|fps| fps.smoothed());
    let frame_time = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|frame_time| frame_time.smoothed());
    let entities = diagnostics
        .get(&EntityCountDiagnosticsPlugin::ENTITY_COUNT)
        .and_then(|count| count.value());
    // The worst of the last 120 frames -- two seconds of them at sixty, half a
    // second at two hundred and forty. A stutter is by definition not in the
    // smoothed number: it is one frame in a hundred costing ten times the rest,
    // which an average buries and this reports. Read it while doing the thing
    // that hitches and it says how big the hitch was and, by how long it takes
    // to clear, roughly when.
    let worst = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .map(|frame_time| frame_time.values().fold(0.0_f64, |most, ms| most.max(*ms)))
        .filter(|worst| *worst > 0.0);
    **readout = match (fps, frame_time) {
        (Some(fps), Some(frame_time)) => {
            let mut line = format!("{fps:.0} fps · {frame_time:.1} ms");
            if let Some(worst) = worst {
                line.push_str(&format!(" · worst {worst:.1} ms"));
            }
            if let Some(entities) = entities {
                line.push_str(&format!(
                    " · {} enemies ({} sprite / {} skinned{}) · {entities:.0} entities",
                    enemies.iter().count(),
                    crowd.sprites,
                    crowd.skinned,
                    // Silent otherwise, and this is the number that says the
                    // far crowd is not being drawn at all.
                    if crowd.missing > 0 {
                        format!(" / {} UNDRAWN", crowd.missing)
                    } else {
                        String::new()
                    },
                ));
            }
            line
        }
        // The diagnostic needs a few frames before it has anything to average,
        // and a blank corner says that better than a zero would.
        _ => String::new(),
    };
}

/// Puts the window back into a window, and back into fullscreen again.
///
/// Fullscreen is the default, and a game that starts fullscreen with no way out
/// of it is a game you have to kill to get your desktop back.
fn toggle_fullscreen(
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Query<&mut Window, With<PrimaryWindow>>,
) {
    // Not gated on the console being shut, unlike everything else that reads a
    // key. Being stuck fullscreen because the console is up is the exact trap
    // this exists to get out of, and F11 types nothing into the prompt anyway.
    if !keys.just_pressed(KeyCode::F11) {
        return;
    }
    let Ok(mut window) = window.single_mut() else {
        return;
    };
    window.mode = match window.mode {
        WindowMode::Windowed => WindowMode::BorderlessFullscreen(MonitorSelection::Current),
        _ => WindowMode::Windowed,
    };
}

fn update_hud(
    state: Res<GameState>,
    input: Res<InputState>,
    loadout: Res<weapon::Loadout>,
    squad: Res<squad::Squad>,
    player: Query<(&Controller, &health::Health), With<Player>>,
    mut text: Query<&mut Text, With<Hud>>,
) {
    let Ok((ctrl, health)) = player.single() else {
        return;
    };
    let Ok(mut hud) = text.single_mut() else {
        return;
    };
    // A single-run text node is written through as a whole string now that
    // extra runs are child entities rather than a `sections` vector.
    **hud = if state.debug {
        let device = if input.pad { "gamepad" } else { "keyboard" };
        let weapon = loadout.equipped.spec().name;
        let following = squad.members.len();
        let marching = squad.marching();
        let holding = squad.sent.len() - marching;
        format!("Super Bevy World 64\n{:?}  ·  {:?}  ·  Health {}/{}  ·  {device}\nSquad {following} following · {marching} marching · {holding} holding\nWASD move · mouse look · Space jump · V jetpack/skate\nShift attack · X squad (hold to whistle, tap to send)\nF/right mouse aim · Y weapon ({weapon}) · ` console · F2 switch · Esc menu · F11 window", state.active, ctrl.motion, health.current, health.max)
    } else {
        String::new()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{asset::AssetPlugin, ecs::schedule::Schedule, world_serialization::WorldAsset};
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
    ///
    /// Crate-visible because [`crate::world`]'s planet test needs the same
    /// game with a glTF loader bolted on: it is the one test whose subject is
    /// what happens *after* an asset arrives, and it must not be testing a
    /// second hand-written copy of this list.
    pub(crate) fn headless() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            // Input has no window behind it here; the resources exist and read
            // as nothing pressed, which is what the systems consume.
            .add_plugins(bevy::input::InputPlugin)
            .add_plugins(AssetPlugin {
                file_path: asset_path().to_string_lossy().into_owned(),
                ..default()
            })
            .add_plugins(FrameTimeDiagnosticsPlugin::default())
            .add_plugins(EntityCountDiagnosticsPlugin::default())
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
            // The planet's collision is read back out of a loaded glTF, so
            // `world::finish_planet` asks for the meshes inside one. Nothing
            // ever finishes loading here, which is the not-ready path it takes
            // on the real game's first frames -- but the store has to exist for
            // the system to be allowed to run at all.
            .init_asset::<bevy::gltf::GltfMesh>()
            .init_asset::<bevy::gltf::GltfNode>()
            // A test loop runs far faster than real time, so without this the
            // clock would barely advance and the fixed step would never tick:
            // the simulation would go unexercised while the test still passed.
            // Sixteen milliseconds a frame is a 60 Hz session.
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                std::time::Duration::from_millis(16),
            ));
        // The same resources and the same schedules the real game gets. Listed
        // in one place rather than two, because the copy that used to live here
        // fell behind the real one and every schedule test failed at once.
        game_resources(&mut app);
        game_systems(&mut app);
        register_world_asset_types(&mut app);
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

    /// The crowd benchmark: the real game, a real GPU, no window, and a field
    /// of a given size.
    ///
    /// Ignored by default because it opens a device and takes tens of seconds;
    /// it is a measuring instrument rather than a thing that can fail. Run it
    /// with
    ///
    /// ```text
    /// cargo test --release -- --ignored --nocapture crowd_benchmark
    /// ```
    ///
    /// and it prints a row per field size. `--release` is not optional: this
    /// crate builds at `opt-level = 1` in dev, and a debug-speed simulation
    /// tells you about `rustc` rather than about the game.
    ///
    /// Everything the game does is here except presenting to a surface: the
    /// full plugin set, the real `setup`, the real schedules, and the world
    /// camera drawing into the same offscreen target `display` gives it. What
    /// that leaves out is the swap chain and vsync, which is exactly what a
    /// benchmark wants left out.
    #[test]
    #[ignore = "measuring instrument: needs a GPU and tens of seconds"]
    fn crowd_benchmark() {
        use bevy::window::ExitCondition;
        use std::time::{Duration, Instant};

        /// Frames run before timing starts, to get the assets loaded, the
        /// scenes spawned and the pipelines compiled. A first frame that
        /// includes compiling every shader in the game is not a frame.
        const WARMUP: usize = 90;
        const TIMED: usize = 120;

        // `CROWD_BENCH=2000` runs one size instead of the sweep, which is what
        // makes it usable for A/B-ing a single change: toggle the thing, run
        // one row, compare. The default is the whole sweep.
        let sweep: Vec<(usize, &str)> = match std::env::var("CROWD_BENCH") {
            Ok(sizes) => sizes
                .split(',')
                .filter_map(|size| size.trim().parse().ok())
                .map(|count| (count, "mix"))
                .collect(),
            Err(_) => vec![
                (0, "empty"),
                (500, "mix"),
                (1000, "mix"),
                (2000, "mix"),
                (2000, "ant"),
            ],
        };
        for (count, kind) in sweep {
            let mut app = App::new();
            app.add_plugins(
                DefaultPlugins
                    .build()
                    .disable::<bevy::winit::WinitPlugin>()
                    .set(WindowPlugin {
                        primary_window: None,
                        exit_condition: ExitCondition::DontExit,
                        close_when_requested: false,
                        ..default()
                    })
                    .set(AssetPlugin {
                        file_path: asset_path().to_string_lossy().into_owned(),
                        ..default()
                    }),
            );
            add_game(&mut app);
            // A real clock would let the loop outrun the fixed step and leave
            // the simulation half-exercised. Sixteen milliseconds a frame is
            // the 60 Hz session this is trying to hold.
            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_millis(16),
            ));

            app.finish();
            app.cleanup();
            app.update();
            // `CROWD_DRAW=60` runs the sweep with the skinned-model cull pulled
            // in to sixty metres. Worth having as a knob because the shipped
            // default is 140, which is wider than the castle is across -- so out
            // of the box the cull never fires at all and everything downstream
            // of it, including stopping a culled enemy's animation, is dead
            // code on this map.
            if let Ok(budget) = std::env::var("CROWD_SIM")
                .ok()
                .map_or(Err(()), |v| v.parse::<f32>().map_err(|_| ()))
            {
                app.world_mut()
                    .resource_mut::<console::GameTuning>()
                    .sim_budget = budget;
            }
            let draw = std::env::var("CROWD_DRAW")
                .ok()
                .and_then(|value| value.parse::<f32>().ok());
            if let Some(draw) = draw {
                app.world_mut()
                    .resource_mut::<console::GameTuning>()
                    .enemy_draw = draw;
            }
            {
                let mut tuning = app.world_mut().resource::<console::GameTuning>().clone();
                let mut console = app.world_mut().resource_mut::<console::ConsoleState>();
                console.execute(&format!("crowd {count} {kind}"), &mut tuning);
            }
            for _ in 0..WARMUP {
                app.update();
            }

            // `count_spawned` rather than `len`, which reports the entity
            // meta-list's capacity and so always comes out a power of two.
            let entities = app.world().entities().count_spawned();
            let started = Instant::now();
            for _ in 0..TIMED {
                app.update();
            }
            let each = started.elapsed().as_secs_f64() / TIMED as f64;
            println!(
                "crowd {count:>5} {kind:<11} {:>7.2} ms/frame  {:>6.1} fps  {entities:>7} entities",
                each * 1000.0,
                1.0 / each,
            );
        }
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
    /// Each pipe fires every few seconds and its slime walks straight at the
    /// player. Before the combat rules were ported in full, that slime threw
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
    /// Three pipes and three different broods: slimes out of one, ants
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
            let mut brood = app
                .world_mut()
                .query_filtered::<&Transform, With<pipe::Brood>>();
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
        for kind in [enemy::Kind::Slime, enemy::Kind::Ant] {
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

    /// The world goes into the render target and the UI goes onto the window.
    ///
    /// Two things break silently if this ever stops holding, and neither one
    /// is a compile error. Point the world camera back at the window and the
    /// render-resolution setting quietly does nothing. Give the world camera
    /// the higher order, or the window as a target, and Bevy hands the UI to
    /// *it* instead -- so the HUD and the menu are drawn into the
    /// low-resolution image and come out blurred.
    #[test]
    fn the_world_is_drawn_into_the_target_and_the_ui_onto_the_window() {
        use bevy::camera::RenderTarget;

        let mut app = headless();
        app.update();

        let target = app.world().resource::<SceneTarget>().0.clone();
        let mut world = app
            .world_mut()
            .query::<(&Camera, &RenderTarget, &Camera3d)>();
        let (world_order, world_target) = world
            .single(app.world())
            .map(|(camera, target, _)| (camera.order, target.clone()))
            .expect("there should be exactly one camera drawing the world");
        assert_eq!(
            world_target.as_image(),
            Some(&target),
            "the world camera draws into the render target"
        );

        let mut presentation = app
            .world_mut()
            .query::<(&Camera, &RenderTarget, &Camera2d)>();
        let (presentation_order, presentation_target) = presentation
            .single(app.world())
            .map(|(camera, target, _)| (camera.order, target.clone()))
            .expect("there should be exactly one camera showing it");
        assert!(
            matches!(presentation_target, RenderTarget::Window(_)),
            "the presentation camera is the one that draws to the window"
        );
        assert!(
            presentation_order > world_order,
            "the stretch has to happen after the frame it stretches"
        );
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
        initialise(input_pipeline());
    }

    #[test]
    fn startup_has_no_conflicting_queries() {
        initialise(setup.into_configs());
    }

    /// A key press must be read once, not twice.
    ///
    /// Bevy's `keyboard_input_system` clears and refills `ButtonInput` in
    /// `PreUpdate`, the same schedule the game reads it in. Both touch the same
    /// resource, so the executor serialises them -- but until
    /// [`input_pipeline`] says `after`, *which order* is down to thread timing,
    /// and an order that flips between frames reads one press on both sides of
    /// the clear. Every edge the pipeline latches is a toggle, so a doubled
    /// read cancels itself and the key appears dead.
    ///
    /// Bevy can prove the ordering exists rather than the test having to catch
    /// a race in the act: ambiguity detection fails the schedule build when two
    /// systems share access with no order between them.
    #[test]
    fn the_compute_pool_keeps_its_cap_unless_the_environment_names_one() {
        assert_eq!(compute_thread_cap(None), COMPUTE_THREADS);
        assert_eq!(compute_thread_cap(Some("8")), 8);
        assert_eq!(compute_thread_cap(Some(" 2 ")), 2);
        // A pool of no threads would never run a system, so these fall back.
        assert_eq!(compute_thread_cap(Some("0")), COMPUTE_THREADS);
        assert_eq!(compute_thread_cap(Some("")), COMPUTE_THREADS);
        assert_eq!(compute_thread_cap(Some("lots")), COMPUTE_THREADS);
    }

    #[test]
    fn keys_are_read_after_bevy_refills_them() {
        use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::input::InputPlugin)
            .init_resource::<console::ConsoleState>()
            .init_resource::<console::GameTuning>()
            .init_resource::<enemy::Threats>()
            .init_resource::<input::InputState>()
            .init_resource::<menu::MenuState>()
            .init_resource::<display::DisplaySettings>()
            // The display page's lighting row writes into it.
            .init_resource::<n64::N64Lighting>()
            // The menu's level page asks for levels, and reads which one is up.
            .init_resource::<world::LevelId>()
            .init_resource::<world::LevelLoad>()
            .add_message::<world::LoadLevel>()
            .add_systems(PreUpdate, input_pipeline());
        app.edit_schedule(PreUpdate, |schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Error,
                ..default()
            });
        });
        app.finish();
        app.cleanup();
        // Panics with the ambiguity spelled out if the ordering is ever dropped.
        app.update();
    }
}
