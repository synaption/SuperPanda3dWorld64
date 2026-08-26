//! In-game command console and live gameplay tuning.
//!
//! Command parsing and value clamping live independently of Bevy UI, making
//! the part that can change gameplay deterministic and headless-testable.

use crate::{
    enemy::Enemy,
    menu::MenuState,
    player::{Controller, Player},
    GameState,
};
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::{
        keyboard::{Key, KeyboardInput},
        mouse::{MouseScrollUnit, MouseWheel},
        ButtonState,
    },
    prelude::*,
};
use std::collections::VecDeque;

const LOG_LIMIT: usize = 200;

#[derive(Clone, Copy)]
pub struct TunableSpec {
    pub name: &'static str,
    pub low: f32,
    pub high: f32,
    pub step: f32,
    pub doc: &'static str,
}

pub const SPECS: &[TunableSpec] = &[
    TunableSpec {
        name: "hero_speed",
        low: 1.0,
        high: 30.0,
        step: 0.2,
        doc: "Hero running speed",
    },
    TunableSpec {
        name: "mario_speed",
        low: 1.0,
        high: 30.0,
        step: 0.2,
        doc: "Mario running speed",
    },
    TunableSpec {
        name: "walk_accel",
        low: 1.0,
        high: 60.0,
        step: 0.5,
        doc: "ground acceleration",
    },
    TunableSpec {
        name: "decel",
        low: 0.0,
        high: 30.0,
        step: 0.5,
        doc: "ground deceleration",
    },
    TunableSpec {
        name: "jump_velocity",
        low: 1.0,
        high: 30.0,
        step: 0.2,
        doc: "jump take-off speed",
    },
    TunableSpec {
        name: "skate_speed",
        low: 1.0,
        high: 40.0,
        step: 0.2,
        doc: "Hero skate speed",
    },
    TunableSpec {
        name: "skate_accel",
        low: 0.1,
        high: 30.0,
        step: 0.2,
        doc: "Hero skate acceleration",
    },
    TunableSpec {
        name: "jet_thrust",
        low: 0.0,
        high: 8.0,
        step: 0.1,
        doc: "booster lift per tick",
    },
    TunableSpec {
        name: "jet_rise",
        low: 0.0,
        high: 20.0,
        step: 0.2,
        doc: "booster terminal rise speed",
    },
    TunableSpec {
        name: "mario_swim",
        low: 0.5,
        high: 15.0,
        step: 0.2,
        doc: "Mario swim speed",
    },
    TunableSpec {
        name: "hero_wade",
        low: 0.5,
        high: 12.0,
        step: 0.2,
        doc: "Hero wading speed",
    },
    // The aiming layer. Every one of these came off a slider in the Panda3D
    // build rather than out of a calculation -- `docs/aim.md`, "As Built" --
    // so they are back on sliders here, at the values that build ended on.
    TunableSpec {
        name: "torso_limit",
        low: 0.0,
        high: 90.0,
        step: 1.0,
        doc: "degrees the torso twists before the feet come round",
    },
    TunableSpec {
        name: "torso_comfort",
        low: 0.0,
        high: 90.0,
        step: 1.0,
        doc: "degrees of twist he will stand still holding",
    },
    TunableSpec {
        name: "torso_response",
        low: 0.02,
        high: 0.6,
        step: 0.01,
        doc: "seconds the torso takes to reach a new aim",
    },
    TunableSpec {
        name: "torso_pitch",
        low: 0.0,
        high: 1.0,
        step: 0.05,
        doc: "share of the shot's elevation the chest takes",
    },
    TunableSpec {
        name: "torso_pitch_up",
        low: 0.0,
        high: 90.0,
        step: 1.0,
        doc: "degrees the torso may lean back",
    },
    TunableSpec {
        name: "torso_pitch_down",
        low: 0.0,
        high: 90.0,
        step: 1.0,
        doc: "degrees the torso may lean forward",
    },
    TunableSpec {
        name: "torso_turn_rate",
        low: 30.0,
        high: 720.0,
        step: 10.0,
        doc: "degrees a second the feet come round at",
    },
    TunableSpec {
        name: "gun_projectile",
        low: 0.0,
        high: 1.0,
        step: 1.0,
        doc: "fire every gun as a projectile rather than hitscan",
    },
    TunableSpec {
        name: "tracer_seconds",
        low: 0.0,
        high: 0.5,
        step: 0.005,
        doc: "how long a hitscan tracer stays on screen",
    },
    TunableSpec {
        name: "tracer_width",
        low: 0.005,
        high: 0.3,
        step: 0.005,
        doc: "how thick a tracer and a bullet are drawn",
    },
    TunableSpec {
        name: "bullet_speed",
        low: 5.0,
        high: 300.0,
        step: 5.0,
        doc: "metres a second a projectile shot travels",
    },
    TunableSpec {
        name: "cam_distance",
        low: 2.0,
        high: 24.0,
        step: 0.2,
        doc: "third-person camera distance",
    },
    TunableSpec {
        name: "cam_aim_distance",
        low: 2.0,
        high: 16.0,
        step: 0.2,
        doc: "aim camera distance",
    },
    TunableSpec {
        name: "cam_height",
        low: 0.0,
        high: 6.0,
        step: 0.1,
        doc: "camera focus height",
    },
    TunableSpec {
        name: "cam_smooth",
        low: 0.01,
        high: 1.0,
        step: 0.01,
        doc: "camera position response",
    },
    TunableSpec {
        name: "mouse_sens",
        low: 0.0005,
        high: 0.02,
        step: 0.0005,
        doc: "horizontal mouse sensitivity",
    },
    TunableSpec {
        name: "pad_look",
        low: 0.0,
        high: 8.0,
        step: 0.1,
        doc: "gamepad look speed in radians per second",
    },
    TunableSpec {
        name: "sfx_volume",
        low: 0.0,
        high: 1.0,
        step: 0.05,
        doc: "sound effect volume",
    },
    TunableSpec {
        name: "sfx_range",
        low: 1.0,
        high: 60.0,
        step: 1.0,
        doc: "metres a world sound carries at full volume before it fades",
    },
    TunableSpec {
        name: "ally_count",
        low: 0.0,
        high: 200.0,
        step: 1.0,
        doc: "Marios in the field",
    },
    TunableSpec {
        name: "ally_speed",
        low: 0.5,
        high: 30.0,
        step: 0.2,
        doc: "ally walking speed",
    },
    TunableSpec {
        name: "enemy_speed",
        low: 0.0,
        high: 10.0,
        step: 0.1,
        doc: "enemy chase speed",
    },
    TunableSpec {
        name: "enemy_sight",
        low: 0.0,
        high: 200.0,
        step: 1.0,
        doc: "how near a creature has to be to be noticed",
    },
    TunableSpec {
        name: "enemy_alert",
        low: 0.0,
        high: 200.0,
        step: 1.0,
        doc: "how far one creature's alarm carries to its own side",
    },
    TunableSpec {
        name: "sim_budget",
        low: 0.0,
        high: 2_000.0,
        step: 25.0,
        doc: "how many of the nearest enemies get the full simulation",
    },
    TunableSpec {
        name: "enemy_lod_near",
        low: 5.0,
        high: 150.0,
        step: 5.0,
        doc: "distance where enemy AI drops to 15 Hz",
    },
    TunableSpec {
        name: "enemy_lod_far",
        low: 10.0,
        high: 250.0,
        step: 5.0,
        doc: "distance where enemy AI drops to 7.5 Hz",
    },
    TunableSpec {
        name: "shadows",
        // A flag rather than a dial, and the arrow keys step it end to end.
        // Two draws and a mesh rebuilt every frame is not nothing at a crowd
        // this size, but that is not really why it is here: the discs are the
        // one thing in the scene that is blended, drawn last and laid across
        // every enemy's feet, which makes them the first suspect whenever
        // something is wrong with how the crowd looks. Being able to take them
        // away for a second answers that in one keystroke.
        low: 0.0,
        high: 1.0,
        step: 1.0,
        doc: "draw the contact shadows under everything",
    },
    TunableSpec {
        name: "enemy_draw",
        // Down from 20, because this is now a quality dial rather than a
        // visibility one and the interesting end of it is the near end.
        low: 5.0,
        high: 500.0,
        step: 5.0,
        doc: "distance where enemies become impostor sprites",
    },
    TunableSpec {
        name: "enemy_rate",
        low: 1.0 / 30.0,
        high: 60.0,
        step: 1.0 / 30.0,
        doc: "pipe spawn interval in seconds",
    },
    TunableSpec {
        name: "enemy_limit",
        low: 0.0,
        // A hundred thousand, which is a number to find the ceiling with rather
        // than a number to play at: the crowd tier is what makes it even
        // askable, and `sim_budget` still holds the fully simulated share to a
        // couple of hundred however big this gets. The arrow keys still step by
        // one, so reaching the top of this range is a matter of typing
        // `enemy_limit 100000` rather than of holding a key down.
        high: 100_000.0,
        step: 1.0,
        doc: "global live enemy cap",
    },
    TunableSpec {
        name: "pipe_brood",
        low: 0.0,
        high: 50.0,
        step: 1.0,
        doc: "how many allies the Mario pipe keeps alive",
    },
];

