#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! Space Crusaders.
//!
//! Comments throughout cite paths under `app/` and `sm64py/`. Those are the
//! Panda3D implementation this game was ported from, which was removed once
//! the port took over; they are provenance for a constant or a rule rather
//! than files to open, and `git log` still has them if one needs reading.

mod action;
mod aim;
mod animation;
mod audio;
mod autopilot;
mod billboard;
mod camera;
mod collide;
mod console;
mod display;
mod enemy;
mod energy;
mod flatten;
mod flow;
mod frame_chart;
mod furniture;
mod goap;
mod gravity;
mod health;
mod impostor;
mod input;
mod level;
mod menu;
mod n64;
mod nuclonium;
mod orbit;
mod orrery;
mod path;
mod pipe;
mod player;
mod pylon;
mod route;
mod shadow;
mod shot;
mod sky;
#[cfg(feature = "spike")]
mod spike;
mod squad;
mod stellarator;
mod structure;
mod water;
mod weapon;
#[cfg(test)]
mod workbench;
mod world;

use bevy::{
    app::{TaskPoolOptions, TaskPoolPlugin},
    core_pipeline::tonemapping::Tonemapping,
    diagnostic::{DiagnosticsStore, EntityCountDiagnosticsPlugin, FrameTimeDiagnosticsPlugin},
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    input::InputSystems,
    post_process::bloom::{Bloom, BloomPrefilter},
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

/// Who is on screen: the one the player is driving, and -- since an ally is a
/// character too -- who each of the squad is.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub enum ActiveCharacter {
    #[default]
    Luna,
    Mario,
}

impl ActiveCharacter {
    /// Both playable characters, for anything that has to do a thing per
    /// character rather than for the one in hand.
    pub const ALL: [ActiveCharacter; 2] = [ActiveCharacter::Luna, ActiveCharacter::Mario];

    /// The scene this character is drawn from, and the scale it is drawn at.
    ///
    /// One place rather than four. The player's two visuals, the squad's
    /// allies and anything else that ever puts a character in the world all
    /// ask here, so a Luna the AI is driving is the same model at the same
    /// size as the Luna the player drives -- and re-exporting either model at
    /// a different size is one edit.
    ///
    /// The scales are the models', not a style choice: Luna's glTF is authored
    /// a little over life size and Mario's is in SM64 units, which are a
    /// hundred to the metre.
    pub fn model(self) -> (&'static str, f32) {
        match self {
            ActiveCharacter::Luna => ("luna/luna.glb#Scene0", 0.81),
            ActiveCharacter::Mario => ("mario/mario.glb#Scene0", 0.00667),
        }
    }

    /// How much punishment one of these takes as an ally.
    ///
    /// Luna is the character the game is balanced around and it shows: an AI
    /// Luna is worth five Marios in a fight, which is what makes the choice of
    /// who to fill the field with a choice at all.
    pub fn ally_health(self) -> i32 {
        match self {
            ActiveCharacter::Luna => health::PLAYER_HEALTH,
            ActiveCharacter::Mario => health::MARIO_HEALTH,
        }
    }

    /// What to call one of these in the console and the HUD.
    pub fn name(self) -> &'static str {
        match self {
            ActiveCharacter::Luna => "Luna",
            ActiveCharacter::Mario => "Mario",
        }
    }
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
                let _ = writeln!(file, "Space Crusaders stopped with:\n\n{info}");
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
///   COMPUTE_THREADS=8 ./SpaceCrusaders.exe
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
                    title: "Space Crusaders".into(),
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
    // The saved console rows, loaded before `add_game`'s `init_resource` can
    // put the defaults there instead -- and written back as they change.
    // Only here, in the windowed game: tests, screenshots and benchmarks run
    // on the defaults, so yesterday's slider cannot change today's numbers.
    app.insert_resource(console::GameTuning::load_saved());
    add_game(&mut app);
    app.add_systems(Update, console::persist);
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
    app.add_plugins((n64::N64Plugin, nuclonium::VfxPlugin))
        // Enough raw history for the chart's four-second window at 240 Hz.
        .add_plugins(FrameTimeDiagnosticsPlugin::new(960))
        // The other half of the benchmark readout: an enemy is a whole scene of
        // entities rather than one, and that multiplier is what the crowd work
        // is trying to bring down.
        .add_plugins(EntityCountDiagnosticsPlugin::default());
    register_world_asset_types(app);
    game_systems(app);
    // **Here rather than in `game_systems`, which is the shared list, because
    // this one needs a renderer.** `Gizmos` is backed by `GizmoPlugin`, which
    // `DefaultPlugins` brings along behind the `bevy_gizmos` feature -- and
    // which cannot be added to the headless harness, because its own systems
    // want resources only the render app has. A `Gizmos` parameter with nothing
    // behind it does not quietly draw nothing; it fails validation and takes
    // the frame with it. So the debug overlay is a thing the windowed game has
    // and the test harness does not, exactly as the render passes are.
    //
    // `enemy::draw_crawlers` rides on the same switch and for the same reason
    // it cannot ride in `path::draw`: an ant is never routed, so the route
    // overlay has nothing to say about one, and a blank where a body should be
    // reads as a broken body. See that system for what it draws instead.
    // After `sync_visual` -- and through it, after `orbit::glide` -- or the
    // ordering is the scheduler's to pick. Picked wrong, the overlays read
    // the gravity and the collision while they still hold the *tick* poses
    // (`orbit::advance` re-points them at the top of every fixed step,
    // before any Update system runs) and last frame's `RenderPose` besides:
    // the local-down arrow and the gravity shells then saw back and forth at
    // 30 Hz across a world that glides, which on the orbiting level was a
    // genuinely nauseating thing to stand under.
    app.add_systems(Startup, path::configure).add_systems(
        Update,
        (
            path::draw,
            enemy::draw_crawlers,
            collide::draw,
            orrery::draw,
            orrery::readout,
        )
            .after(player::sync_visual),
    );
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
            active: ActiveCharacter::Luna,
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
        .init_resource::<path::Pathing>()
        .init_resource::<action::Action>()
        .init_resource::<squad::Whistle>()
        // The ring the whistle leaves where an order landed. Beside the
        // whistle itself because the one writes the other.
        .init_resource::<squad::OrderMark>()
        .init_resource::<stellarator::Build>()
        // The pylon network and the key that plants one. Beside the machine's
        // build state because they are the same control in a different hand.
        .init_resource::<pylon::Plant>()
        .init_resource::<pylon::Network>()
        // The one die in the game -- see `nuclonium::Drops::maybe` -- and what the
        // squad has managed to ship back down the beams.
        .init_resource::<nuclonium::Drops>()
        .init_resource::<nuclonium::Bank>()
        // The grab circle's own hold, beside the squad whistle's: same gesture,
        // same shape, two circles that must not resize each other.
        .init_resource::<nuclonium::Grab>()
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
        .init_resource::<orbit::SolarSystem>()
        .init_resource::<autopilot::Autopilot>()
        .init_resource::<flatten::Curve>()
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
        // Anywhere in the frame: it only ever *adds* a marker, and a mesh
        // spawned this frame is culled honestly for one frame at worst.
        .add_systems(Update, flatten::uncull)
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
    // `action::choose` reads the keyboard for the Tab picker; `action::route`
    // then points X at whatever it settled on, between the snapshot being taken
    // and anything reading it. See [`action`] for why that indirection exists.
    (
        console::input,
        menu::input,
        input::gather,
        action::choose,
        action::route,
    )
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
        // Nested rather than two more entries: Bevy's system tuples stop at
        // twenty. `pylon::supply` runs straight after the step that moved him,
        // so the bar he is filling is filled against where he is now rather
        // than where he was. It only ever adds, so its place in the tick is a
        // matter of which pylon it measures against and nothing else.
        // `squad::steady` goes first in the whole tick, and that placement is
        // its correctness rather than tidiness: the drawn frames since the last
        // tick have been showing every Mario somewhere between two poses, and
        // this puts the simulated one back before anything reads it. See
        // [`squad::Glide`], and `squad::bank` at the bottom of this list, which
        // is the other half of it.
        // `player::remember` opens the tick before anything can move him:
        // what it files is the pose the drawn frames blend *from*, and it has
        // to be taken before `orbit::advance` rides him round with his planet
        // or the ride falls outside the blend window and the model buzzes in
        // place on the moving ground. Then the clockwork turns -- the ground
        // the player is about to be resolved against has to be the ground as
        // it stands this tick -- and the autopilot reads its target's
        // position off that same fresh state before the movement burns
        // towards it.
        (
            player::remember,
            orbit::advance,
            autopilot::select,
            squad::steady,
            player::movement,
            pylon::supply,
        )
            .chain(),
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
        // Decide, walk, and then pick up whatever the walk arrived at, in that
        // order. `goap::plan` scores every job a Mario could be doing this tick
        // and writes down the winner; `move_allies` walks to it and knows
        // nothing about why; `haul` goes last so a Mario that reached a ball
        // this tick is holding it this tick rather than next.
        //
        // Nested rather than three more entries, for the reason the tuple above
        // is: Bevy's system tuples stop at twenty.
        // Nested rather than three more entries, for the reason the tuple above
        // is: Bevy's system tuples stop at twenty.
        //
        // `escort` decides what joins Luna's train and what leaves it, and
        // reaches out from every live mast for whatever ended up beside it --
        // which is the one place a ball is handed over however it arrived.
        // Where the train actually swims to is `nuclonium::swim`, per drawn
        // frame; the whistle that fills it is `nuclonium::call`, likewise. See
        // both for why those two are drawn rather than simulated.
        //
        // `path::plan` sits between the decision and the walk, and that is the
        // only ordering it needs: a destination written this tick is routed
        // this tick, and a Mario whose turn the budget did not reach walks last
        // tick's route -- or straight at its goal -- and comes round again.
        //
        // `enemy::navigate` rides in front of it for exactly that reason and
        // is why this nest holds a system with nothing to do with Marios:
        // **two deciders, one router.** The enemies want the same searches out
        // of the same budget and want them grouped against the Marios' rather
        // than beside them, so both say where they are going before the one
        // system that works out the way. What it reads is last tick's aggro --
        // `enemy::alert` runs further down -- which is a tick of staleness
        // against `path::DRIFT`, three metres, and so is no staleness at all.
        (
            enemy::navigate,
            goap::plan,
            path::plan,
            squad::move_allies,
            nuclonium::haul,
            nuclonium::escort,
        )
            .chain(),
        // Before `enemy::combat`, so a shot and a swing in the same tick are
        // resolved in the order the trigger was pulled rather than the swing
        // silently winning. Both take the same latched edge and only one of
        // them is allowed to, so in practice they never both act -- but the
        // order is the cheap half of making that true.
        (weapon::swap, weapon::fire).chain(),
        // The player's blows, and the window that rate-limits the ones that
        // land on something which cannot walk away. `recover` goes first so a
        // window opened last tick has been counted down before this tick's
        // attackers arrive; it is its own system rather than a line inside
        // `siege` because two systems hit buildings, and a window counted down
        // inside one of them would run at a different rate -- or not at all --
        // in a level with no enemies in it.
        //
        // `demolish` is the sword against the nests, straight after the sword
        // against the crowd: one swing, two kinds of thing it might land on,
        // and the building's own window is what stops it spending twice.
        (structure::recover, enemy::combat, structure::demolish).chain(),
        // After the walk step: a Mario mid-punch is punching, whatever the walk
        // made of it.
        enemy::ally_combat,
        // And the other two sides of the same fight, straight after it, so a
        // Mario that killed what it was hitting is not then hurt by the thing it
        // just removed. `siege` is `maul` pointed at the things that cannot walk
        // away. All three raise threats, and `alert` below drains them together
        // -- a mast going down is an act, and the squad ought to hear about it
        // on the tick it happens.
        (enemy::maul, structure::siege).chain(),
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
        // Last of the things that kill: every drop this tick has been queued by
        // now, so a ball is on the ground on the tick the thing that left it
        // died. `ship` beside it, flying the ones already on their way home.
        //
        // `health::mend` rides in the same nest -- Bevy's tuples stop at twenty
        // and this list is at it -- and belongs beside them anyway: it is the
        // red half of the same drop, handed to whoever ran over it, and it goes
        // after `shed` so one dropped this tick is on the ground before anybody
        // is asked whether they are standing on it.
        // `linger` rides along too: it is the clock on a ball nobody came
        // for, and it goes after `mend` so a red one absorbed this tick is not
        // also aged this tick.
        (
            nuclonium::shed,
            nuclonium::ship,
            health::mend,
            nuclonium::linger,
        )
            .chain(),
        // The feet come round after the walk step that decided where he was
        // facing, for the same reason `ally_combat` follows the walk.
        //
        // And `squad::bank` last of everything, because "where this tick left
        // the Marios" is only true once nothing else is going to move one --
        // the walk, the fight, the warp pipe's arc are all above it. See
        // [`squad::Glide`].
        (aim::turn_body, squad::bank).chain(),
    )
        .chain()
        .run_if(console::is_closed)
        .run_if(menu::is_closed)
}

