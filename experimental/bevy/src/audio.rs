//! Sound events, and playing them through Bevy.
//!
//! The split follows `sm64py/audio.py`: gameplay never touches the audio
//! device. The fixed-step systems append typed events to [`SoundQueue`] and a
//! render-rate system drains it, so the simulation runs identically with no
//! audio device present -- which matters, because under WSL there often is
//! none -- and stays testable headless.
//!
//! Each event resolves to a stack of *layers*, and one variant of each layer
//! plays together. That is what SM64 itself does for a jump: a terrain sound
//! from the ground and a voice from Mario, two samples on one event.
//!
//! The tables below and the queue are always compiled and tested. Only the
//! playback backend is conditional -- see the `sound` feature in Cargo.toml --
//! and where it is absent the queue is still drained, so a build without an
//! audio device behaves like one whose device is muted rather than one that
//! slowly fills a buffer nobody reads.

// Without a playback backend the sample tables are read only by the tests,
// and the compiler is right that nothing else touches them. That is the
// intended shape of a no-device build rather than a finding, so the warning
// is silenced there and left on everywhere else.
#![cfg_attr(not(any(feature = "sound", windows)), allow(dead_code))]

use crate::ActiveCharacter;
use bevy::prelude::*;

/// The gameplay events that make a noise. Kept small and behavioural rather
/// than mirroring the decomp's 467 packed IDs: the port raises the events its
/// own action set can actually reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sfx {
    Jump,
    Land,
    Step,
    Attack,
    Hurt,
    /// An enemy was defeated. Shared: it is the enemy that makes this one.
    Defeat,
    /// Breaking the water surface, in either direction.
    Splash,
    /// One swimming stroke.
    Stroke,
}

/// One event's samples: a list of layers that play together, each layer a list
/// of interchangeable takes to choose between. Paths are relative to
/// `assets/sounds`.
pub type Layers = &'static [&'static [&'static str]];

// The Hero speaks with the Zelda voice set and lands and steps with its
// effect set, exactly as `sm64py.audio` pairs them.
const HERO_JUMP: Layers = &[&["vc_zelda/vc_zelda_jump01.wav"]];
const HERO_LAND: Layers = &[&[
    "se_zelda/se_zelda_landing01.wav",
    "se_zelda/se_zelda_landing02.wav",
]];
const HERO_STEP: Layers = &[&[
    "se_zelda/se_zelda_step_left_m.wav",
    "se_zelda/se_zelda_step_right_m.wav",
]];
// Two layers: the blade through the air, and the shout over it.
const HERO_ATTACK: Layers = &[
    &["se_zelda/se_zelda_swing_s.wav"],
    &[
        "vc_zelda/vc_zelda_attack01.wav",
        "vc_zelda/vc_zelda_attack02.wav",
        "vc_zelda/vc_zelda_attack03.wav",
    ],
];
const HERO_HURT: Layers = &[&[
    "vc_zelda/vc_zelda_damage01.wav",
    "vc_zelda/vc_zelda_damage02.wav",
]];

// Mario's samples are the placeholders `sm64py.audio` synthesises, because the
// decomp ships a sound taxonomy and no waveforms. Grass is the castle
// grounds' terrain, so these are the grass takes.
const MARIO_JUMP: Layers = &[
    &["mario64/jump_grass.wav"],
    &[
        "mario64/mario_yah_wah_hoo.wav",
        "mario64/mario_yahoo.wav",
        "mario64/mario_hoohoo.wav",
    ],
];
const MARIO_LAND: Layers = &[&["mario64/landing_grass.wav"]];
const MARIO_STEP: Layers = &[&["mario64/step_grass.wav"]];
const MARIO_ATTACK: Layers = &[&["mario64/mario_haha.wav"]];
const MARIO_HURT: Layers = &[&["mario64/mario_ooof.wav"]];

// Water belongs to the level rather than the character, and the enemy makes
// its own death noise, so these three are shared.
const SPLASH: Layers = &[&["mario64/water_plunge.wav"]];
const STROKE: Layers = &[&["mario64/swim_stroke.wav"]];
const DEFEAT: Layers = &[&["se_zelda/se_zelda_smash_L01.wav"]];

/// The samples an event plays for a character.
pub fn layers(character: ActiveCharacter, sfx: Sfx) -> Layers {
    match (sfx, character) {
        (Sfx::Defeat, _) => DEFEAT,
        (Sfx::Splash, _) => SPLASH,
        (Sfx::Stroke, _) => STROKE,
        (Sfx::Jump, ActiveCharacter::Hero) => HERO_JUMP,
        (Sfx::Land, ActiveCharacter::Hero) => HERO_LAND,
        (Sfx::Step, ActiveCharacter::Hero) => HERO_STEP,
        (Sfx::Attack, ActiveCharacter::Hero) => HERO_ATTACK,
        (Sfx::Hurt, ActiveCharacter::Hero) => HERO_HURT,
        (Sfx::Jump, ActiveCharacter::Mario) => MARIO_JUMP,
        (Sfx::Land, ActiveCharacter::Mario) => MARIO_LAND,
        (Sfx::Step, ActiveCharacter::Mario) => MARIO_STEP,
        (Sfx::Attack, ActiveCharacter::Mario) => MARIO_ATTACK,
        (Sfx::Hurt, ActiveCharacter::Mario) => MARIO_HURT,
    }
}

/// Every sample the game can play, for preloading and for the file check.
pub fn all_paths() -> impl Iterator<Item = &'static str> {
    const EVENTS: [Sfx; 8] = [
        Sfx::Jump,
        Sfx::Land,
        Sfx::Step,
        Sfx::Attack,
        Sfx::Hurt,
        Sfx::Defeat,
        Sfx::Splash,
        Sfx::Stroke,
    ];
    [ActiveCharacter::Hero, ActiveCharacter::Mario]
        .into_iter()
        .flat_map(|character| EVENTS.iter().map(move |sfx| layers(character, *sfx)))
        .flatten()
        .flat_map(|layer| layer.iter().copied())
}