#[derive(Resource, Clone, Debug)]
pub struct GameTuning {
    pub hero_speed: f32,
    pub mario_speed: f32,
    pub walk_accel: f32,
    pub decel: f32,
    pub jump_velocity: f32,
    pub skate_speed: f32,
    pub skate_accel: f32,
    pub jet_thrust: f32,
    pub jet_rise: f32,
    pub mario_swim: f32,
    /// The Hero does not swim, he wades: `WADE_SPEED_SCALE` in
    /// `sm64py/hero/constants.py` is 0.45 of his walk, which is where the
    /// default below comes from.
    pub hero_wade: f32,
    /// The aiming layer, from `docs/aim.md`'s "As Built". See
    /// [`crate::aim`], which is the only reader of all seven.
    pub torso_limit: f32,
    pub torso_comfort: f32,
    pub torso_response: f32,
    pub torso_pitch: f32,
    pub torso_pitch_up: f32,
    pub torso_pitch_down: f32,
    pub torso_turn_rate: f32,
    /// Non-zero puts every gun on the projectile path instead of its own
    /// choice, so both resolutions stay reachable in a build shipping one gun.
    pub gun_projectile: f32,
    /// How a shot is *seen*. On sliders rather than in constants because a
    /// tracer is drawn nearly end-on -- it runs from the muzzle toward what the
    /// crosshair is over, which is roughly where the camera is already looking,
    /// so its whole length projects into a short streak beside the gun. What
    /// that looks like is not something the number predicts, and it took one
    /// wrong guess from a still frame to establish that it needed a slider.
    pub tracer_seconds: f32,
    pub tracer_width: f32,
    pub bullet_speed: f32,
    pub cam_distance: f32,
    pub cam_aim_distance: f32,
    pub cam_height: f32,
    pub cam_smooth: f32,
    /// Radians of yaw per mouse count.
    ///
    /// Halved from the 0.003 it was, because a mouse reports whole counts and
    /// this number is therefore the smallest movement the player can make: at
    /// 0.003 that was a fifth of a degree, three pixels of crosshair at 1080p,
    /// and there was no way to aim between two of them. `mouse_sens 0.003`
    /// puts the old speed back for anyone who wants it.
    pub mouse_sens: f32,
    pub pad_look: f32,
    pub sfx_volume: f32,
    pub sfx_range: f32,
    pub ally_count: f32,
    pub ally_speed: f32,
    pub enemy_speed: f32,
    pub enemy_sight: f32,
    pub enemy_alert: f32,
    /// How many of the nearest enemies are simulated in full. The rest are
    /// moved by the flow field -- see [`crate::enemy::Detail`].
    pub sim_budget: f32,
    pub enemy_lod_near: f32,
    pub enemy_lod_far: f32,
    pub enemy_draw: f32,
    /// Whether the contact discs are drawn at all. Off is a diagnostic rather
    /// than a look: see the spec above.
    pub shadows: f32,
    pub enemy_rate: f32,
    pub enemy_limit: f32,
    /// How many allies the Mario warp pipe keeps alive. Enemy pipes instead
    /// answer directly to the field-wide `enemy_limit` above.
    pub pipe_brood: f32,
}

