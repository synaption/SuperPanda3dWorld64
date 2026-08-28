//! Attribution for the tall bars in [`crate::frame_chart`].
//!
//! The chart says a frame cost sixty milliseconds; it cannot say which part of
//! it did. This times each schedule of `Main` separately and, whenever a frame
//! runs over budget, writes one line naming the phases that made it up.
//!
//! The probes are their own *schedules* rather than systems added to the
//! existing ones, inserted into [`MainScheduleOrder`] between the real phases.
//! That is the only placement with no ambiguity: a marker system sharing
//! `Update` with three hundred others runs at whatever point the multi-threaded
//! executor felt like, which is precisely the measurement being taken.

use bevy::{app::MainScheduleOrder, ecs::schedule::ScheduleLabel, prelude::*};
use std::{
    io::Write,
    time::{Duration, Instant},
};

/// A frame slower than this is worth a line. Two and a half times the 60 fps
/// budget: below that the noise is the fixed step landing on some frames and
/// not others, which is expected and not what is being hunted.
const SPIKE_MS: f32 = 8.0;

/// The boundaries between the phases of `Main`, in the order they are crossed.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
enum Probe {
    Open,
    AfterFirst,
    AfterPreUpdate,
    AfterFixed,
    AfterUpdate,
    AfterSpawnScene,
    AfterPostUpdate,
    Close,
}

const PHASES: [&str; 7] = [
    "First",
    "PreUpdate",
    "FixedMain",
    "Update",
    "SpawnScene",
    "PostUpdate",
    "Last",
];

#[derive(Resource)]
pub struct Phases {
    frame_started: Instant,
    /// Cumulative time from the start of the frame to each boundary crossed.
    marks: Vec<Duration>,
    /// How many times `FixedMain` ran inside this frame.
    fixed_ticks: u32,
    entities: usize,
    /// Scenes asked for and thrown away by `enemy::shed_scenes` this frame.
    promoted: usize,
    demoted: usize,
    /// Frames seen, so a spike can be placed in the run.
    frame: u64,
    log: Option<std::path::PathBuf>,
}

impl Default for Phases {
    fn default() -> Self {
        Self {
            frame_started: Instant::now(),
            marks: Vec::with_capacity(8),
            fixed_ticks: 0,
            entities: 0,
            promoted: 0,
            demoted: 0,
            frame: 0,
            log: std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join("spikes.txt"))),
        }
    }
}

fn open_frame(mut phases: ResMut<Phases>) {
    phases.frame_started = Instant::now();
    phases.marks.clear();
    phases.fixed_ticks = 0;
    phases.frame += 1;
}

fn mark(mut phases: ResMut<Phases>) {
    let elapsed = phases.frame_started.elapsed();
    phases.marks.push(elapsed);
}

fn count_fixed_tick(mut phases: ResMut<Phases>) {
    phases.fixed_ticks += 1;
}

/// How many enemies crossed the swap distance on this frame's fixed steps.
///
/// Read off `WorldAssetRoot` rather than from inside `shed_scenes`, so the
/// count is of what actually reached the world after the command queue flushed.
fn count_churn(
    mut phases: ResMut<Phases>,
    gained: Query<(), Added<bevy::world_serialization::WorldAssetRoot>>,
    mut lost: RemovedComponents<bevy::world_serialization::WorldAssetRoot>,
) {
    phases.promoted = gained.iter().count();
    phases.demoted = lost.read().count();
}

fn count_entities(mut phases: ResMut<Phases>, entities: Query<()>) {
    phases.entities = entities.iter().count();
}