/// Everything that runs per rendered frame while the console is closed.
fn presentation() -> ScheduleConfigs<ScheduleSystem> {
    (
        // Nested rather than two entries, for the reason every other nest in
        // this list is: Bevy's system tuples stop at twenty. `glide` does for
        // the squad exactly what `sync_visual` does for Luna -- draws it
        // between two fixed ticks -- and one without the other is a squad
        // stuttering along behind a leader who glides. See [`squad::Glide`].
        // Both before the camera, which frames her, and long before
        // `nuclonium::swim`, which swims a carried ball after the pose its
        // carrier is actually drawn at.
        // `orbit::glide` first in the whole frame: it re-points the world --
        // scenery, collision, gravity -- at the render-rate pose, and every
        // line below that probes the level or asks which way is up has to
        // ask about the world as this frame draws it, not as the last tick
        // simulated it. The camera especially: its boom probe and its idea
        // of the horizon both chatter at 30 Hz otherwise.
        // `flatten::chart` closes the chain: it anchors the frame's flat-map
        // to the player's freshly-written `RenderPose` over the world's
        // blended centre, so the map and the terrain it bends agree about
        // which frame is being painted.
        (
            orbit::glide,
            player::sync_visual,
            squad::glide,
            flatten::chart,
        )
            .chain(),
        camera::update,
        animation::resolve_clips,
        animation::claim_players,
        animation::attach_graphs,
        animation::track_player,
        animation::update,
        // Chained rather than two entries, and not only for the
        // tuple-of-twenty reason: `whistle` leaves the mark that `order_ring`
        // draws, so an order given this frame should be ringed this frame.
        (squad::whistle, squad::order_ring).chain(),
        squad::animate_allies,
        // The build button and the plasma it draws, and then the pylons.
        // Beside the whistle because it is the same control in a different
        // hand: one aim, one hold, one release. Nested because Bevy's system
        // tuples stop at twenty, and chained because the pylons follow the
        // machines rather than lead them -- a network rebuilt this frame
        // should be looking at the stellarator that went up this frame rather
        // than at last frame's world. See [`stellarator`] and [`pylon`].
        // Then the balls the squad carries between them, which is all
        // presentation: a loose one bobs and turns on wall-clock time, and
        // nothing in the simulation reads where it ended up. Nested in with the
        // machines and the masts for the tuple-of-twenty reason, and it belongs
        // there anyway -- it is the third thing the network is made of.
        // `call` first, because it is a button and the circle it draws is
        // aimed with this frame's camera; then `swim`, which glides the train
        // it filled after the player's *rendered* pose; then the look, and
        // then the trail, which records where all of that ended up.
        (
            stellarator::systems(),
            pylon::systems(),
            nuclonium::call,
            nuclonium::swim,
            nuclonium::shimmer,
            nuclonium::glow,
            nuclonium::trail,
        )
            .chain(),
        // Nested rather than five entries, because Bevy's system tuples stop
        // at twenty and this outer one is full.
        (
            water::drift,
            water::adopt_surfaces,
            water::find_ocean,
            water::drift_ocean,
            water::camera_medium,
        )
            .chain(),
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
        (
            controls,
            update_hud,
            health::draw_player_bar,
            energy::draw_player_bar,
            // After the burn they are reporting on, so "braking" reaches the
            // console -- and the bracket turns amber -- the frame the brake
            // begins and not the frame after.
            autopilot::hud,
            autopilot::report,
        )
            .chain(),
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
        // And straight after that, so the `test_world` fixtures follow the
        // planet whose collision and gravity they are filed into.
        world::finish_fixtures,
        // Out here rather than in `simulation` because the console is open at
        // the moment a `crowd` command is typed, and a field that only arrived
        // once you shut the console is a field you never saw arrive.
        enemy::crowd,
        // Straight after it, because the two share the console's request queue
        // and each hands back what the other one wanted. See `ConsoleState::defer`.
        weapon::equip,
        // All three out here rather than in `presentation` because a scene
        // finishes loading whenever it does, and a console left open must not
        // be the difference between a machine with a blue balloon inside it
        // and one without, a mast that breathes and one that does not, or a
        // lawn with the level's baked tree shadows still on it and one
        // without. Nested for the tuple-of-twenty reason.
        (stellarator::claim, pylon::claim, shadow::shed_baked),
        // Straight after `enemy::crowd` and `weapon::equip` in spirit: the
        // three share the console's request queue and each hands back what the
        // others wanted. See `ConsoleState::defer`.
        pylon::command,
        // Straight after it, and for the third time in this chain the reason is
        // `ConsoleState::defer`: these systems share one request queue and each
        // hands back what the others asked for.
        nuclonium::command,
        stellarator::command,
        enemy::sync_animation_visibility,
        audio::play,
        console::draw,
        // Before the menu is drawn rather than after, so the resolution the
        // menu reads back out of the target is the one the row above it just
        // asked for rather than last frame's.
        display::resize,
        menu::draw,
        // The X-button picker, beside the menu because it is the same kind of
        // thing: an overlay that stays legible whatever the game underneath it
        // is doing, including being paused with the console up.
        action::draw,
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
        .filter(|path| path.join("luna/luna.glb").is_file())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets"))
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sprites: ResMut<Assets<n64::N64Material>>,
    mut glows: ResMut<Assets<nuclonium::GlowMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut console: ResMut<console::ConsoleState>,
    mut load: ResMut<world::LevelLoad>,
    tuning: Res<console::GameTuning>,
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
    // first. `sky::advance` is what turns it off on a level with no sky of
    // its own.
    sky::prepare(&mut commands, &mut meshes, &mut images, &mut sprites);
    squad::spawn_circle(&mut commands, &mut meshes, &mut materials);
    // The build preview, which outlives a level exactly as the whistle ring
    // does: the thing you build with must not go away when you change level.
    stellarator::prepare(&mut commands, &mut meshes, &mut materials);
    // The pylon's own preview ring and the beams' shared art, put up once for
    // the same reason: what you build with outlives the level you build it on.
    pylon::prepare(&mut commands, &mut meshes, &mut materials);
    // And the shared core, glow, trail and wash art, for the same reason again:
    // a kill should cost one gameplay entity and no render-side allocation.
    nuclonium::prepare(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut glows,
        &mut images,
    );
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
        &tuning,
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
        // Which world currently holds him, and how hard -- the parenting
        // record the orbiting level keeps so he is drawn through his
        // planet's own frame. Inert everywhere else. See [`orbit::Rider`].
        orbit::Rider::default(),
        // A hundred points, which is a different kind of number from the three
        // hearts this used to be: what threatens him is a crowd's worth of
        // small hits rather than any one enemy. See [`health`].
        health::Health::new(health::PLAYER_HEALTH),
        // The other pool: what the booster burns and the gun taxes. Only the
        // player carries one -- see [`energy`].
        energy::Energy::new(),
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
    // Both visuals off `ActiveCharacter::model`, so the player's Luna and an
    // AI one are the same model at the same size by construction.
    for character in ActiveCharacter::ALL {
        let (model, scale) = character.model();
        commands.spawn((
            PlayerVisual,
            character,
            shadow,
            WorldAssetRoot(assets.load(model)),
            // Only the character in hand is drawn, and Luna is who the game
            // starts on.
            match character {
                ActiveCharacter::Luna => Visibility::Inherited,
                _ => Visibility::Hidden,
            },
            Transform::from_scale(Vec3::splat(scale)),
        ));
    }

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
        // Only values above the display range contribute: the world keeps its
        // authored N64 colours, while the HDR nuclonium material supplies the
        // energy this old-school bloom scatters around each orb and mote.
        Bloom {
            prefilter: BloomPrefilter {
                threshold: 1.1,
                threshold_softness: 0.2,
            },
            ..Bloom::OLD_SCHOOL
        },
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
    // Under it, and pinned to the corner the health bar is stacked on top of.
    energy::spawn(&mut commands);
    commands.spawn(console::panel_bundle());
    commands.spawn(console::tuning_tray_bundle());
    menu::spawn(&mut commands);
    action::spawn(&mut commands);
    autopilot::spawn_hud(&mut commands);
    orrery::spawn(&mut commands);
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
        state.active = if state.active == ActiveCharacter::Luna {
            ActiveCharacter::Mario
        } else {
            ActiveCharacter::Luna
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
    tuning: Res<console::GameTuning>,
    mut text: Query<&mut Text, With<FpsText>>,
) {
    let Ok(mut readout) = text.single_mut() else {
        return;
    };
    if tuning.debug <= 0.5 {
        **readout = String::new();
        return;
    }
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

#[allow(clippy::too_many_arguments)]
fn update_hud(
    state: Res<GameState>,
    input: Res<InputState>,
    loadout: Res<weapon::Loadout>,
    squad: Res<squad::Squad>,
    bank: Res<nuclonium::Bank>,
    tuning: Res<console::GameTuning>,
    pathing: Res<path::Pathing>,
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
    // F1 toggles the text; the console's `debug 0` overrules it along with
    // every other piece of standing debug furniture.
    **hud = if state.debug && tuning.debug > 0.5 {
        let device = if input.pad { "gamepad" } else { "keyboard" };
        let weapon = loadout.equipped.spec().name;
        let following = squad.members.len();
        let marching = squad.marching();
        let holding = squad.sent.len() - marching;
        let stored = bank.stored;
        // Only while the overlay is on. The counters are how you tell a field
        // that is re-planning constantly from one that is walking -- see
        // [`path::Pathing`] -- and they are noise the rest of the time.
        let paths = match tuning.path_debug > 0.0 {
            true => format!(
                "\nPaths {} walking · {} queued in {} groups · {} searched · {} direct · {} partial · {} lost · {} cells",
                pathing.routed,
                pathing.queued,
                pathing.groups,
                pathing.searched,
                pathing.direct,
                pathing.partial,
                pathing.lost,
                pathing.settled,
            ),
            false => String::new(),
        };
        format!("Space Crusaders\n{:?}  ·  {:?}  ·  Health {}/{}  ·  {device}\nSquad {following} following · {marching} marching · {holding} holding · Nuclonium {stored}\nWASD move · mouse look · Space jump · V jetpack/skate\nShift attack · X squad (hold to whistle, tap to send)\nB build stellarator (hold to grow, tap for the smallest)\nG plant pylon (hold to place, beams link on sight)\nF/right mouse aim · Y weapon ({weapon}) · ` console · F2 switch · Esc menu · F11 window{paths}", state.active, ctrl.motion, health.current, health.max)
    } else {
        String::new()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::AssetPlugin, ecs::schedule::Schedule, ecs::system::RunSystemOnce,
        world_serialization::WorldAsset,
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
            .init_asset::<nuclonium::GlowMaterial>()
            // The one buffer the lamps live in. Its store is part of the
            // renderer in a real build, and `n64::lamplight` writes it every
            // frame here as it does there.
            .init_asset::<bevy::render::storage::ShaderBuffer>()
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

    /// Luna is playable by the AI as well as by the player.
    ///
    /// The squad used to be Marios by construction: `spawn_ally` loaded
    /// Mario's glTF and stamped Mario's animation table on whatever came out
    /// of it. This asks the console for a field of Lunas instead and checks
    /// what actually stands there -- that they are Lunas rather than Marios,
    /// that they carry the health an AI Luna is worth rather than a Mario's,
    /// that they are ordinary squad members the whistle can pick up, and that
    /// the two kinds live in the field side by side.
    #[test]
    fn the_squad_can_be_filled_with_lunas_beside_the_marios() {
        let mut app = headless();
        app.update();
        {
            let mut tuning = app.world_mut().resource_mut::<console::GameTuning>();
            tuning.ally_count = 3.0;
            tuning.luna_count = 2.0;
        }
        // A few frames for `maintain_population` to reconcile both counts and
        // for the squad to be stepped with them in it.
        for _ in 0..8 {
            app.update();
        }
        let mut allies = app
            .world_mut()
            .query::<(&squad::Ally, &ActiveCharacter, &health::Health)>();
        let standing: Vec<_> = allies
            .iter(app.world())
            .map(|(_, character, health)| (*character, health.max))
            .collect();
        let lunas = standing
            .iter()
            .filter(|(character, _)| *character == ActiveCharacter::Luna)
            .count();
        let marios = standing
            .iter()
            .filter(|(character, _)| *character == ActiveCharacter::Mario)
            .count();
        assert_eq!((lunas, marios), (2, 3), "the field holds {standing:?}");
        // Each kind is worth what its character is worth, which is the whole
        // reason for asking for one rather than the other.
        for (character, max) in &standing {
            assert_eq!(*max, character.ally_health(), "{character:?} has {max} hp");
        }
        // Asking for fewer takes the right ones away and leaves the rest.
        app.world_mut()
            .resource_mut::<console::GameTuning>()
            .luna_count = 0.0;
        for _ in 0..4 {
            app.update();
        }
        let mut allies = app.world_mut().query::<(&squad::Ally, &ActiveCharacter)>();
        let left: Vec<_> = allies
            .iter(app.world())
            .map(|(_, character)| *character)
            .collect();
        assert_eq!(left.len(), 3, "{left:?}");
        assert!(
            left.iter().all(|kind| *kind == ActiveCharacter::Mario),
            "clearing the Lunas took a Mario with them: {left:?}"
        );
    }

    /// The build button, end to end, through the game's own schedules.
    ///
    /// The unit tests in [`stellarator`] prove the arithmetic; this proves the
    /// *wiring* -- that the button reaches the system, that the system finds
    /// the camera and the player it aims between, that a machine ends up in the
    /// world with an empty store on it, and that the second one is refused for
    /// standing on the first. None of that is reachable from a test that calls
    /// `stellarator::fits` directly, and all of it is a game that opens and
    /// shuts if it is wrong.
    #[test]
    fn a_held_button_builds_a_stellarator_where_it_was_aimed() {
        let mut app = headless();
        app.update();
        // Long enough that a hold is being read rather than only a release --
        // which used to grow the machine and now only opens the ring.
        for _ in 0..40 {
            app.world_mut().resource_mut::<input::InputState>().build = true;
            app.update();
        }
        {
            let mut input = app.world_mut().resource_mut::<input::InputState>();
            input.build = false;
            input.build_released = true;
        }
        app.update();

        let aim = app.world().resource::<stellarator::Build>().aim;
        let mut machines = app
            .world_mut()
            .query::<(&stellarator::Stellarator, &Transform)>();
        let built: Vec<_> = machines
            .iter(app.world())
            .map(|(machine, at)| (machine.radius, at.translation))
            .collect();
        assert_eq!(built.len(), 1, "the button built {} machines", built.len());
        let (radius, at) = built[0];
        assert_eq!(at, aim, "the machine is not where the crosshair was");
        assert_eq!(
            radius,
            stellarator::footprint(stellarator::BUILD_SCALE),
            "the hold sized the machine; it is only supposed to aim it"
        );
        // It arrived empty, and nothing is turning inside it. A machine's
        // field is its stock -- see [`stellarator::stock`] -- so a brand new
        // one being dark is the thing to assert, not a bug to work around.
        let mut stores = app.world_mut().query::<&stellarator::Store>();
        let held: Vec<u32> = stores.iter(app.world()).map(|store| store.held).collect();
        assert_eq!(held, vec![0], "a machine was built holding something");
        let mut motes = app.world_mut().query::<&stellarator::Orbit>();
        assert_eq!(
            motes.iter(app.world()).count(),
            0,
            "an empty machine has a field"
        );

        // And the same site a second time is refused, because there is a
        // machine standing on it. Nothing has moved the player or the camera,
        // so the aim resolves to the same spot.
        for _ in 0..40 {
            app.world_mut().resource_mut::<input::InputState>().build = true;
            app.update();
        }
        {
            let mut input = app.world_mut().resource_mut::<input::InputState>();
            input.build = false;
            input.build_released = true;
        }
        app.update();
        assert!(
            !app.world().resource::<stellarator::Build>().fits,
            "the site under the first machine reads as clear"
        );
        let mut machines = app.world_mut().query::<&stellarator::Stellarator>();
        assert_eq!(
            machines.iter(app.world()).count(),
            1,
            "a second machine was built through the first"
        );
    }

    /// The plant key, end to end, and the network it builds.
    ///
    /// The unit tests in [`pylon`] prove the graph; this proves the *wiring* --
    /// that the key reaches the system, that a mast ends up standing where the
    /// crosshair was, that a second mast in reach of it is linked to it with a
    /// beam that gets drawn, and that a machine put down beside them lights the
    /// pair up. None of that is reachable from a test that calls
    /// `pylon::links` directly, and all of it is a game that opens and shuts if
    /// it is wrong.
    #[test]
    fn planted_masts_wire_themselves_to_the_machine_that_feeds_them() {
        let mut app = headless();
        app.update();
        // Long enough to have opened a site, so the hold is being read rather
        // than only the release.
        for _ in 0..20 {
            app.world_mut().resource_mut::<input::InputState>().pylon = true;
            app.update();
        }
        {
            let mut input = app.world_mut().resource_mut::<input::InputState>();
            input.pylon = false;
            input.pylon_released = true;
        }
        app.update();

        let aim = app.world().resource::<pylon::Plant>().aim;
        let mut masts = app.world_mut().query::<(&pylon::Pylon, &Transform)>();
        let planted: Vec<_> = masts
            .iter(app.world())
            .map(|(_, at)| at.translation)
            .collect();
        assert_eq!(planted.len(), 1, "the key planted {} masts", planted.len());
        assert_eq!(planted[0], aim, "the mast is not where the crosshair was");
        // Nothing is making power yet, so it stands dark -- which is a network
        // of one node and no beams.
        let network = app.world().resource::<pylon::Network>();
        assert_eq!(network.nodes.len(), 1);
        assert!(!network.powered(0), "a mast with no machine has power");

        // A second mast a few metres along, put down by hand rather than by
        // aiming again: the crosshair has not moved and the first mast is
        // standing where it points.
        let beside = aim + Vec3::new(8.0, 0.0, 0.0);
        app.world_mut()
            .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                pylon::spawn(&mut commands, &assets, beside, 0.0);
            })
            .unwrap();
        app.update();
        let network = app.world().resource::<pylon::Network>();
        assert_eq!(network.nodes.len(), 2, "the second mast never joined");
        assert_eq!(network.links.len(), 1, "no beam between two masts in reach");
        assert!(
            !network.powered(0) && !network.powered(1),
            "a linked pair lit itself with no machine anywhere"
        );
        // The beam is a thing in the world rather than a pair of indices.
        let mut beams = app.world_mut().query::<&pylon::Beam>();
        assert_eq!(beams.iter(app.world()).count(), 1);

        // And a machine beside them lights the pair up, one hop apart.
        let feeding = aim - Vec3::new(8.0, 0.0, 0.0);
        app.world_mut()
            .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                stellarator::spawn(&mut commands, &assets, feeding, 0.0, 0.5);
            })
            .unwrap();
        app.update();
        let network = app.world().resource::<pylon::Network>();
        let hops: Vec<_> = network.nodes.iter().map(|node| node.hops).collect();
        assert_eq!(network.live(), 2, "the machine lit {hops:?}");
        // The nearer mast is fed straight off the machine and the further one
        // through it, which is the flood doing its job rather than everything
        // happening to be in range of everything.
        assert!(hops.contains(&Some(0)), "{hops:?}");
        // The supply packet has somewhere to go now.
        assert!(network.run.len() >= 2, "no supply run over a live pair");
    }

    /// The whole resource chain, end to end.
    ///
    /// The unit tests in [`nuclonium`] prove the arithmetic -- the drop rate, the
    /// claim rules, the walk down a route -- and [`pylon`]'s prove the graph the
    /// route is read off. What none of them can reach is the *wiring*: that an
    /// idle Mario is actually handed an errand, that it walks far enough to pick
    /// a ball up, that a mast recognises the hand-over, and that the shipment
    /// makes it to a machine and is counted. Every one of those is a system
    /// boundary, and a chain of five is exactly the kind of thing that compiles
    /// and does nothing.
    #[test]
    fn a_ball_is_fetched_by_a_mario_and_shipped_down_the_beams_to_a_machine() {
        let mut app = headless();
        app.update();
        // A machine, a mast inside its reach, a Mario standing by the mast, and
        // a ball on the ground a few metres away. Placed relative to wherever
        // the player is standing on the castle, so this test does not carry a
        // second copy of where the level's ground happens to be.
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        let mast_at = here + Vec3::new(6.0, 0.0, 0.0);
        // Out of Luna's reach and out of the mast's, so the only thing that can
        // collect it is the Mario -- see `nuclonium::MAGNET_RANGE` and
        // `nuclonium::MAST_REACH`, both of which would otherwise do this test's
        // job for it and prove nothing about the squad.
        let ball_at = here + Vec3::new(0.0, 0.0, 10.0);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      art: Res<nuclonium::Art>| {
                    stellarator::spawn(&mut commands, &assets, here, 0.0, 0.5);
                    pylon::spawn(&mut commands, &assets, mast_at, 0.0);
                    squad::spawn_ally(
                        &mut commands,
                        &assets,
                        ActiveCharacter::Mario,
                        here + Vec3::new(1.0, 0.0, 1.0),
                        0.0,
                    );
                    nuclonium::spawn(
                        &mut commands,
                        &art,
                        nuclonium::Kind::Nuclonium,
                        ball_at,
                        0.0,
                    );
                },
            )
            .unwrap();
        app.update();
        assert!(
            app.world().resource::<pylon::Network>().powered(0),
            "the mast beside the machine never lit"
        );

        // Long enough for a Mario to walk a few metres twice over, at the
        // squad's marching pace, and for the shipment to fly a short beam.
        for _ in 0..400 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<nuclonium::Bank>().stored,
            1,
            "the ball never reached the machine"
        );
        let mut loose = app.world_mut().query::<&nuclonium::Nuclonium>();
        assert_eq!(
            loose.iter(app.world()).count(),
            0,
            "the ball was delivered and is still lying about as well"
        );
        let mut flying = app.world_mut().query::<&nuclonium::Shipment>();
        assert_eq!(
            flying.iter(app.world()).count(),
            0,
            "the shipment arrived and was not cleared away"
        );
    }

    /// A mast picks up what is lying at its foot, with nobody sent for it.
    ///
    /// The rule that makes a network worth building next to where the fighting
    /// happens rather than only worth building: what falls inside it is
    /// collected. No ally is involved -- the squad the castle spawns is left
    /// standing where it is, and the machine is far enough away that its own
    /// reach is not what did this.
    #[test]
    fn a_ball_under_a_live_mast_is_taken_up_by_the_mast() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        let mast_at = here + Vec3::new(12.0, 0.0, 0.0);
        // At the mast's foot, and a long way from Luna, so the only thing that
        // can have collected it is the mast.
        let ball_at = mast_at + Vec3::new(2.0, 0.0, 0.0);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      art: Res<nuclonium::Art>| {
                    stellarator::spawn(&mut commands, &assets, here, 0.0, 0.5);
                    pylon::spawn(&mut commands, &assets, mast_at, 0.0);
                    nuclonium::spawn(
                        &mut commands,
                        &art,
                        nuclonium::Kind::Nuclonium,
                        ball_at,
                        0.0,
                    );
                },
            )
            .unwrap();
        for _ in 0..200 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<nuclonium::Bank>().stored,
            1,
            "a ball lying under a live mast was never taken up"
        );
    }

    /// A ball lying in a field lights the field.
    ///
    /// **The illumination half of "the orbs need to be emissive".** The HDR
    /// glow card and the bloom pass are the half that makes an orb *look* like
    /// a light; this is the half that puts its colour on the grass. It ends up
    /// in one storage buffer that every material in the world binds, and the
    /// vertex stage adds it to `ambient + key * cos` -- see [`n64::LAMPS`].
    ///
    /// So what is asserted is the whole chain rather than any link of it: a
    /// ball spawned by the game's own spawn, through the bundle it shares with
    /// the motes and the shipments, through the pick, into the exact bytes the
    /// shader is handed. A test that stopped at `n64::nearest` would still
    /// pass with the buffer never written, written somewhere else, or written
    /// in a layout the shader does not read.
    ///
    /// Two things are said out loud here that the renderer would otherwise
    /// say: where the ball is in the world, and that it can be seen. Both are
    /// worked out in `PostUpdate` by systems this app has no renderer to
    /// bring, so without them everything in it stands at the origin and reads
    /// as hidden. Written into the test rather than taken out of
    /// [`n64::lamplight`], which is where they belong.
    #[test]
    fn a_ball_lying_in_a_field_lights_the_field() {
        use bevy::render::storage::ShaderBuffer;

        let mut app = headless();
        app.update();
        let lamps = |app: &App| {
            n64::Lamplight::read(
                app.world()
                    .resource::<Assets<ShaderBuffer>>()
                    .get(&n64::LAMPLIGHT)
                    .expect("nothing ever put a lamp buffer in the world"),
            )
        };
        // It is written even with nothing glowing, because a material whose
        // binding names a missing asset has no bind group and draws nothing at
        // all.
        assert!(
            lamps(&app).lit().is_empty(),
            "an empty world was already lamplit"
        );

        // A couple of paces from the camera, which is at the origin here for
        // the reason above.
        let spot = Vec3::new(1.5, 0.0, 0.0);
        app.world_mut()
            .run_system_once(move |mut commands: Commands, art: Res<nuclonium::Art>| {
                let ball =
                    nuclonium::spawn(&mut commands, &art, nuclonium::Kind::Nuclonium, spot, 0.0);
                commands.entity(ball).insert((
                    GlobalTransform::from_translation(spot),
                    InheritedVisibility::VISIBLE,
                ));
            })
            .unwrap();
        app.update();

        let lit = lamps(&app).lit();
        assert_eq!(lit.len(), 1, "the ball did not reach the shader's buffer");
        let lamp = lit[0];
        assert!(
            lamp.at.truncate().distance(spot) < 1e-3,
            "the lamp was not where the ball is: {} against {spot}",
            lamp.at.truncate(),
        );
        assert!(lamp.at.w > 0.0, "the lamp reached nowhere");
        // Green, because that is what nuclonium is. The colour is what makes
        // the grass under a ball green rather than merely brighter: the
        // console's combiner could only ever multiply a surface's own colour.
        assert!(
            lamp.glow.y > lamp.glow.x && lamp.glow.y > lamp.glow.z,
            "the lamp was not the ball's own colour: {}",
            lamp.glow,
        );
    }

    /// A ball is snatched up off the ground rather than appearing in a hand.
    ///
    /// The other half of "balls should not abruptly change location". Being
    /// picked up used to be a write: the tick a Mario got within reach, the
    /// ball was assigned to a point a metre and a half over its head, having
    /// crossed the gap in no time at all. Now `nuclonium::haul` decides it is
    /// held and `nuclonium::swim` flies it into the Mario's hands over the next
    /// few frames, on the same easing the train uses.
    #[test]
    fn a_ball_is_snatched_off_the_ground_rather_than_appearing_in_a_hand() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // Out of Luna's magnet, with one Mario of our own beside it.
        let ball_at = here + Vec3::new(0.0, 0.0, 10.0);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      art: Res<nuclonium::Art>| {
                    squad::spawn_ally(
                        &mut commands,
                        &assets,
                        ActiveCharacter::Mario,
                        ball_at + Vec3::new(1.0, 0.0, 0.0),
                        0.0,
                    );
                    nuclonium::spawn(
                        &mut commands,
                        &art,
                        nuclonium::Kind::Nuclonium,
                        ball_at,
                        0.0,
                    );
                },
            )
            .unwrap();
        let ball = loop {
            app.update();
            let mut balls = app
                .world_mut()
                .query_filtered::<Entity, With<nuclonium::Nuclonium>>();
            if let Some(ball) = balls.iter(app.world()).next() {
                break ball;
            }
        };
        // The frame it changes hands, and where it is at that moment.
        let mut carrier = None;
        for _ in 0..400 {
            app.update();
            if let nuclonium::Held::Carried(mario) =
                app.world().get::<nuclonium::Nuclonium>(ball).unwrap().held
            {
                carrier = Some(mario);
                break;
            }
        }
        let carrier = carrier.expect("nobody ever picked the ball up");
        let hands = |app: &App| {
            app.world().get::<Transform>(carrier).unwrap().translation
                + Vec3::Y * nuclonium::CARRY_HEIGHT
        };
        let grabbed = app.world().get::<Transform>(ball).unwrap().translation;
        assert!(
            grabbed.distance(hands(&app)) > 0.5,
            "the ball was in the Mario's hands on the tick it was claimed, having
             crossed {} m in no time",
            grabbed.distance(hands(&app))
        );
        // And it gets there, promptly, which is the other half of easing rather
        // than teleporting: a pull that never arrives is a ball being dragged.
        for _ in 0..30 {
            app.update();
        }
        let carried = app.world().get::<Transform>(ball).unwrap().translation;
        assert!(
            carried.distance(hands(&app)) < 0.35,
            "the ball never caught its carrier up: {} m behind",
            carried.distance(hands(&app))
        );
    }

    /// A ball handed in at a mast flies to the mast rather than from it.
    ///
    /// **Nothing made of nuclonium may change place without travelling.** The
    /// route the network hands back starts at the mast's own head, several
    /// metres up and up to `MAST_REACH` away from the ball it just took -- so
    /// the ball used to vanish from the grass and reappear at the top of the
    /// tower on the following tick, which is the "abruptly appears in the pylon
    /// network" this is against. Now the flight starts where the ball was
    /// lying and climbs into the beams. See `nuclonium::deliver`.
    #[test]
    fn a_ball_taken_up_by_a_mast_flies_from_where_it_was_lying() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        let mast_at = here + Vec3::new(12.0, 0.0, 0.0);
        // Out at the edge of the mast's reach, so the gap between where the
        // ball is and where the network starts is the whole point.
        let ball_at = mast_at + Vec3::new(nuclonium::MAST_REACH - 0.5, 0.0, 0.0);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      art: Res<nuclonium::Art>| {
                    stellarator::spawn(&mut commands, &assets, here, 0.0, 0.5);
                    pylon::spawn(&mut commands, &assets, mast_at, 0.0);
                    nuclonium::spawn(
                        &mut commands,
                        &art,
                        nuclonium::Kind::Nuclonium,
                        ball_at,
                        0.0,
                    );
                },
            )
            .unwrap();
        // As soon as there is something in the air, look at where it is.
        let mut started: Option<Vec3> = None;
        for _ in 0..200 {
            app.update();
            let mut flying = app
                .world_mut()
                .query_filtered::<&Transform, With<nuclonium::Shipment>>();
            if let Ok(at) = flying.single(app.world()) {
                started = Some(at.translation);
                break;
            }
        }
        let started = started.expect("the mast never sent anything home");
        // A frame is a tick or three of flight, so the first sight of it is
        // still near the grass it was picked up off -- nearer to that, at any
        // rate, than to the head of the mast it is on its way to. Before the
        // prepended leg it started *at* the mast head, five and a half metres
        // away and several metres up, on the frame it appeared.
        let mast_top = {
            let network = app.world().resource::<pylon::Network>();
            network
                .nodes
                .iter()
                .map(|node| node.top)
                .min_by(|a, b| a.distance(mast_at).total_cmp(&b.distance(mast_at)))
                .expect("no mast was planted")
        };
        assert!(
            started.distance(ball_at) < started.distance(mast_top),
            "the ball appeared {} m from where it was lying and {} m from the mast head",
            started.distance(ball_at),
            started.distance(mast_top)
        );
    }

    /// A ball Luna walks past joins her, and comes off her at a mast.
    ///
    /// The whole of "near Luna should follow Luna until near a pylon", end to
    /// end, and it is two rules rather than one: the magnet that recruits it
    /// and the mast that takes it. Luna is teleported rather than driven --
    /// this is about the balls, and steering her with a fake input snapshot
    /// would be a test of the input snapshot.
    #[test]
    fn a_ball_luna_walks_over_follows_her_and_comes_off_at_a_mast() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // The squad would fetch this ball long before Luna reached it, and
        // this test is about Luna. Sent home, and the tuning with them, or
        // `squad::maintain_population` puts them straight back.
        {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            let marios: Vec<Entity> = allies.iter(app.world()).collect();
            for mario in marios {
                app.world_mut().entity_mut(mario).despawn();
            }
            app.world_mut()
                .resource_mut::<console::GameTuning>()
                .ally_count = 0.0;
        }
        let mast_at = here + Vec3::new(14.0, 0.0, 0.0);
        // Out of the mast's reach, out of Luna's, and out of the machine's.
        let ball_at = here + Vec3::new(0.0, 0.0, 12.0);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      art: Res<nuclonium::Art>| {
                    stellarator::spawn(&mut commands, &assets, here, 0.0, 0.5);
                    pylon::spawn(&mut commands, &assets, mast_at, 0.0);
                    nuclonium::spawn(
                        &mut commands,
                        &art,
                        nuclonium::Kind::Nuclonium,
                        ball_at,
                        0.0,
                    );
                },
            )
            .unwrap();
        app.update();

        // Walk Luna onto it.
        let put = |app: &mut App, at: Vec3| {
            let mut player = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Player>>();
            player.single_mut(app.world_mut()).unwrap().translation = at;
        };
        put(&mut app, ball_at);
        for _ in 0..10 {
            app.update();
        }
        {
            let mut balls = app.world_mut().query::<&nuclonium::Nuclonium>();
            let following = balls
                .iter(app.world())
                .filter(|ball| matches!(ball.held, nuclonium::Held::Following(_)))
                .count();
            assert_eq!(following, 1, "the ball Luna stood on did not join her");
        }
        assert_eq!(
            app.world().resource::<nuclonium::Bank>().stored,
            0,
            "it was banked without ever reaching a mast"
        );

        // And now carry it to the mast.
        put(&mut app, mast_at + Vec3::new(1.0, 0.0, 0.0));
        for _ in 0..120 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<nuclonium::Bank>().stored,
            1,
            "the ball following Luna was never handed in at the mast"
        );
    }

    /// Luna's train glides on the frames the simulation skips.
    ///
    /// **This is the stutter, written as a test.** The fixed step runs thirty
    /// times a second and the frames come faster than that, so most frames are
    /// drawn *between* two ticks -- which is exactly what
    /// `player::sync_visual` interpolates Luna across, and why she glides. A
    /// train solved on the fixed step is therefore a train that stands still on
    /// every frame the simulation skipped and jumps on the ones it did not,
    /// behind a leader who did neither. Nothing about the spring was wrong; it
    /// was being solved at the wrong rate.
    ///
    /// So this looks for a frame the fixed step did not tick on, and asks
    /// whether the ball moved on it anyway. See `nuclonium::swim`.
    #[test]
    fn a_ball_in_lunas_train_glides_on_the_frames_between_two_ticks() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // The squad would pick this up before her magnet reached it, and this
        // test is about her train. Sent home, and the tuning with them, or
        // `squad::maintain_population` puts them straight back.
        {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            let marios: Vec<Entity> = allies.iter(app.world()).collect();
            for mario in marios {
                app.world_mut().entity_mut(mario).despawn();
            }
            app.world_mut()
                .resource_mut::<console::GameTuning>()
                .ally_count = 0.0;
        }
        // Dropped at her feet, so her magnet has it within a tick or two.
        app.world_mut()
            .run_system_once(move |mut commands: Commands, art: Res<nuclonium::Art>| {
                nuclonium::spawn(&mut commands, &art, nuclonium::Kind::Nuclonium, here, 0.0);
            })
            .unwrap();
        for _ in 0..6 {
            app.update();
        }
        let ball = {
            let mut balls = app
                .world_mut()
                .query_filtered::<Entity, With<nuclonium::Nuclonium>>();
            balls.single(app.world()).unwrap()
        };
        assert!(
            matches!(
                app.world().get::<nuclonium::Nuclonium>(ball).unwrap().held,
                nuclonium::Held::Following(_)
            ),
            "the ball Luna is standing on never joined her"
        );
        // Somewhere to swim to.
        {
            let mut player = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Player>>();
            player.single_mut(app.world_mut()).unwrap().translation =
                here + Vec3::new(4.0, 0.0, 0.0);
        }
        let ticks = |app: &App| app.world().resource::<Time<Fixed>>().elapsed();
        let mut glided = false;
        for _ in 0..40 {
            let (before, was) = (
                app.world().get::<Transform>(ball).unwrap().translation,
                ticks(&app),
            );
            app.update();
            let after = app.world().get::<Transform>(ball).unwrap().translation;
            // A frame with no tick in it: whatever moved the ball here was not
            // the simulation.
            if ticks(&app) == was && after.distance(before) > 1e-4 {
                glided = true;
                break;
            }
        }
        assert!(
            glided,
            "the train only ever moved on ticks, which is the judder"
        );
    }

    /// And so does a Mario, which is the same defect one leader further out.
    ///
    /// The squad walks on the fixed step like everything else, and until now it
    /// was *drawn* on the fixed step too: the same pose held for two or three
    /// frames and then a whole tick's stride at once, beside a leader gliding
    /// smoothly past it. What could not be done about it is the interesting
    /// part -- a Mario's `Transform` is where the Mario is, read by the fight,
    /// the planner and the walk, so it cannot simply be smoothed in place
    /// without feeding a drawn half-step back into the next tick's arithmetic.
    ///
    /// So the pose is banked and put back. This asks the question from the
    /// outside: on a frame the simulation did not tick, did a Mario move
    /// anyway? See `squad::Glide`.
    #[test]
    fn a_mario_glides_on_the_frames_between_two_ticks() {
        let mut app = headless();
        // Long enough for the field to be standing and for some of it to have
        // grown bored and started ambling.
        for _ in 0..30 {
            app.update();
        }
        let marios = |app: &mut App| -> Vec<(Entity, Vec3)> {
            let mut allies = app
                .world_mut()
                .query_filtered::<(Entity, &Transform), With<squad::Ally>>();
            allies
                .iter(app.world())
                .map(|(entity, at)| (entity, at.translation))
                .collect()
        };
        assert!(
            !marios(&mut app).is_empty(),
            "the field has no Marios in it"
        );
        let ticks = |app: &App| app.world().resource::<Time<Fixed>>().elapsed();
        let mut glided = false;
        for _ in 0..240 {
            let (before, was) = (marios(&mut app), ticks(&app));
            app.update();
            if ticks(&app) != was {
                // The simulation ran: whatever moved is allowed to have moved.
                continue;
            }
            let after = marios(&mut app);
            glided = before
                .iter()
                .zip(after.iter())
                .any(|(before, after)| before.0 == after.0 && after.1.distance(before.1) > 1e-5);
            if glided {
                break;
            }
        }
        assert!(
            glided,
            "the squad only ever moved on ticks, which is the stutter"
        );
    }

    /// A red ball comes to you rather than being had at arm's length.
    ///
    /// Two halves that fail differently. One is a medkit that teleports into
    /// your health bar from two metres away, which is what this replaced: no
    /// moment, nothing on the screen joining the ball to the number going up.
    /// The other is a medkit that notices and then never arrives.
    #[test]
    fn a_medkit_comes_to_whoever_needs_it_rather_than_being_had_at_arms_length() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // Inside the lure and well outside the touch.
        let kit_at = here + Vec3::new(3.0, 0.0, 0.0);
        app.world_mut()
            .run_system_once(move |mut commands: Commands, art: Res<nuclonium::Art>| {
                nuclonium::spawn(&mut commands, &art, nuclonium::Kind::Medkit, kit_at, 0.0);
            })
            .unwrap();
        {
            let mut player = app
                .world_mut()
                .query_filtered::<&mut health::Health, With<Player>>();
            player.single_mut(app.world_mut()).unwrap().current = 10;
        }
        let where_is_it = |app: &mut App| {
            let mut kits = app
                .world_mut()
                .query_filtered::<&Transform, With<health::Medkit>>();
            kits.single(app.world()).map(|at| at.translation).ok()
        };
        let started = where_is_it(&mut app).expect("the medkit was never spawned");
        for _ in 0..4 {
            app.update();
        }
        let moved = where_is_it(&mut app).expect("the medkit was taken at three metres");
        assert!(
            moved.distance(here) < started.distance(here) - 0.05,
            "the medkit noticed a hurt Luna and did not come towards her"
        );
        // And then it arrives and is absorbed.
        for _ in 0..60 {
            app.update();
        }
        assert!(
            where_is_it(&mut app).is_none(),
            "the medkit drifted in and then never landed"
        );
        let mut player = app
            .world_mut()
            .query_filtered::<&health::Health, With<Player>>();
        assert_eq!(
            player.single(app.world()).unwrap().current,
            10 + health::MEDKIT_HEAL,
            "it was absorbed and put nothing back"
        );
    }

    /// Holding the grab button opens a circle, growing it, and letting go shuts
    /// it again.
    ///
    /// Driven through `InputState` rather than through the keyboard, because
    /// `input_pipeline` is wired up in `main` and does not run in a headless
    /// app -- which is also why this is not a test of the picker. That X
    /// reaches this flag at all is `action::aim`'s, and has its own test there.
    #[test]
    fn holding_the_grab_button_opens_a_circle_that_grows() {
        let mut app = headless();
        app.update();
        assert!(
            app.world().resource::<nuclonium::Grab>().held_for.is_none(),
            "a circle was open before anything was pressed"
        );
        app.world_mut().resource_mut::<input::InputState>().grab = true;
        app.update();
        let opened = app.world().resource::<nuclonium::Grab>().radius;
        assert!(
            app.world().resource::<nuclonium::Grab>().held_for.is_some(),
            "holding the grab button opened no circle"
        );
        assert!(
            opened >= nuclonium::grab_radius(0.0),
            "the circle opened at {opened}, inside its own smallest size"
        );
        for _ in 0..30 {
            app.update();
        }
        let grown = app.world().resource::<nuclonium::Grab>().radius;
        assert!(
            grown > opened,
            "holding it longer did not grow the circle: {opened} then {grown}"
        );
        {
            let mut input = app.world_mut().resource_mut::<input::InputState>();
            input.grab = false;
            input.grab_released = true;
        }
        app.update();
        assert!(
            app.world().resource::<nuclonium::Grab>().held_for.is_none(),
            "letting go left the circle open"
        );
    }

    /// A red ball is hit points, and only for somebody who needs them.
    ///
    /// Both halves matter and they fail differently: one is a pickup that does
    /// nothing, the other is a pickup that is thrown away. See
    /// [`health::mend`].
    #[test]
    fn a_medkit_mends_whoever_needs_it_and_waits_for_somebody_who_does() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        app.world_mut()
            .run_system_once(move |mut commands: Commands, art: Res<nuclonium::Art>| {
                nuclonium::spawn(&mut commands, &art, nuclonium::Kind::Medkit, here, 0.0);
            })
            .unwrap();
        for _ in 0..10 {
            app.update();
        }
        // Nobody is hurt, so it is still lying there.
        {
            let mut kits = app.world_mut().query::<&health::Medkit>();
            assert_eq!(
                kits.iter(app.world()).count(),
                1,
                "a medkit was spent on somebody at full health"
            );
        }

        // Now hurt Luna, standing on it.
        {
            let mut player = app
                .world_mut()
                .query_filtered::<&mut health::Health, With<Player>>();
            player.single_mut(app.world_mut()).unwrap().current = 10;
        }
        for _ in 0..10 {
            app.update();
        }
        let mut kits = app.world_mut().query::<&health::Medkit>();
        assert_eq!(
            kits.iter(app.world()).count(),
            0,
            "a hurt Luna standing on a medkit did not pick it up"
        );
        let mut player = app
            .world_mut()
            .query_filtered::<&health::Health, With<Player>>();
        assert_eq!(
            player.single(app.world()).unwrap().current,
            10 + health::MEDKIT_HEAL,
            "the medkit was taken and put nothing back"
        );
    }

    /// What a *kill* leaves behind is collectable, which is not the same test.
    ///
    /// The one above hands the squad a ball placed on the ground, and it has
    /// passed all along while the squad visibly ignored what the fighting
    /// dropped. The difference is where the ball ends up: a kill is resolved at
    /// the dying thing's own origin, or at the point a round landed on it, and
    /// nothing in this game falls -- so a ball shed by a body was left hanging
    /// at the height it died at. A Mario walked underneath it, arrived, and
    /// could not reach it, because the reach was a straight three-dimensional
    /// distance and the walk had already spent three quarters of it getting
    /// there.
    ///
    /// So this drops one the way a kill does, a metre and a bit off the floor,
    /// and asks for it back. Both halves of the fix are needed for it to pass
    /// and either alone would do it, which is why it is written against the
    /// outcome rather than against a height.
    #[test]
    fn a_ball_shed_by_a_kill_off_the_ground_is_still_collected() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        let mast_at = here + Vec3::new(6.0, 0.0, 0.0);
        // Where a slime's middle is, rather than where its feet were.
        let died_at = here + Vec3::new(2.0, 1.2, 3.0);
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      mut drops: ResMut<nuclonium::Drops>| {
                    stellarator::spawn(&mut commands, &assets, here, 0.0, 0.5);
                    pylon::spawn(&mut commands, &assets, mast_at, 0.0);
                    squad::spawn_ally(
                        &mut commands,
                        &assets,
                        ActiveCharacter::Mario,
                        here + Vec3::new(1.0, 0.0, 1.0),
                        0.0,
                    );
                    // Through the one die in the game rather than round it, so
                    // this is the path a real kill takes. One roll in twenty
                    // lands; twenty rolls is one ball.
                    for _ in 0..20 {
                        if drops.maybe(died_at) == Some(nuclonium::Kind::Nuclonium) {
                            break;
                        }
                    }
                },
            )
            .unwrap();
        for _ in 0..400 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<nuclonium::Bank>().stored,
            1,
            "the ball a kill dropped was never collected"
        );
    }

    /// A mast is something that can be lost.
    ///
    /// The point of the whole [`structure`] module: an ant that has noticed a
    /// pylon stands on it and wears it down, and the network it was part of
    /// rebuilds around the hole. What this proves past the unit tests is that a
    /// pylon is a *target* at all -- that `enemy::alert` picks a building out of
    /// the field on the strength of its `Side` alone, with no targeting code
    /// written for it.
    #[test]
    fn a_crowd_takes_a_planted_mast_down_and_the_network_closes_over_it() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // Well away from the player, so what the ants notice is the mast rather
        // than him -- `enemy_sight` is 14 m and this is comfortably past it.
        let mast_at = here + Vec3::new(40.0, 0.0, 0.0);
        app.world_mut()
            .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                let mast = pylon::spawn(&mut commands, &assets, mast_at, 0.0);
                let _ = mast;
                for step in 0..4 {
                    enemy::spawn(
                        &mut commands,
                        &assets,
                        enemy::Kind::Ant,
                        mast_at + Vec3::new(2.0 + step as f32 * 0.4, 0.0, 1.0),
                        step as f32,
                    );
                }
            })
            .unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<pylon::Network>().nodes.len(),
            1,
            "the mast never joined the network"
        );

        // A hundred and twenty points at three a blow, one blow every third of a
        // second however many ants turned up: fourteen seconds if the siege
        // never lets up, and longer than that in practice, because `enemy::spread`
        // and the weave keep shoving individual ants in and out of arm's reach.
        // The budget is generous for that reason -- what is being proved here is
        // that a mast *can* be lost, not the rate.
        let mut standing = true;
        for _ in 0..1600 {
            app.update();
            let mut masts = app.world_mut().query::<&pylon::Pylon>();
            if masts.iter(app.world()).next().is_none() {
                standing = false;
                break;
            }
        }
        assert!(
            !standing,
            "four ants stood on a mast and never took it down: {} points left",
            app.world_mut()
                .query_filtered::<&health::Health, With<pylon::Pylon>>()
                .iter(app.world())
                .next()
                .map_or(-1, |health| health.current)
        );
        // And the network noticed, rather than keeping a node for a mast that
        // is not there.
        app.update();
        assert_eq!(
            app.world().resource::<pylon::Network>().nodes.len(),
            0,
            "the network still holds a mast that has been knocked over"
        );
    }

    /// A nest is an objective.
    ///
    /// Two things at once, and both are wiring the unit tests cannot see. That
    /// a warp pipe placed by the level is standing there as something with hit
    /// points and a side -- so the sword finds it at all -- and that one held
    /// swing spends *one* blow against it rather than one a tick. The second is
    /// the whole reason `structure::demolish` takes a rising edge: against a
    /// creature the difference is invisible, because a swing one-shots
    /// everything the game places, and against a pipe it is six swings or one.
    #[test]
    fn a_warp_pipe_takes_a_swing_at_a_time_and_can_be_knocked_down() {
        let mut app = headless();
        app.update();
        // Whichever hostile pipe the castle put down, and where it is.
        let nest = {
            let mut pipes = app
                .world_mut()
                .query::<(Entity, &Transform, &pipe::WarpPipe, &enemy::Side)>();
            pipes
                .iter(app.world())
                .find(|(_, _, _, side)| **side == enemy::Side::Hostile)
                .map(|(entity, at, _, _)| (entity, at.translation))
        };
        let (nest, standing_at) = nest.expect("the castle has no enemy warp pipe on it");
        let full = app
            .world()
            .get::<health::Health>(nest)
            .expect("a pipe with no hit points")
            .max;
        assert_eq!(full, health::WARP_PIPE_HEALTH);

        // The squad off the field first. A Mario that notices a hostile pipe
        // walks over and punches it -- which is the feature working, and which
        // would make the arithmetic below about two attackers rather than one.
        {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            let marios: Vec<Entity> = allies.iter(app.world()).collect();
            for mario in marios {
                app.world_mut().entity_mut(mario).despawn();
            }
            app.world_mut()
                .resource_mut::<console::GameTuning>()
                .ally_count = 0.0;
        }
        // Stood against it, so every swing is in reach.
        {
            let mut player = app
                .world_mut()
                .query_filtered::<&mut Transform, With<Player>>();
            player.single_mut(app.world_mut()).unwrap().translation = standing_at;
        }
        // One swing, held for as long as the player actually holds one.
        let swing = |app: &mut App| {
            app.world_mut()
                .run_system_once(|mut player: Query<&mut Controller, With<Player>>| {
                    if let Ok(mut ctrl) = player.single_mut() {
                        ctrl.attack_left = 0.55;
                    }
                })
                .unwrap();
            // Long enough for the window to have opened and closed again, so
            // the next call is a fresh swing rather than the same one.
            for _ in 0..60 {
                app.update();
            }
        };
        swing(&mut app);
        assert_eq!(
            app.world().get::<health::Health>(nest).map(|it| it.current),
            Some(full - health::PLAYER_DAMAGE),
            "one held swing did not spend exactly one blow"
        );
        // And the rest of them finish it.
        for _ in 0..(full / health::PLAYER_DAMAGE) {
            if app.world().get_entity(nest).is_err() {
                break;
            }
            swing(&mut app);
        }
        assert!(
            app.world().get_entity(nest).is_err(),
            "the nest survived a pool's worth of swings"
        );
    }

    /// A squad tapped at something walks over and *fights* it.
    ///
    /// **The bug this is here to keep out is the one a player sees as "some of
    /// them just stop and do nothing".** A Mario standing on the spot it was
    /// sent to is at zero range from its order, which scores 0.90 for ever; a
    /// slime four metres away scores 0.64. So the squad marched over, spread
    /// out around the target, and stood in the middle of a fight -- every one
    /// of them doing exactly as it was told, and the only ones that ever swung
    /// were the ones whose slot in the cluster happened to land inside their own
    /// punch's reach. See [`goap::Goal::Hold`], which is what an order becomes
    /// once it has been carried out.
    ///
    /// Staged in the running game rather than against [`goap::choose`], because
    /// the unit test cannot see the thing that actually went wrong: the score
    /// was right, the *state fed to it* was a journey that had already finished.
    #[test]
    fn a_squad_sent_at_a_slime_closes_on_it_rather_than_standing_on_its_spot() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // The castle's own squad is somewhere else and would confuse the count.
        {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            let marios: Vec<Entity> = allies.iter(app.world()).collect();
            for mario in marios {
                app.world_mut().entity_mut(mario).despawn();
            }
            let mut tuning = app.world_mut().resource_mut::<console::GameTuning>();
            tuning.ally_count = 4.0;
            tuning.luna_count = 0.0;
        }
        // Four Marios in a huddle, and a slime standing five metres from where
        // they are about to be sent. Five is the number that matters: outside
        // `enemy::MARIO_REACH`, so nobody can hit it from the spot they are
        // told to stand on, and well inside `enemy_sight`, so all four can see
        // it the whole time.
        let squad_at = here + Vec3::new(-12.0, 0.0, -6.0);
        let target = Vec2::new(here.x - 12.0, here.z);
        let slime_at = Vec3::new(target.x, here.y, target.y + 5.0);
        let slime = app
            .world_mut()
            .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                for index in 0..4 {
                    let offset = squad::slot(index, 1.5);
                    squad::spawn_ally(
                        &mut commands,
                        &assets,
                        ActiveCharacter::Mario,
                        squad_at + Vec3::new(offset.x, 0.0, offset.y),
                        index as f32,
                    );
                }
                enemy::spawn(&mut commands, &assets, enemy::Kind::Slime, slime_at, 0.0)
            })
            .unwrap();
        app.update();

        // Whistled up and sent, which is the two halves of the button.
        let marios: Vec<Entity> = {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            allies.iter(app.world()).collect()
        };
        assert_eq!(marios.len(), 4, "the squad is not the squad this test sent");
        app.world_mut()
            .resource_scope(|world, mut squad: Mut<squad::Squad>| {
                squad.recruit(&marios);
                squad.send(
                    world.resource::<flow::FlowField>(),
                    Vec3::new(target.x, slime_at.y, target.y),
                );
            });

        // Long enough to walk twelve metres, arrive, notice, close and land a
        // few punches.
        let mut fought = false;
        for _ in 0..600 {
            app.update();
            let mut plans = app.world_mut().query::<&squad::Ally>();
            fought |= plans
                .iter(app.world())
                .any(|ally| matches!(ally.plan, goap::Goal::Fight { .. }));
            if app.world().get_entity(slime).is_err() {
                break;
            }
        }
        assert!(
            fought,
            "the whole squad stood on its spots with a slime five metres away"
        );
        if app.world().get_entity(slime).is_ok() {
            let at = app.world().get::<Transform>(slime).unwrap().translation;
            let hp = app.world().get::<health::Health>(slime).map(|h| h.current);
            let mut marios = app.world_mut().query::<(&squad::Ally, &Transform)>();
            let who: Vec<_> = marios
                .iter(app.world())
                .map(|(a, t)| (t.translation.distance(at), a.plan))
                .collect();
            panic!("nobody ever reached the slime they were sent at: alive at {at:?} hp {hp:?}; marios {who:#?}");
        }
    }

    /// A squad sent somewhere deals with what is standing in the way of getting
    /// there, rather than filing round it.
    ///
    /// **The complaint this is here for is "they walk straight past things".**
    /// It was true and the scoring said so: an order is worth 0.76 at eleven
    /// metres out and a fight is worth 0.54 at six, so a Mario with somewhere
    /// to be squeezed past the slime in the gateway and only turned on it once
    /// it had arrived and the order had retired into a post. Which is a squad
    /// that fights *at* its destination and never *on the way* to one.
    ///
    /// The fix is [`goap::detour`] -- a fight is scored by how far out of the
    /// Mario's way it is, and something on the line is no way out of it at all
    /// -- and the assertion that separates the two behaviours is the timing.
    /// The old squad fought this slime too. It fought it afterwards.
    ///
    /// Staged in the running game because what is being tested is the state the
    /// scoring is fed: that a Mario walking an order really does see the thing
    /// in front of it as sitting on its line.
    #[test]
    fn a_squad_marching_past_a_slime_stops_and_fights_it_on_the_way() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // The castle's own squad is somewhere else and would confuse the count.
        {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            let marios: Vec<Entity> = allies.iter(app.world()).collect();
            for mario in marios {
                app.world_mut().entity_mut(mario).despawn();
            }
            let mut tuning = app.world_mut().resource_mut::<console::GameTuning>();
            tuning.ally_count = 4.0;
            tuning.luna_count = 0.0;
        }
        // The same stretch of courtyard the test above walks, with the pieces
        // in the other order: the squad at one end, the spot they are sent to
        // at the other, and the slime half way between the two. Five metres
        // short of the spot, so nobody can reach it from where they are told
        // to stand -- if it dies before they arrive, somebody left the line
        // for it.
        let squad_at = here + Vec3::new(-12.0, 0.0, -6.0);
        let slime_at = Vec3::new(here.x - 12.0, here.y, here.z);
        let target = Vec2::new(here.x - 12.0, here.z + 5.0);
        let slime = app
            .world_mut()
            .run_system_once(move |mut commands: Commands, assets: Res<AssetServer>| {
                for index in 0..4 {
                    let offset = squad::slot(index, 1.5);
                    squad::spawn_ally(
                        &mut commands,
                        &assets,
                        ActiveCharacter::Mario,
                        squad_at + Vec3::new(offset.x, 0.0, offset.y),
                        index as f32,
                    );
                }
                enemy::spawn(&mut commands, &assets, enemy::Kind::Slime, slime_at, 0.0)
            })
            .unwrap();
        app.update();

        let marios: Vec<Entity> = {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            allies.iter(app.world()).collect()
        };
        assert_eq!(marios.len(), 4, "the squad is not the squad this test sent");
        app.world_mut()
            .resource_scope(|world, mut squad: Mut<squad::Squad>| {
                squad.recruit(&marios);
                squad.send(
                    world.resource::<flow::FlowField>(),
                    Vec3::new(target.x, slime_at.y, target.y),
                );
            });

        // Long enough to walk eleven metres and have the fight.
        let mut fought_on_the_way = false;
        let mut killed_on_the_way = false;
        for _ in 0..600 {
            app.update();
            // A Mario whose order is still outstanding, turning on the slime.
            // Both halves matter: the plan on its own is what the old squad
            // did once it had arrived and its order had become a post.
            let marching: Vec<Entity> = app
                .world()
                .resource::<squad::Squad>()
                .sent
                .iter()
                .filter(|order| !order.arrived)
                .map(|order| order.who)
                .collect();
            let turned = marching.iter().any(|mario| {
                app.world()
                    .get::<squad::Ally>(*mario)
                    .is_some_and(|ally| matches!(ally.plan, goap::Goal::Fight { .. }))
            });
            fought_on_the_way |= turned;
            if app.world().get_entity(slime).is_err() {
                killed_on_the_way = !marching.is_empty();
                break;
            }
        }
        assert!(
            fought_on_the_way,
            "the squad filed past a slime standing in the gateway"
        );
        assert!(
            app.world().get_entity(slime).is_err(),
            "the slime was noticed and never dealt with"
        );
        assert!(
            killed_on_the_way,
            "the slime only died once every Mario had already finished its order"
        );
    }

    /// A tap on the whistle sends the squad and leaves a ring where it landed.
    ///
    /// The end-to-end shape of the command, through the real button rather than
    /// by poking `Squad::send`: the release resolves an aim, the squad is sent
    /// to it, and `squad::order_ring` puts a visible mark on that exact spot.
    /// Nothing else in the game reports where an order went, so a ring that is
    /// drawn somewhere other than where the Marios were sent is a lie the
    /// player has no way to catch.
    #[test]
    fn a_tap_sends_the_squad_and_rings_where_the_order_landed() {
        let mut app = headless();
        // A few frames for `squad::maintain_population` to put the field out.
        for _ in 0..8 {
            app.update();
        }
        // Recruit whatever the field starts with, so there is a squad to send.
        let marios: Vec<Entity> = {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            allies.iter(app.world()).collect()
        };
        assert!(!marios.is_empty(), "no Marios to order about");
        app.world_mut()
            .resource_mut::<squad::Squad>()
            .recruit(&marios);

        // A tap: down for one frame, up on the next. Sixteen milliseconds is
        // well inside `TAP_SECONDS`, so this is an order and not a whistle.
        {
            let mut input = app.world_mut().resource_mut::<input::InputState>();
            input.squad = true;
        }
        app.update();
        {
            let mut input = app.world_mut().resource_mut::<input::InputState>();
            input.squad = false;
            input.squad_released = true;
        }
        app.update();

        let mark = app.world().resource::<squad::OrderMark>();
        let (at, radius) = (mark.at, mark.radius);
        assert!(mark.fade() > 0.0, "the order left no ring");

        // The Marios were sent to the same spot the ring is drawn on, each to
        // its own slot inside it.
        let squad = app.world().resource::<squad::Squad>();
        assert_eq!(squad.marching(), marios.len(), "the tap sent nobody");

        // And the ring entity is actually up, on that spot and that wide.
        let mut rings = app
            .world_mut()
            .query_filtered::<(&Transform, &Visibility), With<squad::OrderCircle>>();
        let (transform, visibility) = rings
            .single(app.world())
            .expect("no order ring in the world");
        assert!(
            matches!(visibility, Visibility::Visible),
            "the order ring stayed hidden"
        );
        assert!(
            transform.translation.distance(at) < 0.2,
            "the ring was drawn at {:?} for an order that landed at {at:?}",
            transform.translation
        );
        assert!(
            transform.scale.x <= radius + 1e-3 && transform.scale.x > 0.0,
            "the ring was drawn {} wide for a {radius} order",
            transform.scale.x
        );

        // It does not stay up: run past its life and it is gone.
        for _ in 0..160 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<squad::OrderMark>().fade(),
            0.0,
            "the order ring never faded out"
        );
        let mut rings = app
            .world_mut()
            .query_filtered::<&Visibility, With<squad::OrderCircle>>();
        assert!(
            matches!(rings.single(app.world()), Ok(Visibility::Hidden)),
            "the order ring is still on the ground"
        );
    }

    /// The fetch decision under the condition it actually runs in: a fight.
    ///
    /// **Both halves of the rule, in one staging, because they are one rule.**
    /// A Mario must not walk away from something it can see to pick a ball up
    /// -- and it must not be paralysed by the memory of something it noticed
    /// once and can no longer reach. Those pull in opposite directions and the
    /// game has been wrong in each direction in turn:
    ///
    ///   * Deciding began as a priority list with the fight above the ball. But
    ///     aggro in this game is *never given up*, so in any level with enemies
    ///     standing about every Mario acquires a target within seconds and keeps
    ///     it for the session. No ball was ever fetched, and the ones already in
    ///     hand were carried around the castle for ever.
    ///   * Scoring the two against each other fixed that and bought the opposite
    ///     complaint, which is the one a player actually sees: a Mario with a
    ///     slime on it turning its back to go and collect something.
    ///
    /// So [`goap::choose`] strikes hauling off the list while the quarry is
    /// within sight and scores it normally past that. This test is that
    /// sentence in the running game: a slime ten metres off, then the same slime
    /// forty metres off, with the same ball at the Mario's feet throughout.
    ///
    /// The integration test above stages an empty corner of the lawn and would
    /// miss both failures.
    #[test]
    fn a_mario_fights_what_it_can_see_and_fetches_once_it_cannot() {
        let mut app = headless();
        app.update();
        let here = {
            let mut player = app.world_mut().query_filtered::<&Transform, With<Player>>();
            player.single(app.world()).unwrap().translation
        };
        // The squad the castle spawns is somewhere else and would muddy the
        // count; this test is about one Mario. The tuning has to come down with
        // them, or `squad::maintain_population` puts the other seven straight
        // back -- and it is set to one rather than zero because the Mario spawned
        // below is the one it is then asked to keep.
        {
            let mut allies = app
                .world_mut()
                .query_filtered::<Entity, With<squad::Ally>>();
            let marios: Vec<Entity> = allies.iter(app.world()).collect();
            for mario in marios {
                app.world_mut().entity_mut(mario).despawn();
            }
            app.world_mut()
                .resource_mut::<console::GameTuning>()
                .ally_count = 1.0;
        }
        // Well clear of Luna, and that spacing is load-bearing rather than
        // decoration: a ball inside `nuclonium::MAGNET_RANGE` of her follows her
        // instead of waiting for a Mario, and one inside `MAST_REACH` of a live
        // mast is taken up by the mast. Either would answer this test without a
        // Mario ever deciding anything.
        let mario_at = here + Vec3::new(9.0, 0.0, 0.0);
        let ball_at = here + Vec3::new(10.0, 0.0, 0.0);
        // Ten metres off the Mario: inside `enemy_sight`, so it is noticed at
        // once, and ten times further away than the ball -- which is exactly
        // the gap a score hands to the ball and the rule has to hand to the
        // slime.
        let slime_at = here + Vec3::new(9.0, 0.0, 10.0);
        let slime = app
            .world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      assets: Res<AssetServer>,
                      art: Res<nuclonium::Art>| {
                    stellarator::spawn(&mut commands, &assets, here, 0.0, 0.5);
                    pylon::spawn(&mut commands, &assets, here + Vec3::new(0.0, 0.0, 3.0), 0.0);
                    squad::spawn_ally(
                        &mut commands,
                        &assets,
                        ActiveCharacter::Mario,
                        mario_at,
                        0.0,
                    );
                    nuclonium::spawn(
                        &mut commands,
                        &art,
                        nuclonium::Kind::Nuclonium,
                        ball_at,
                        0.0,
                    );
                    enemy::spawn(&mut commands, &assets, enemy::Kind::Slime, slime_at, 0.0)
                },
            )
            .unwrap();
        // Given a pool it cannot lose. This test is about what the Mario
        // *plans*, not about who wins: a slime that dies half way through takes
        // the fight away with it, and both halves below need the same live
        // target throughout.
        {
            let mut pool = app.world_mut().entity_mut(slime);
            let mut health = pool.get_mut::<health::Health>().unwrap();
            *health = health::Health::new(1_000_000);
        }

        // Settle first, then watch. `goap::plan` runs before `enemy::alert` in
        // the fixed step, so on the tick a slime is first noticed the plan
        // standing is one tick old by construction -- and a frame loop does not
        // run one fixed tick, it runs however many are owed. Asserting from the
        // first frame is asserting that the squad reads the future.
        for _ in 0..60 {
            app.update();
        }
        let sight = app.world().resource::<console::GameTuning>().enemy_sight;
        let mut watched = 0;
        for _ in 0..90 {
            app.update();
            let mut marios = app
                .world_mut()
                .query::<(&squad::Ally, &enemy::Aggro, &Transform)>();
            for (ally, aggro, at) in marios.iter(app.world()) {
                // Only while it is actually in the fight. A slime it has killed
                // or lost is the other half of this test, below.
                if aggro.target.is_none() || at.translation.distance(aggro.at) > sight {
                    continue;
                }
                watched += 1;
                assert!(
                    !matches!(
                        ally.plan,
                        goap::Goal::Fetch { .. } | goap::Goal::Deliver { .. }
                    ),
                    "a Mario {} m from what it has noticed went for the ball anyway: {:?}",
                    at.translation.distance(aggro.at),
                    ally.plan
                );
            }
        }
        assert!(
            watched > 10,
            "the slime was never noticed, so nothing above was tested"
        );

        // Now put the same slime out past sight without letting go of it. This
        // is the state every Mario in a populated level is really in -- holding
        // a target it noticed once -- and the squad has to get on with its work
        // in it. Moved rather than killed, precisely so that the target stays.
        app.world_mut()
            .entity_mut(slime)
            .get_mut::<Transform>()
            .unwrap()
            .translation = here + Vec3::new(9.0, 0.0, 40.0);
        let mut held_a_grudge = false;
        for _ in 0..400 {
            app.update();
            let mut marios = app.world_mut().query::<(&squad::Ally, &enemy::Aggro)>();
            held_a_grudge |= marios.iter(app.world()).any(|(ally, aggro)| {
                aggro.target.is_some()
                    && matches!(
                        ally.plan,
                        goap::Goal::Fetch { .. } | goap::Goal::Deliver { .. }
                    )
            });
            if app.world().resource::<nuclonium::Bank>().stored > 0 {
                break;
            }
        }
        assert!(
            held_a_grudge,
            "a Mario holding a distant target never planned to fetch -- \
             the grudge is standing in for a fight again"
        );
        assert_eq!(
            app.world().resource::<nuclonium::Bank>().stored,
            1,
            "the ball never reached the machine with a slime on the lawn"
        );
        // And what reached the machine is showing inside it: the field a
        // stellarator draws is its stock. See `stellarator::stock`.
        let mut stores = app.world_mut().query::<&stellarator::Store>();
        assert_eq!(
            stores
                .iter(app.world())
                .map(|store| store.held)
                .sum::<u32>(),
            1,
            "the machine banked it without taking it in"
        );
    }

    /// The jetpack is metered, end to end.
    ///
    /// The unit tests in [`energy`] prove the arithmetic; this proves the
    /// *wiring* -- that the component is on the player the game spawns, that
    /// `player::movement` finds it through its own query, and that the bar
    /// empties over five seconds of the real fixed step rather than five
    /// seconds of a number a test made up. A booster that flew forever would
    /// pass every test in `energy` and still be wrong here.
    ///
    /// It also holds the booster down through the whole lockout, which is what
    /// a player who has just fallen out of the sky actually does. That is the
    /// case a lockout is easiest to get wrong in: the refill has to run under a
    /// held key, or the game hangs on a bar that never comes back.
    #[test]
    fn the_jetpack_burns_the_energy_bar_and_letting_go_brings_it_back() {
        let mut app = headless();
        // Wider steps than the sixteen milliseconds the rest of these use:
        // this test is about durations rather than frames, and it has fifteen
        // seconds of them to get through.
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_millis(100),
        ));
        app.update();
        let mut players = app.world_mut().query_filtered::<Entity, With<Player>>();
        let luna = players
            .iter(app.world())
            .next()
            .expect("no player in the world");
        let bar = |app: &App| *app.world().get::<energy::Energy>(luna).unwrap();
        assert_eq!(bar(&app).level, 1.0, "he did not start with a full bar");
        // Off the ground and then the booster held down and *kept* down: five
        // seconds of flight, then three more of holding a dead key through the
        // lockout, landing partway through and skating on it. Every one of
        // those is a thing that would ordinarily park the refill, and none of
        // them may while the lockout is on.
        for frame in 0..80 {
            {
                let mut input = app.world_mut().resource_mut::<input::InputState>();
                input.jump = frame == 0;
                input.boost = true;
            }
            app.update();
        }
        let held = bar(&app);
        assert!(held.drained(), "the booster never ran dry");
        assert!(
            held.level > 0.0,
            "the refill was parked by the held key and the lockout could never end"
        );
        assert!(held.level < 1.0, "three seconds cannot have filled it");
        // Let go and wait out the rest. The bar coming back full is what ends
        // the lockout, so one assertion covers both.
        for _ in 0..40 {
            app.world_mut().resource_mut::<input::InputState>().boost = false;
            app.update();
        }
        assert_eq!(bar(&app).level, 1.0, "and it never came back");
        assert!(!bar(&app).drained(), "a full bar was still locked out");
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
            // The Tab picker rides in the same pipeline now.
            .init_resource::<action::Action>()
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