impl Default for GameTuning {
    fn default() -> Self {
        Self {
            hero_speed: 11.4,
            mario_speed: 9.6,
            walk_accel: 22.0,
            decel: 10.0,
            jump_velocity: 12.6,
            skate_speed: 16.8,
            skate_accel: 9.0,
            jet_thrust: 2.4,
            jet_rise: 6.0,
            mario_swim: 5.5,
            hero_wade: 5.13,
            torso_limit: 60.0,
            torso_comfort: 20.0,
            torso_response: 0.12,
            torso_pitch: 0.55,
            torso_pitch_up: 60.0,
            torso_pitch_down: 45.0,
            // Not one of the recorded numbers -- the Panda3D build drove the
            // turn from its own controller rather than a rate -- so this is a
            // starting point rather than a tuned value. Roughly two thirds of a
            // turn a second, which keeps up with a stick without spinning.
            torso_turn_rate: 240.0,
            gun_projectile: 0.0,
            // The values the tracer shipped with and was legible at.
            tracer_seconds: 0.035,
            tracer_width: 0.03,
            bullet_speed: 90.0,
            cam_distance: 9.5,
            cam_aim_distance: 5.7,
            cam_height: 1.35,
            cam_smooth: 0.24,
            mouse_sens: 0.0015,
            pad_look: 2.6,
            sfx_volume: 0.7,
            // A shade under `enemy_sight`, so a slime close enough to have
            // noticed the player is close enough to be heard undimmed.
            sfx_range: 12.0,
            ally_count: 8.0,
            ally_speed: 7.0,
            enemy_speed: 1.8,
            enemy_sight: 14.0,
            enemy_alert: 9.0,
            // The budget the crowd work is built around: a couple of hundred
            // enemies get collision, jostling and the aggro chain, and
            // everything past that is carried by the flow field. Chosen as a
            // *count* rather than a distance on purpose -- it is a fixed amount
            // of CPU whether the field holds fifty enemies or five thousand.
            sim_budget: 200.0,
            enemy_lod_near: 35.0,
            enemy_lod_far: 70.0,
            // Where a skinned enemy becomes a sprite. It used to be 140, which
            // is wider than the castle is across -- so nothing was ever culled
            // and the cull may as well not have existed.
            //
            // 25 is chosen against `enemy_sight` rather than against a frame
            // budget: an enemy only notices the player within 14 units, so
            // everything that could conceivably be fighting you is comfortably
            // inside this and drawn as a real skeleton. What is beyond it is
            // scenery, and scenery a dozen pixels tall is exactly what an
            // impostor is for. Past here the whole far crowd is two draw calls
            // instead of two per slime and one per ant.
            enemy_draw: 25.0,
            shadows: 1.0,
            enemy_rate: 7.0,
            enemy_limit: 20.0,
            pipe_brood: 5.0,
        }
    }
}

impl GameTuning {
    pub fn get(&self, name: &str) -> Option<f32> {
        Some(match name {
            "hero_speed" => self.hero_speed,
            "mario_speed" => self.mario_speed,
            "walk_accel" => self.walk_accel,
            "decel" => self.decel,
            "jump_velocity" => self.jump_velocity,
            "skate_speed" => self.skate_speed,
            "skate_accel" => self.skate_accel,
            "jet_thrust" => self.jet_thrust,
            "jet_rise" => self.jet_rise,
            "mario_swim" => self.mario_swim,
            "hero_wade" => self.hero_wade,
            "torso_limit" => self.torso_limit,
            "torso_comfort" => self.torso_comfort,
            "torso_response" => self.torso_response,
            "torso_pitch" => self.torso_pitch,
            "torso_pitch_up" => self.torso_pitch_up,
            "torso_pitch_down" => self.torso_pitch_down,
            "torso_turn_rate" => self.torso_turn_rate,
            "gun_projectile" => self.gun_projectile,
            "tracer_seconds" => self.tracer_seconds,
            "tracer_width" => self.tracer_width,
            "bullet_speed" => self.bullet_speed,
            "cam_distance" => self.cam_distance,
            "cam_aim_distance" => self.cam_aim_distance,
            "cam_height" => self.cam_height,
            "cam_smooth" => self.cam_smooth,
            "mouse_sens" => self.mouse_sens,
            "pad_look" => self.pad_look,
            "sfx_volume" => self.sfx_volume,
            "sfx_range" => self.sfx_range,
            "ally_count" => self.ally_count,
            "ally_speed" => self.ally_speed,
            "enemy_speed" => self.enemy_speed,
            "enemy_sight" => self.enemy_sight,
            "enemy_alert" => self.enemy_alert,
            "sim_budget" => self.sim_budget,
            "enemy_lod_near" => self.enemy_lod_near,
            "enemy_lod_far" => self.enemy_lod_far,
            "enemy_draw" => self.enemy_draw,
            "shadows" => self.shadows,
            "enemy_rate" => self.enemy_rate,
            "enemy_limit" => self.enemy_limit,
            "pipe_brood" => self.pipe_brood,
            _ => return None,
        })
    }

    pub fn set(&mut self, name: &str, value: f32) -> Result<(f32, f32), String> {
        let Some(spec) = SPECS.iter().find(|spec| spec.name == name) else {
            return Err(format!("no such variable: {name}  (try `vars`)"));
        };
        let previous = self.get(name).unwrap();
        let value = value.clamp(spec.low, spec.high);
        match name {
            "hero_speed" => self.hero_speed = value,
            "mario_speed" => self.mario_speed = value,
            "walk_accel" => self.walk_accel = value,
            "decel" => self.decel = value,
            "jump_velocity" => self.jump_velocity = value,
            "skate_speed" => self.skate_speed = value,
            "skate_accel" => self.skate_accel = value,
            "jet_thrust" => self.jet_thrust = value,
            "jet_rise" => self.jet_rise = value,
            "mario_swim" => self.mario_swim = value,
            "hero_wade" => self.hero_wade = value,
            "torso_limit" => self.torso_limit = value,
            "torso_comfort" => self.torso_comfort = value,
            "torso_response" => self.torso_response = value,
            "torso_pitch" => self.torso_pitch = value,
            "torso_pitch_up" => self.torso_pitch_up = value,
            "torso_pitch_down" => self.torso_pitch_down = value,
            "torso_turn_rate" => self.torso_turn_rate = value,
            "gun_projectile" => self.gun_projectile = value,
            "tracer_seconds" => self.tracer_seconds = value,
            "tracer_width" => self.tracer_width = value,
            "bullet_speed" => self.bullet_speed = value,
            "cam_distance" => self.cam_distance = value,
            "cam_aim_distance" => self.cam_aim_distance = value,
            "cam_height" => self.cam_height = value,
            "cam_smooth" => self.cam_smooth = value,
            "mouse_sens" => self.mouse_sens = value,
            "pad_look" => self.pad_look = value,
            "sfx_volume" => self.sfx_volume = value,
            "sfx_range" => self.sfx_range = value,
            "ally_count" => self.ally_count = value,
            "ally_speed" => self.ally_speed = value,
            "enemy_speed" => self.enemy_speed = value,
            "enemy_sight" => self.enemy_sight = value,
            "enemy_alert" => self.enemy_alert = value,
            "sim_budget" => self.sim_budget = value,
            "enemy_lod_near" => self.enemy_lod_near = value,
            "enemy_lod_far" => self.enemy_lod_far = value,
            "enemy_draw" => self.enemy_draw = value,
            "shadows" => self.shadows = value,
            "enemy_rate" => self.enemy_rate = value,
            "enemy_limit" => self.enemy_limit = value,
            "pipe_brood" => self.pipe_brood = value,
            _ => unreachable!(),
        }
        Ok((previous, value))
    }
}

/// The largest crowd `crowd` will place, however big a number it is handed.
///
/// A typo in a benchmark command should cost a second, not the session. The
/// same ceiling `enemy_limit` has, so that the two ways of filling a field --
/// letting the pipes do it and placing one outright -- can reach the same size
/// and a benchmark is not quietly capped below the cap it is testing.
const CROWD_LIMIT: usize = 100_000;