/// Closes the frame and, if it ran long, writes the breakdown.
///
/// Its own schedule after `Last`, for the same reason the rest are: a system
/// sharing `Last` would be timed at whatever point the executor reached it.
fn close_frame(
    mut phases: ResMut<Phases>,
    pacing: Res<crate::display::FramePacing>,
    settings: Res<crate::display::DisplaySettings>,
    tuning: Res<crate::console::GameTuning>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
) {
    // The frame cap sleeps inside `Last`, which these probes bracket. Left in,
    // a capped game reads as a game whose `Last` costs sixteen milliseconds.
    let total = phases.frame_started.elapsed().saturating_sub(pacing.slept);
    phases.marks.push(total);
    if total.as_secs_f32() * 1000.0 < SPIKE_MS {
        return;
    }
    // Cumulative marks differenced back into per-phase costs.
    let mut previous = Duration::ZERO;
    let mut parts = Vec::with_capacity(PHASES.len());
    // The wait is already off `total`, and `total` is the mark that closes the
    // list, so the `Last` span differenced out of it has lost the wait too --
    // taking it off again here would be taking it off twice.
    for (name, mark) in PHASES.iter().zip(phases.marks.iter()) {
        parts.push((*name, mark.saturating_sub(previous).as_secs_f32() * 1000.0));
        previous = *mark;
    }
    // Loudest first: the whole point of the line is which name is at the front
    // of it.
    parts.sort_by(|a, b| b.1.total_cmp(&a.1));
    let breakdown = parts
        .iter()
        .filter(|(_, ms)| *ms >= 0.05)
        .map(|(name, ms)| format!("{name} {ms:.1}"))
        .collect::<Vec<_>>()
        .join("  ");
    // Which knobs were where, on the line itself. These are all toggled at
    // runtime from the console and the pause menu, so a log without them is a
    // log whose settings have to be remembered -- and half of this
    // investigation was spent working out which build produced which file.
    let settings = format!(
        "vsync {} cap {} spin {} threads {}",
        match windows.single() {
            Ok(window) if crate::display::is_vsync(window.present_mode) => "on",
            Ok(_) => "off",
            Err(_) => "?",
        },
        match settings.frame_cap_hz() {
            Some(hz) => hz.to_string(),
            None => "off".to_string(),
        },
        if tuning.frame_spin >= 0.5 {
            "on"
        } else {
            "off"
        },
        bevy::tasks::ComputeTaskPool::get().thread_num(),
    );
    let line = format!(
        "frame {:>7}  {:5.1} ms  ticks {}  entities {}  +{}/-{} scenes  [{settings}]  | {breakdown}",
        phases.frame,
        total.as_secs_f32() * 1000.0,
        phases.fixed_ticks,
        phases.entities,
        phases.promoted,
        phases.demoted,
    );
    eprintln!("{line}");
    if let Some(path) = &phases.log {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Wires the probe schedules in between the real ones.
pub fn plugin(app: &mut App) {
    app.init_resource::<Phases>();
    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    order.insert_before(First, Probe::Open);
    order.insert_after(First, Probe::AfterFirst);
    order.insert_after(PreUpdate, Probe::AfterPreUpdate);
    order.insert_after(RunFixedMainLoop, Probe::AfterFixed);
    order.insert_after(Update, Probe::AfterUpdate);
    order.insert_after(SpawnScene, Probe::AfterSpawnScene);
    order.insert_after(PostUpdate, Probe::AfterPostUpdate);
    order.insert_after(Last, Probe::Close);
    app.add_systems(Probe::Open, open_frame)
        .add_systems(Probe::AfterFirst, mark)
        .add_systems(Probe::AfterPreUpdate, mark)
        .add_systems(Probe::AfterFixed, (mark, count_churn))
        .add_systems(Probe::AfterUpdate, mark)
        .add_systems(Probe::AfterSpawnScene, mark)
        .add_systems(Probe::AfterPostUpdate, (mark, count_entities))
        .add_systems(FixedFirst, count_fixed_tick)
        .add_systems(Probe::Close, close_frame);
}

// --- temporary harness: drives the game so a spike can be reproduced here ---
//
// `AUTOPILOT=1` walks the player in a circle and sweeps the camera, and
// `STARTUP_CONSOLE="crowd 2000 mix"` runs console lines once the level is up.
// Both exist so the profile below is taken on a moving player with a field
// down, which is the case the spikes were reported in.

#[derive(Resource)]
struct Harness {
    /// Read once. `env::var` takes a lock on the environment, which is not a
    /// thing to do sixty times a second inside a profiler.
    autopilot: bool,
    console: Option<String>,
    frames: u64,
    ran_console: bool,
    /// Every frame's total, for the periodic distribution.
    totals: Vec<f32>,
    /// Per-phase totals over the same window.
    phase_totals: [f32; PHASES.len()],
    phase_peaks: [f32; PHASES.len()],
    window_started: Option<Instant>,
}

impl Default for Harness {
    fn default() -> Self {
        Self {
            autopilot: std::env::var("AUTOPILOT").as_deref() == Ok("1"),
            console: std::env::var("STARTUP_CONSOLE").ok(),
            frames: 0,
            ran_console: false,
            totals: Vec::new(),
            phase_totals: [0.0; PHASES.len()],
            phase_peaks: [0.0; PHASES.len()],
            window_started: None,
        }
    }
}

fn autopilot(harness: Res<Harness>, mut input: ResMut<crate::input::InputState>, time: Res<Time>) {
    if !harness.autopilot {
        return;
    }
    let t = time.elapsed_secs();
    input.move_axis = Vec2::new(t.cos(), t.sin());
    input.look_mouse = Vec2::new(6.0, 0.0);
}

fn startup_console(world: &mut World) {
    {
        let mut harness = world.resource_mut::<Harness>();
        harness.frames += 1;
        if harness.ran_console || harness.frames < 180 {
            return;
        }
        harness.ran_console = true;
    }
    let Some(lines) = world.resource::<Harness>().console.clone() else {
        return;
    };
    for line in lines.split(';') {
        let mut tuning = world.resource::<crate::console::GameTuning>().clone();
        world
            .resource_mut::<crate::console::ConsoleState>()
            .execute(line.trim(), &mut tuning);
        *world.resource_mut::<crate::console::GameTuning>() = tuning;
    }
    eprintln!("harness: ran {lines:?}");
}

/// A distribution every five seconds, so the shape of the frame is visible
/// rather than only its outliers.
fn report(mut harness: ResMut<Harness>, phases: Res<Phases>) {
    let mut previous = Duration::ZERO;
    for (index, mark) in phases.marks.iter().enumerate().take(PHASES.len()) {
        let ms = mark.saturating_sub(previous).as_secs_f32() * 1000.0;
        harness.phase_totals[index] += ms;
        harness.phase_peaks[index] = harness.phase_peaks[index].max(ms);
        previous = *mark;
    }
    let total = phases
        .marks
        .last()
        .copied()
        .unwrap_or_default()
        .as_secs_f32()
        * 1000.0;
    harness.totals.push(total);
    let started = *harness.window_started.get_or_insert_with(Instant::now);
    if started.elapsed().as_secs_f32() < 5.0 {
        return;
    }
    let mut sorted = harness.totals.clone();
    sorted.sort_by(f32::total_cmp);
    let at = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
    let frames = sorted.len();
    eprintln!(
        "--- {frames} frames  p50 {:.1}  p95 {:.1}  p99 {:.1}  max {:.1} ms  entities {}",
        at(0.50),
        at(0.95),
        at(0.99),
        at(1.0),
        phases.entities,
    );
    for (index, name) in PHASES.iter().enumerate() {
        eprintln!(
            "      {name:<11} mean {:6.2}  peak {:6.2} ms",
            harness.phase_totals[index] / frames as f32,
            harness.phase_peaks[index],
        );
    }
    harness.totals.clear();
    harness.phase_totals = [0.0; PHASES.len()];
    harness.phase_peaks = [0.0; PHASES.len()];
    harness.window_started = Some(Instant::now());
}

pub fn harness(app: &mut App) {
    app.init_resource::<Harness>()
        .add_systems(Probe::AfterPreUpdate, autopilot)
        .add_systems(Update, startup_console)
        .add_systems(Probe::Close, report.after(close_frame));
}