/// How many pending events are kept. Sounds are queued at 30 Hz and drained
/// every rendered frame, so this only ever fills if the window stops drawing;
/// dropping the overflow is better than replaying a backlog on return.
const QUEUE_LIMIT: usize = 32;

#[derive(Resource, Default)]
pub struct SoundQueue {
    events: Vec<Sfx>,
}

impl SoundQueue {
    pub fn push(&mut self, sfx: Sfx) {
        if self.events.len() < QUEUE_LIMIT {
            self.events.push(sfx);
        }
    }

    /// Empties the queue, handing back what was in it.
    pub fn drain(&mut self) -> Vec<Sfx> {
        std::mem::take(&mut self.events)
    }
}

/// A tiny xorshift, so variant choice and pitch jitter cost no dependency and
/// stay reproducible when a test seeds them.
pub struct Rng(u32);

impl Default for Rng {
    fn default() -> Self {
        Self(0x9E37_79B9)
    }
}

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// An index into a list of takes.
    fn index(&mut self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            self.next() as usize % len
        }
    }

    /// A number in `-1..1`, used for the pitch jitter that keeps a repeated
    /// footfall from sounding like a loop.
    #[cfg_attr(not(any(feature = "sound", windows)), allow(dead_code))]
    fn signed(&mut self) -> f32 {
        (self.next() as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// The pitch spread applied to every take, matching `HERO_PITCH_MIN/MAX`.
const PITCH_SPREAD: f32 = 0.05;

/// Chooses one take from each layer of an event.
///
/// Separated from playback so the choice itself is testable without an audio
/// device: it is the part that can pick the wrong sample.
pub fn takes(character: ActiveCharacter, sfx: Sfx, rng: &mut Rng) -> Vec<&'static str> {
    layers(character, sfx)
        .iter()
        .filter(|layer| !layer.is_empty())
        .map(|layer| layer[rng.index(layer.len())])
        .collect()
}

#[cfg(any(feature = "sound", windows))]
pub use backend::{play, preload};

/// Playback against Bevy's audio device. Present only where the `bevy_audio`
/// backend is compiled in; see the `sound` feature in Cargo.toml.
#[cfg(any(feature = "sound", windows))]
mod backend {
    use super::{takes, Rng, SoundQueue, PITCH_SPREAD};
    use crate::{console::GameTuning, GameState};
    use bevy::{
        audio::{PlaybackMode, Volume},
        prelude::*,
        utils::HashMap,
    };

    /// Samples held loaded for the whole run. Loading is asynchronous, so a
    /// sample first requested when it is needed would arrive after the moment
    /// that wanted it; preloading at startup is what makes a footfall land on
    /// the foot.
    #[derive(Resource, Default)]
    pub struct SoundBank {
        samples: HashMap<&'static str, Handle<AudioSource>>,
    }

    /// Loads every sample the tables name.
    pub fn preload(commands: &mut Commands, assets: &AssetServer) {
        let samples = super::all_paths()
            .map(|path| (path, assets.load(format!("sounds/{path}"))))
            .collect();
        commands.insert_resource(SoundBank { samples });
    }

    /// Drains the queue and spawns one despawning audio entity per layer.
    ///
    /// Runs unconditionally rather than only while the console is closed, so
    /// events raised on the tick the console opened still play out instead of
    /// waiting, silent, for the console to close.
    pub fn play(
        mut commands: Commands,
        mut queue: ResMut<SoundQueue>,
        bank: Res<SoundBank>,
        state: Res<GameState>,
        tuning: Res<GameTuning>,
        mut rng: Local<Rng>,
    ) {
        for sfx in queue.drain() {
            for path in takes(state.active, sfx, &mut rng) {
                let Some(source) = bank.samples.get(path) else {
                    continue;
                };
                commands.spawn(AudioBundle {
                    source: source.clone(),
                    settings: PlaybackSettings {
                        mode: PlaybackMode::Despawn,
                        volume: Volume::new_relative(tuning.sfx_volume),
                        speed: 1.0 + rng.signed() * PITCH_SPREAD,
                        ..default()
                    },
                });
            }
        }
    }
}

/// The no-device build: the queue is still drained, so gameplay behaves the
/// same and nothing accumulates.
#[cfg(not(any(feature = "sound", windows)))]
pub fn preload(_commands: &mut Commands, _assets: &AssetServer) {}

#[cfg(not(any(feature = "sound", windows)))]
pub fn play(mut queue: ResMut<SoundQueue>) {
    queue.drain();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_drops_events_past_its_limit_and_drains_empty() {
        let mut queue = SoundQueue::default();
        for _ in 0..QUEUE_LIMIT + 10 {
            queue.push(Sfx::Step);
        }
        assert_eq!(queue.drain().len(), QUEUE_LIMIT);
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn every_event_resolves_to_at_least_one_take() {
        let mut rng = Rng::default();
        for character in [ActiveCharacter::Hero, ActiveCharacter::Mario] {
            for sfx in [
                Sfx::Jump,
                Sfx::Land,
                Sfx::Step,
                Sfx::Attack,
                Sfx::Hurt,
                Sfx::Defeat,
                Sfx::Splash,
                Sfx::Stroke,
            ] {
                let takes = takes(character, sfx, &mut rng);
                assert!(!takes.is_empty(), "{character:?} {sfx:?} is silent");
            }
        }
    }

    #[test]
    fn a_jump_layers_terrain_under_voice_for_mario() {
        // Two layers means two samples at once, which is the SM64 behaviour
        // this table exists to reproduce.
        assert_eq!(layers(ActiveCharacter::Mario, Sfx::Jump).len(), 2);
        let mut rng = Rng::default();
        let takes = takes(ActiveCharacter::Mario, Sfx::Jump, &mut rng);
        assert_eq!(takes.len(), 2);
        assert!(takes[0].contains("jump_grass"), "{takes:?}");
        assert!(takes[1].contains("mario_"), "{takes:?}");
    }

    #[test]
    fn the_two_characters_never_share_a_voice() {
        // One event queue serves both, so a table that pointed Mario at the
        // Hero's voice would substitute one character's shout for the other's.
        for sfx in [Sfx::Jump, Sfx::Attack, Sfx::Hurt, Sfx::Step, Sfx::Land] {
            for path in layers(ActiveCharacter::Mario, sfx)
                .iter()
                .flat_map(|l| l.iter())
            {
                assert!(path.starts_with("mario64/"), "Mario plays {path}");
            }
            for path in layers(ActiveCharacter::Hero, sfx)
                .iter()
                .flat_map(|l| l.iter())
            {
                assert!(
                    path.starts_with("se_zelda/") || path.starts_with("vc_zelda/"),
                    "the Hero plays {path}"
                );
            }
        }
    }

    #[test]
    fn rng_index_stays_in_range_and_varies() {
        let mut rng = Rng::default();
        let mut seen = [false; 3];
        for _ in 0..64 {
            let index = rng.index(3);
            assert!(index < 3);
            seen[index] = true;
        }
        assert!(seen.iter().all(|hit| *hit), "one variant never came up");
        assert_eq!(rng.index(0), 0, "an empty layer must not divide by zero");
    }

    #[test]
    fn pitch_jitter_stays_within_the_spread() {
        let mut rng = Rng::default();
        for _ in 0..256 {
            let speed = 1.0 + rng.signed() * PITCH_SPREAD;
            assert!((0.95..=1.05).contains(&speed), "{speed}");
        }
    }

    /// The tables name files by hand, so this is what catches a typo or a
    /// sample that was renamed out from under them.
    #[test]
    fn every_named_sample_exists_in_the_repository() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/sounds");
        for path in all_paths() {
            assert!(root.join(path).is_file(), "missing sample: sounds/{path}");
        }
    }
}