/// What a `crowd` command's kind argument may be spelled, and what it means.
///
/// One table rather than a match arm each, so that the prefix matching in
/// [`ConsoleState::crowd`] can see all of the names at once and tell an
/// ambiguous abbreviation from an unknown one.
const CROWD_NAMES: [(&str, CrowdKind); 3] = [
    ("slime", CrowdKind::Slime),
    ("ant", CrowdKind::Ant),
    ("mix", CrowdKind::Mix),
];

/// Which kind a `crowd` argument names, or what to say about it.
///
/// Prefixes, so `crowd 2000 m` works: this is a command typed between two
/// readings of a frame-rate counter, not a configuration file. Matched against
/// every name rather than taken in order, because first match wins is how
/// `crowd 2000 s` quietly ran a benchmark of the wrong enemy back when the
/// slime and the scuttlebug shared a letter.
///
/// Free-standing, and handed its table rather than reading [`CROWD_NAMES`]
/// itself, so that the ambiguous case stays under test: the three names the
/// game ships now start with three different letters, and no line a player can
/// type reaches that branch any more.
fn crowd_kind(word: &str, names: &[(&str, CrowdKind)]) -> Result<CrowdKind, String> {
    let matched: Vec<_> = names
        .iter()
        .filter(|(name, _)| name.starts_with(word))
        .map(|(name, kind)| (*name, *kind))
        .collect();
    match matched.as_slice() {
        [(_, kind)] => Ok(*kind),
        [] => {
            let all: Vec<_> = names.iter().map(|(name, _)| *name).collect();
            Err(format!("{word:?} is not {}", or_list(&all)))
        }
        several => {
            let all: Vec<_> = several.iter().map(|(name, _)| *name).collect();
            Err(format!(
                "{word:?} could be {} -- say more of it",
                or_list(&all)
            ))
        }
    }
}

/// `["slime", "ant", "mix"]` as `"slime, ant or mix"`.
fn or_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// Which enemies a `crowd` command asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrowdKind {
    Slime,
    Ant,
    /// Half of each, alternating, which is the case the draw-call cost of a
    /// mixed field is actually measured on.
    Mix,
}

/// A side effect a command asked for, queued for a system with the access to
/// carry it out.
///
/// [`ConsoleState::execute`] is deliberately Bevy-free: it takes a line and a
/// [`GameTuning`] and touches nothing else, which is what lets the whole
/// command table be tested without a renderer. Putting a crowd on the map needs
/// `Commands`, the asset server and the level's collision, so the command
/// records what it wants here and [`crate::enemy::crowd`] does it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Put this many enemies on the map, of this mix.
    Crowd(usize, CrowdKind),
    /// Take every enemy off it again.
    ClearCrowd,
    /// Put a named weapon in the Hero's hand.
    ///
    /// The weapon is a resource rather than a tunable float, so unlike every
    /// slider it cannot be set by writing into [`GameTuning`] -- hence a
    /// request. It is what lets `cargo run -- screenshot` photograph the gun:
    /// `SHOT_SETUP="weapon pistol"`, through the same path a player uses.
    Equip(crate::weapon::Weapon),
}

#[derive(Resource)]
pub struct ConsoleState {
    pub open: bool,
    pub closed_this_frame: bool,
    /// What the commands run so far have asked the world to do, in the order
    /// they asked. Drained by whoever can do it.
    pending: Vec<Request>,
    input: String,
    /// Where the caret sits in `input`, as a byte offset. Always on a character
    /// boundary: every move steps by whole characters, so a multi-byte one
    /// cannot be split down the middle and panic the string operations.
    cursor: usize,
    log: VecDeque<String>,
    history: Vec<String>,
    history_at: usize,
    pinned: Vec<String>,
    selected: Option<String>,
    scroll: usize,
    defaults: GameTuning,
}

impl Default for ConsoleState {
    fn default() -> Self {
        let mut log = VecDeque::new();
        log.push_back("Bevy tuning console ready. Type `help`.".into());
        Self {
            open: false,
            closed_this_frame: false,
            pending: Vec::new(),
            input: String::new(),
            cursor: 0,
            log,
            history: Vec::new(),
            history_at: 0,
            pinned: Vec::new(),
            selected: None,
            scroll: 0,
            defaults: GameTuning::default(),
        }
    }
}

impl ConsoleState {
    // -- line editing -------------------------------------------------------
    //
    // A caret rather than an append-only line. Typing a variable name and
    // realising the number in the middle of it is wrong should not mean
    // deleting back to it and typing the rest again.
    //
    // All of this is plain string work on `input` and `cursor` with no Bevy
    // anywhere, so the whole of it is exercised by tests that never open a
    // window -- the same split the rest of the console keeps.

    /// The text either side of the caret.
    pub fn split(&self) -> (&str, &str) {
        self.input.split_at(self.cursor)
    }

    /// Puts a typed character in at the caret and steps over it.
    fn insert(&mut self, character: char) {
        self.input.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    /// Deletes the character before the caret, the way Backspace does.
    fn backspace(&mut self) {
        let Some(character) = self.input[..self.cursor].chars().next_back() else {
            return;
        };
        self.cursor -= character.len_utf8();
        self.input.remove(self.cursor);
    }

    /// Deletes the character the caret sits on, the way Delete does.
    fn delete(&mut self) {
        if self.cursor < self.input.len() {
            self.input.remove(self.cursor);
        }
    }

    /// Moves the caret one character left or right, stopping at either end.
    fn step(&mut self, direction: i8) {
        self.cursor = match direction {
            ..=-1 => self.input[..self.cursor]
                .chars()
                .next_back()
                .map_or(0, |character| self.cursor - character.len_utf8()),
            _ => self.input[self.cursor..]
                .chars()
                .next()
                .map_or(self.input.len(), |character| {
                    self.cursor + character.len_utf8()
                }),
        };
    }

    /// Replaces the line and puts the caret at the end of it, which is what
    /// recalling a command from the history or clearing the line both want.
    fn set_input(&mut self, line: String) {
        self.input = line;
        self.cursor = self.input.len();
    }

    /// Puts a line in the console's log from outside it.
    ///
    /// For the things that go wrong at startup and then stay wrong quietly. A
    /// packaged Windows build has no stderr attached to anything, so `eprintln`
    /// there is the same as saying nothing -- and "the impostor sheets are
    /// missing so no distant enemy will be drawn" is exactly the kind of fact
    /// that otherwise reaches the player as a mystery rather than a message.
    pub fn report(&mut self, message: impl Into<String>) {
        self.echo(message);
    }

    fn echo(&mut self, message: impl Into<String>) {
        for line in message.into().lines() {
            if self.log.len() == LOG_LIMIT {
                self.log.pop_front();
            }
            self.log.push_back(line.to_owned());
        }
    }

    pub fn execute(&mut self, line: &str, tuning: &mut GameTuning) {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.is_empty() {
            return;
        }
        self.echo(format!("> {line}"));
        match words[0].to_ascii_lowercase().as_str() {
            "help" | "?" => self.echo("commands: <name> [value], vars, reset <name|all>, close <name|all>, clear\ncrowd <n> [slime|ant|mix] puts a whole field down at once; crowd clear takes it away.
weapon <sword|pistol> puts one in the Hero's hand; Y cycles it in play.\nLeft/Right/Home/End move the caret; Up/Down recall; Tab completes.\nSelect a variable then use [ and ] (Shift = 10x) to tune it. Wheel/PageUp/PageDown scroll the log."),
            "vars" | "list" => {
                for spec in SPECS {
                    self.echo(format!("  {:<18} {:>7.3}  [{:.3} .. {:.3}]  {}", spec.name, tuning.get(spec.name).unwrap(), spec.low, spec.high, spec.doc));
                }
            }
            "clear" => self.log.clear(),
            "crowd" => self.crowd(&words[1..]),
            "weapon" | "equip" => self.weapon(&words[1..]),
            "reset" => self.reset(&words[1..], tuning),
            "close" | "hide" => self.close(&words[1..]),
            "set" | "slider" | "var" if words.len() > 1 => self.tunable(words[1], &words[2..], tuning),
            name if tuning.get(name).is_some() => self.tunable(name, &words[1..], tuning),
            name => {
                let matches: Vec<_> = SPECS.iter().filter(|s| s.name.starts_with(name)).map(|s| s.name).collect();
                let hint = if matches.is_empty() { String::new() } else { format!(" -- did you mean {}?", matches.join(", ")) };
                self.echo(format!("unknown: {}{}  (try `help`)", words[0], hint));
            }
        }
    }

    /// Everything the commands have asked the world for, taken away as it is
    /// read: a request carried out twice is a crowd placed twice.
    pub fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.pending)
    }

    /// Puts back a request this reader was not the one for.
    ///
    /// [`take_requests`](Self::take_requests) drains the queue whole, which is
    /// what stops a crowd being placed twice, but it also means the first
    /// reader to run takes everything -- including the requests meant for
    /// somebody else. Handing those back is how more than one system can share
    /// the queue without either of them having to know what the others want.
    pub fn defer(&mut self, request: Request) {
        self.pending.push(request);
    }

    /// `weapon <sword|pistol>`, or `weapon` to say what is in hand.
    ///
    /// Named rather than cycled, so a screenshot or a repro asks for the
    /// weapon it wants instead of counting presses of `Y` from whatever the
    /// last session left equipped.
    fn weapon(&mut self, words: &[&str]) {
        let names: Vec<&'static str> = crate::weapon::Weapon::ALL
            .iter()
            .map(|weapon| weapon.spec().name)
            .collect();
        let Some(asked) = words.first() else {
            self.echo(format!("weapon <{}>", names.join("|")));
            return;
        };
        let found = crate::weapon::Weapon::ALL
            .iter()
            .find(|weapon| weapon.spec().name == *asked);
        match found {
            Some(weapon) => {
                self.pending.push(Request::Equip(*weapon));
                self.echo(format!("equipped {asked}"));
            }
            None => self.echo(format!("no weapon {asked:?} -- try {}", names.join(", "))),
        }
    }

    /// `crowd <n> [slime|ant|mix]`, or `crowd clear`.
    ///
    /// The benchmark command. `enemy_limit` and `enemy_rate` can already fill
    /// the field, but they do it a brood at a time over a minute or more, and a
    /// field that arrives gradually is no way to compare two builds. This puts
    /// the whole crowd down at once, in the same places every time, so what
    /// changes between two runs is the build rather than the layout.
    fn crowd(&mut self, args: &[&str]) {
        if matches!(args.first(), Some(&"clear") | Some(&"none")) {
            self.pending.push(Request::ClearCrowd);
            self.echo("crowd: clearing the field");
            return;
        }
        let Some(count) = args.first().and_then(|raw| raw.parse::<usize>().ok()) else {
            self.echo("crowd: needs a count -- `crowd 2000 mix`, or `crowd clear`");
            return;
        };
        let kind = match args.get(1).map(|word| word.to_ascii_lowercase()) {
            None => CrowdKind::Mix,
            Some(word) => match crowd_kind(&word, &CROWD_NAMES) {
                Ok(kind) => kind,
                Err(why) => {
                    self.echo(format!("crowd: {why}"));
                    return;
                }
            },
        };
        let count = count.min(CROWD_LIMIT);
        self.pending.push(Request::Crowd(count, kind));
        self.echo(format!("crowd: placing {count}, {kind:?}"));
    }

    fn tunable(&mut self, name: &str, args: &[&str], tuning: &mut GameTuning) {
        if let Some(raw) = args.first() {
            match raw.parse::<f32>() {
                Ok(value) => match tuning.set(name, value) {
                    Ok((old, new)) => self.echo(format!("{name} = {new:.3}  (was {old:.3})")),
                    Err(error) => self.echo(error),
                },
                Err(_) => self.echo(format!("{name}: {raw:?} is not a number")),
            }
        } else {
            if !self.pinned.iter().any(|item| item == name) {
                self.pinned.push(name.to_owned());
            }
            self.selected = Some(name.to_owned());
            self.echo(format!(
                "{name} = {:.3}  -- control added",
                tuning.get(name).unwrap()
            ));
        }
    }

    fn reset(&mut self, args: &[&str], tuning: &mut GameTuning) {
        let names: Vec<_> = if args.first() == Some(&"all") {
            SPECS.iter().map(|s| s.name).collect()
        } else {
            args.to_vec()
        };
        if names.is_empty() {
            self.echo("reset: needs a variable name, or `all`");
            return;
        }
        for name in names {
            if let Some(value) = self.defaults.get(name) {
                let _ = tuning.set(name, value);
                self.echo(format!("{name} = {value:.3}  (default)"));
            } else {
                self.echo(format!("no such variable: {name}"));
            }
        }
    }

    fn close(&mut self, args: &[&str]) {
        if args.first() == Some(&"all") {
            self.pinned.clear();
            self.selected = None;
            self.echo("all controls closed");
        } else if let Some(name) = args.first() {
            self.pinned.retain(|item| item != name);
            if self.selected.as_deref() == Some(name) {
                self.selected = self.pinned.last().cloned();
            }
            self.echo(format!("{name} control closed"));
        } else {
            self.echo("close: needs a control name, or `all`");
        }
    }

    /// Completes the word the caret is in, rather than the end of the line: the
    /// caret is where the typing is happening, and completing somewhere else
    /// would rewrite a word the user had already moved on from.
    fn complete(&mut self) {
        let head = &self.input[..self.cursor];
        let prefix = head
            .rsplit(|character: char| character.is_whitespace())
            .next()
            .unwrap_or("");
        let matches: Vec<_> = SPECS
            .iter()
            .filter(|s| s.name.starts_with(prefix))
            .collect();
        if matches.len() == 1 {
            let start = self.cursor - prefix.len();
            self.input
                .replace_range(start..self.cursor, matches[0].name);
            self.cursor = start + matches[0].name.len();
        } else if !matches.is_empty() {
            self.echo(
                matches
                    .iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join("  "),
            );
        }
    }
}

#[derive(Component)]
pub struct ConsolePanel;
#[derive(Component)]
pub struct TuningTray;

/// The console panel's marker and the components that draw it.
///
/// A tuple of components rather than a bundle now that bundles are gone. The
/// old warning about keeping every built-in field inside one bundle has become
/// the language's problem rather than this function's: naming a component twice
/// in one tuple does not compile, where the duplicate it used to guard against
/// was a panic at startup.
///
/// `GlobalZIndex` rather than `ZIndex`: the console draws over the whole game
/// and must be ordered against every other root, not against its siblings.
pub fn panel_bundle() -> (
    ConsolePanel,
    Node,
    Text,
    TextFont,
    TextColor,
    BackgroundColor,
    GlobalZIndex,
    Visibility,
) {
    (
        ConsolePanel,
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            right: Val::Px(12.0),
            top: Val::Px(10.0),
            padding: UiRect::all(Val::Px(14.0)),
            ..default()
        },
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(17.0),
            ..default()
        },
        TextColor(Color::srgb(0.88, 0.92, 1.0)),
        BackgroundColor(Color::srgba(0.015, 0.02, 0.04, 0.94)),
        GlobalZIndex(100),
        Visibility::Hidden,
    )
}

/// Persistent controls shown below the console while gameplay is running.
pub fn tuning_tray_bundle() -> (
    TuningTray,
    Node,
    Text,
    TextFont,
    TextColor,
    BackgroundColor,
    GlobalZIndex,
) {
    (
        TuningTray,
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(18.0),
            bottom: Val::Px(18.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::srgb(1.0, 0.92, 0.62)),
        BackgroundColor(Color::srgba(0.02, 0.025, 0.05, 0.72)),
        GlobalZIndex(90),
    )
}

pub fn is_closed(console: Res<ConsoleState>) -> bool {
    !console.open
}

/// The keys that repeat while they are held down.
const EDIT_KEYS: &[KeyCode] = &[
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::Backspace,
    KeyCode::Delete,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
];

/// How long a key has to be held before it starts repeating, and how fast it
/// repeats once it does. A terminal's numbers, near enough.
const REPEAT_DELAY: f32 = 0.4;
const REPEAT_INTERVAL: f32 = 0.04;

/// Auto-repeat for the editing keys.
///
/// Bevy reports a press and a release with nothing in between, so a held arrow
/// key moves the caret exactly one character however long it is held -- which
/// makes getting back to the start of a long line a matter of tapping. One key
/// repeats at a time, the last one pressed, which is what a terminal does.
///
/// Kept as counts of what is owed rather than a "fire now" flag, so a slow
/// frame delivers the repeats it covered instead of swallowing them.
#[derive(Default)]
pub struct KeyRepeat {
    key: Option<KeyCode>,
    held: f32,
    fired: u32,
}

impl KeyRepeat {
    /// Advances the clock and says which key should act this frame, and how
    /// many times.
    fn poll(&mut self, keys: &ButtonInput<KeyCode>, dt: f32) -> (Option<KeyCode>, u32) {
        if let Some(key) = EDIT_KEYS
            .iter()
            .copied()
            .find(|key| keys.just_pressed(*key))
        {
            *self = Self {
                key: Some(key),
                held: 0.0,
                fired: 0,
            };
            return (Some(key), 1);
        }
        let Some(key) = self.key.filter(|key| keys.pressed(*key)) else {
            self.key = None;
            return (None, 0);
        };
        self.held += dt;
        if self.held < REPEAT_DELAY {
            return (Some(key), 0);
        }
        let due = ((self.held - REPEAT_DELAY) / REPEAT_INTERVAL) as u32 + 1;
        let owed = due.saturating_sub(self.fired);
        self.fired = due;
        (Some(key), owed)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn input(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut typed: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<MouseWheel>,
    menu: Res<MenuState>,
    mut console: ResMut<ConsoleState>,
    mut tuning: ResMut<GameTuning>,
    mut repeat: Local<KeyRepeat>,
) {
    console.closed_this_frame = false;
    // The menu has the keyboard while it is up, and the two cannot share it:
    // the grave key would open a console nobody can see behind the menu, and
    // the bracket keys would tune the pinned control while the player is
    // walking a list of settings.
    if menu.open {
        return;
    }
    if keys.just_pressed(KeyCode::Backquote) {
        console.open = !console.open;
        if console.open {
            console.echo("simulation paused");
        }
    }
    // Brackets tune the pinned control whether the console is open or shut. The
    // arrow keys used to do it while it was open, and cannot any more: they
    // move the caret, and a console you cannot move the caret in is a console
    // you retype every line in.
    let direction = i8::from(keys.just_pressed(KeyCode::BracketRight))
        - i8::from(keys.just_pressed(KeyCode::BracketLeft));
    adjust_selected(direction, &keys, &console, &mut tuning);
    if !console.open {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        console.open = false;
        console.closed_this_frame = true;
        return;
    }
    if keys.just_pressed(KeyCode::Enter) {
        let line = std::mem::take(&mut console.input);
        console.cursor = 0;
        if !line.trim().is_empty() {
            console.scroll = 0;
            console.history.push(line.clone());
            console.history_at = console.history.len();
            console.execute(&line, &mut tuning);
        }
    }
    if keys.just_pressed(KeyCode::Tab) {
        console.complete();
    }
    if keys.just_pressed(KeyCode::Home) {
        console.cursor = 0;
    }
    if keys.just_pressed(KeyCode::End) {
        console.cursor = console.input.len();
    }
    let (key, times) = repeat.poll(&keys, time.delta_secs());
    for _ in 0..times {
        match key {
            Some(KeyCode::ArrowLeft) => console.step(-1),
            Some(KeyCode::ArrowRight) => console.step(1),
            Some(KeyCode::Backspace) => console.backspace(),
            Some(KeyCode::Delete) => console.delete(),
            Some(KeyCode::ArrowUp) if !console.history.is_empty() => {
                console.history_at = console.history_at.saturating_sub(1);
                let line = console.history[console.history_at].clone();
                console.set_input(line);
            }
            Some(KeyCode::ArrowDown) => {
                console.history_at = (console.history_at + 1).min(console.history.len());
                let line = if console.history_at == console.history.len() {
                    String::new()
                } else {
                    console.history[console.history_at].clone()
                };
                console.set_input(line);
            }
            _ => {}
        }
    }
    let wheel_lines: f32 = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y * 3.0,
            MouseScrollUnit::Pixel => event.y / 20.0,
        })
        .sum();
    let max_scroll = console.log.len().saturating_sub(16);
    if wheel_lines > 0.0 || keys.just_pressed(KeyCode::PageUp) {
        let amount = if keys.just_pressed(KeyCode::PageUp) {
            16
        } else {
            wheel_lines.round() as usize
        };
        console.scroll = (console.scroll + amount).min(max_scroll);
    } else if wheel_lines < 0.0 || keys.just_pressed(KeyCode::PageDown) {
        let amount = if keys.just_pressed(KeyCode::PageDown) {
            16
        } else {
            (-wheel_lines).round() as usize
        };
        console.scroll = console.scroll.saturating_sub(amount);
    }
    // Typed text is read off the keyboard event's *logical* key rather than
    // from a separate character event, which is the stream that no longer
    // exists. Logical is the right half of the pair here: `key_code` is the
    // physical key and would type an American layout on every keyboard in the
    // world, where `Key::Character` is whatever the layout actually commits --
    // and it can commit more than one char at a time, which is what a dead key
    // resolving or an IME does.
    //
    // Space is its own logical key rather than a `Character`, so it has to be
    // named; without that the console silently refuses to type a space.
    for event in typed.read() {
        if event.state != ButtonState::Pressed {
            continue;
        }
        match &event.logical_key {
            Key::Character(text) => {
                for character in text.chars() {
                    // The backquote opens the console and must not also type
                    // itself into the line it just opened.
                    if !character.is_control() && character != '`' && character != '~' {
                        console.insert(character);
                    }
                }
            }
            Key::Space => console.insert(' '),
            _ => {}
        }
    }
}

fn adjust_selected(
    direction: i8,
    keys: &ButtonInput<KeyCode>,
    console: &ConsoleState,
    tuning: &mut GameTuning,
) {
    if direction == 0 {
        return;
    }
    if let Some(name) = &console.selected {
        let spec = SPECS.iter().find(|spec| spec.name == name).unwrap();
        let multiplier = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            10.0
        } else {
            1.0
        };
        let value = tuning.get(name).unwrap() + direction as f32 * spec.step * multiplier;
        let _ = tuning.set(name, value);
    }
}

/// Bevy's animation clock continues independently of fixed gameplay ticks, so
/// explicitly pause clips while the console owns the simulation.
/// The enemies are left out and handled by [`crate::enemy::sync_animation_visibility`]
/// instead, which has to tell a culled enemy from a merely paused one -- a
/// distinction this cannot make and would undo every frame.
///
/// Guarded on what each player is already doing, and read through the shared
/// reference so that asking costs nothing. `pause_all` and `resume_all` mark
/// the component changed whether or not they changed anything, and this runs
/// over every animation player in the world on every frame: unguarded, a field
/// of two thousand puts two thousand components through change detection every
/// frame to tell them to carry on doing what they were doing.
pub fn pause_animations(
    console: Res<ConsoleState>,
    mut players: Query<&mut AnimationPlayer, Without<crate::enemy::EnemyAnimationRoot>>,
) {
    for mut player in &mut players {
        if player.playing_animations().next().is_none() || player.all_paused() == console.open {
            continue;
        }
        if console.open {
            player.pause_all();
        } else {
            player.resume_all();
        }
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn draw(
    console: Res<ConsoleState>,
    tuning: Res<GameTuning>,
    state: Res<GameState>,
    diagnostics: Res<DiagnosticsStore>,
    player: Query<(&Controller, &Transform), With<Player>>,
    enemies: Query<(), With<Enemy>>,
    mut panel: Query<(&mut Text, &mut Visibility), (With<ConsolePanel>, Without<TuningTray>)>,
    mut tray: Query<&mut Text, (With<TuningTray>, Without<ConsolePanel>)>,
) {
    let Ok((mut text, mut visibility)) = panel.single_mut() else {
        return;
    };
    *visibility = if console.open {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if console.open {
        let Ok((controller, transform)) = player.single() else {
            return;
        };
        let fps = diagnostics
            .get(&FrameTimeDiagnosticsPlugin::FPS)
            .and_then(|diagnostic| diagnostic.smoothed())
            .unwrap_or(0.0);
        let log = console
            .log
            .iter()
            .rev()
            .skip(console.scroll)
            .take(16)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        // The caret is drawn where it actually is rather than always at the end
        // of the line, which is the only way moving it is visible at all.
        let (before, after) = console.split();
        **text = format!(
            "BEVY DEBUG CONSOLE  ·  paused · log scroll {}\n{:?} · {:?} · health {} · enemies {} · {fps:.1} fps\npos {:.2}, {:.2}, {:.2}\n\n{log}\n\n> {before}|{after}",
            console.scroll,
            state.active,
            controller.motion,
            controller.health,
            enemies.iter().len(),
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        );
    }
    let Ok(mut tray) = tray.single_mut() else {
        return;
    };
    **tray = console
        .pinned
        .iter()
        .map(|name| {
            let marker = if console.selected.as_deref() == Some(name) {
                ">"
            } else {
                " "
            };
            format!("{marker} {name:<18} {:>7.3}", tuning.get(name).unwrap())
        })
        .collect::<Vec<_>>()
        .join("\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_sets_clamps_and_resets_tunables() {
        let mut console = ConsoleState::default();
        let mut tuning = GameTuning::default();
        console.execute("hero_speed 999", &mut tuning);
        assert_eq!(tuning.hero_speed, 30.0);
        console.execute("reset hero_speed", &mut tuning);
        assert_eq!(tuning.hero_speed, GameTuning::default().hero_speed);
    }

    #[test]
    fn bare_name_pins_a_persistent_control() {
        let mut console = ConsoleState::default();
        let mut tuning = GameTuning::default();
        console.execute("cam_distance", &mut tuning);
        assert_eq!(console.pinned, vec!["cam_distance"]);
        assert_eq!(console.selected.as_deref(), Some("cam_distance"));
        console.execute("close all", &mut tuning);
        assert!(console.pinned.is_empty());
    }

    /// The benchmark command, including the prefix forms, since the whole point
    /// of it is being typed quickly between two readings of a frame counter.
    #[test]
    fn crowd_queues_a_field_of_the_asked_for_size_and_mix() {
        let mut console = ConsoleState::default();
        let mut tuning = GameTuning::default();
        console.execute("crowd 2000 mix", &mut tuning);
        console.execute("crowd 40 sl", &mut tuning);
        console.execute("crowd 40 ant", &mut tuning);
        // No kind named is the mixed field, which is the one worth measuring.
        console.execute("crowd 7", &mut tuning);
        console.execute("crowd clear", &mut tuning);
        assert_eq!(
            console.take_requests(),
            vec![
                Request::Crowd(2000, CrowdKind::Mix),
                Request::Crowd(40, CrowdKind::Slime),
                Request::Crowd(40, CrowdKind::Ant),
                Request::Crowd(7, CrowdKind::Mix),
                Request::ClearCrowd,
            ]
        );
        // And taking them empties the queue: a request carried out on two
        // consecutive frames is a field placed twice.
        assert!(console.take_requests().is_empty());
    }

    /// An abbreviation that could mean two things must not quietly pick one.
    ///
    /// The version of this that took the first name to match would have handed
    /// a benchmark of one enemy to a benchmark of the other -- two fields with
    /// quite different draw costs, told apart only by squinting at them. That
    /// was a live bug for exactly as long as the slime stood next to the
    /// scuttlebug; the ant that replaced the bug gave the three kinds three
    /// different first letters again, so the colliding table is supplied here
    /// rather than typed at the console, and the branch keeps its test after
    /// the collision it was written for went away.
    #[test]
    fn an_ambiguous_kind_is_refused_and_says_why() {
        let colliding = [
            ("slime", CrowdKind::Slime),
            ("scuttlebug", CrowdKind::Ant),
            ("mix", CrowdKind::Mix),
        ];
        let said = crowd_kind("s", &colliding).expect_err("an ambiguous prefix resolved anyway");
        assert!(
            said.contains("slime") && said.contains("scuttlebug"),
            "the ambiguity was not explained: {said:?}"
        );
        // Enough of it to tell them apart still works, both ways.
        assert_eq!(crowd_kind("sl", &colliding), Ok(CrowdKind::Slime));
        assert_eq!(crowd_kind("sc", &colliding), Ok(CrowdKind::Ant));

        // And with the names the game actually ships, one letter is enough for
        // every one of them -- which is the property a player relies on.
        for (name, kind) in CROWD_NAMES {
            assert_eq!(
                crowd_kind(&name[..1], &CROWD_NAMES),
                Ok(kind),
                "{name:?} is not reachable by its first letter"
            );
        }
    }

    /// A mistyped count must not queue anything, and a fat-fingered one must
    /// not take the machine down with it.
    #[test]
    fn a_bad_crowd_command_is_refused_and_a_huge_one_is_capped() {
        let mut console = ConsoleState::default();
        let mut tuning = GameTuning::default();
        console.execute("crowd", &mut tuning);
        console.execute("crowd lots", &mut tuning);
        console.execute("crowd 10 elephants", &mut tuning);
        assert!(console.take_requests().is_empty());
        console.execute("crowd 999999999", &mut tuning);
        assert_eq!(
            console.take_requests(),
            vec![Request::Crowd(CROWD_LIMIT, CrowdKind::Mix)]
        );
    }

    /// Types a line the way the character events do.
    fn type_line(console: &mut ConsoleState, line: &str) {
        for character in line.chars() {
            console.insert(character);
        }
    }

    /// The caret moves, and typing happens where it is. Without this the only
    /// way to fix the middle of a line is to delete back to it.
    #[test]
    fn the_caret_moves_and_text_lands_where_it_is() {
        let mut console = ConsoleState::default();
        type_line(&mut console, "cam_distance 12");
        for _ in 0..2 {
            console.step(-1);
        }
        type_line(&mut console, "9.");
        assert_eq!(console.input, "cam_distance 9.12");
        assert_eq!(console.split(), ("cam_distance 9.", "12"));
    }

    /// Backspace takes the character before the caret and Delete takes the one
    /// under it, and neither runs off the end of the line.
    #[test]
    fn backspace_and_delete_work_either_side_of_the_caret() {
        let mut console = ConsoleState::default();
        type_line(&mut console, "hero");
        console.step(-1);
        console.backspace();
        assert_eq!(console.input, "heo");
        console.delete();
        assert_eq!(console.input, "he");
        // The caret is at the end now: neither key has anything left to take.
        console.delete();
        assert_eq!(console.input, "he");
        console.step(-1);
        console.step(-1);
        console.step(-1);
        console.backspace();
        assert_eq!(console.input, "he");
        assert_eq!(console.split(), ("", "he"));
    }

    /// Completing works on the word the caret is in, not on the end of the
    /// line, and leaves the caret after what it inserted.
    #[test]
    fn completion_follows_the_caret() {
        let mut console = ConsoleState::default();
        type_line(&mut console, "cam_h 3");
        for _ in 0..2 {
            console.step(-1);
        }
        console.complete();
        assert_eq!(console.input, "cam_height 3");
        assert_eq!(console.split(), ("cam_height", " 3"));
    }

    /// A recalled command is ready to be edited at its end rather than with the
    /// caret left wherever the last line happened to put it -- which, on a
    /// shorter line, would be in the middle of this one.
    #[test]
    fn recalling_a_command_puts_the_caret_at_the_end() {
        let mut console = ConsoleState::default();
        type_line(&mut console, "hi");
        console.step(-1);
        console.set_input("cam_distance 12".into());
        assert_eq!(console.split(), ("cam_distance 12", ""));
    }

    /// A held key repeats, and a slow frame delivers every repeat it covered
    /// rather than one. Tapping to the start of a long line is what this
    /// spares.
    #[test]
    fn a_held_key_repeats_after_a_pause() {
        let mut repeat = KeyRepeat::default();
        let mut keys = ButtonInput::<KeyCode>::default();
        keys.press(KeyCode::ArrowLeft);
        assert_eq!(repeat.poll(&keys, 0.016), (Some(KeyCode::ArrowLeft), 1));
        keys.clear();
        // Held, but not yet long enough to start repeating.
        let mut moves = 0;
        for _ in 0..20 {
            moves += repeat.poll(&keys, 0.016).1;
        }
        assert_eq!(moves, 0, "it repeated before the delay was up");
        // Past the delay, and a long frame owes every repeat it covered.
        assert!(repeat.poll(&keys, 0.2,).1 >= 1);
        let caught_up = repeat.poll(&keys, 0.2).1;
        assert!(
            caught_up >= 4,
            "a 200 ms frame delivered {caught_up} repeats at one every 40 ms"
        );
        keys.release(KeyCode::ArrowLeft);
        assert_eq!(repeat.poll(&keys, 0.016), (None, 0));
    }

    #[test]
    fn console_ui_bundles_spawn_without_duplicate_components() {
        let mut world = World::new();
        world.spawn(panel_bundle());
        world.spawn(tuning_tray_bundle());
    }
}
