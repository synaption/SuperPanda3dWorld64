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
//! An event also either carries the point in the world it happened at or it
//! does not, and that is the whole of the spatial half: the player's own noises
//! are unplaced and play flat, and everything the rest of the field makes is
//! heard from where it was made. See [`SoundEvent`], and [`EAR_GAP`] and
//! [`PAN_RADIUS`] for the shape of the panning and why it is that shape.
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
    /// A gun fired. Shared: it is the weapon that makes this one, not the
    /// character holding it.
    Shoot,
    /// A weapon changed hands.
    Draw,
}

/// One event's samples: a list of layers that play together, each layer a list
/// of interchangeable takes to choose between. Paths are relative to
/// `assets/sounds`.
pub type Layers = &'static [&'static [&'static str]];

// Luna speaks with the Zelda voice set and lands and steps with its
// effect set, exactly as `sm64py.audio` pairs them.
const LUNA_JUMP: Layers = &[&["vc_zelda/vc_zelda_jump01.wav"]];
const LUNA_LAND: Layers = &[&[
    "se_zelda/se_zelda_landing01.wav",
    "se_zelda/se_zelda_landing02.wav",
]];
const LUNA_STEP: Layers = &[&[
    "se_zelda/se_zelda_step_left_m.wav",
    "se_zelda/se_zelda_step_right_m.wav",
]];
// Two layers: the blade through the air, and the shout over it.
const LUNA_ATTACK: Layers = &[
    &["se_zelda/se_zelda_swing_s.wav"],
    &[
        "vc_zelda/vc_zelda_attack01.wav",
        "vc_zelda/vc_zelda_attack02.wav",
        "vc_zelda/vc_zelda_attack03.wav",
    ],
];
const LUNA_HURT: Layers = &[&[
    "vc_zelda/vc_zelda_damage01.wav",
    "vc_zelda/vc_zelda_damage02.wav",
]];

// Mario's samples come out of an extracted asset tree via
// `tools/import_sounds.py`, because the decomp ships a sound taxonomy and no
// waveforms of its own. Grass is the castle grounds' terrain, so these are the
// grass takes.
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
// Placeholders, and knowingly so: there is no gunshot anywhere in either sound
// set, because neither game this port borrows from has a gun in it. The smash
// is the sharpest transient available and the appeal is the nearest thing to a
// weapon being readied. Both are here so that firing and swapping are *audible*
// while the pistol is being tuned; replace them when the real samples exist.
const SHOOT: Layers = &[&[
    "se_zelda/se_zelda_smash_S01.wav",
    "se_zelda/se_zelda_smash_S02.wav",
]];
const DRAW: Layers = &[&["se_zelda/se_zelda_appeal_S01.wav"]];

/// The samples an event plays for a character.
pub fn layers(character: ActiveCharacter, sfx: Sfx) -> Layers {
    match (sfx, character) {
        (Sfx::Defeat, _) => DEFEAT,
        (Sfx::Shoot, _) => SHOOT,
        (Sfx::Draw, _) => DRAW,
        (Sfx::Splash, _) => SPLASH,
        (Sfx::Stroke, _) => STROKE,
        (Sfx::Jump, ActiveCharacter::Luna) => LUNA_JUMP,
        (Sfx::Land, ActiveCharacter::Luna) => LUNA_LAND,
        (Sfx::Step, ActiveCharacter::Luna) => LUNA_STEP,
        (Sfx::Attack, ActiveCharacter::Luna) => LUNA_ATTACK,
        (Sfx::Hurt, ActiveCharacter::Luna) => LUNA_HURT,
        (Sfx::Jump, ActiveCharacter::Mario) => MARIO_JUMP,
        (Sfx::Land, ActiveCharacter::Mario) => MARIO_LAND,
        (Sfx::Step, ActiveCharacter::Mario) => MARIO_STEP,
        (Sfx::Attack, ActiveCharacter::Mario) => MARIO_ATTACK,
        (Sfx::Hurt, ActiveCharacter::Mario) => MARIO_HURT,
    }
}

/// Every sample the game can play, for preloading and for the file check.
pub fn all_paths() -> impl Iterator<Item = &'static str> {
    const EVENTS: [Sfx; 10] = [
        Sfx::Jump,
        Sfx::Land,
        Sfx::Step,
        Sfx::Attack,
        Sfx::Hurt,
        Sfx::Defeat,
        Sfx::Splash,
        Sfx::Stroke,
        Sfx::Shoot,
        Sfx::Draw,
    ];
    [ActiveCharacter::Luna, ActiveCharacter::Mario]
        .into_iter()
        .flat_map(|character| EVENTS.iter().map(move |sfx| layers(character, *sfx)))
        .flatten()
        .flat_map(|layer| layer.iter().copied())
}

/// How many pending events are kept. Sounds are queued at 30 Hz and drained
/// every rendered frame, so this only ever fills if the window stops drawing;
/// dropping the overflow is better than replaying a backlog on return.
const QUEUE_LIMIT: usize = 32;

/// One queued event: what happened, and where.
///
/// The position is what makes a sound a *place* rather than a fact. A noise the
/// player himself makes is left unplaced -- he is what the camera is pointed at,
/// and panning his own footfalls off to one side only makes the view feel
/// crooked -- while a noise something else in the world makes carries the point
/// it happened at and is heard from there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundEvent {
    pub sfx: Sfx,
    /// Where in the world it happened, or `None` for the player's own noises.
    pub at: Option<Vec3>,
}

#[derive(Resource, Default)]
pub struct SoundQueue {
    events: Vec<SoundEvent>,
}

impl SoundQueue {
    /// Queues one of the player's own sounds. It plays flat: full volume, no
    /// side.
    pub fn push(&mut self, sfx: Sfx) {
        self.queue(SoundEvent { sfx, at: None });
    }

    /// Queues a sound something in the world made, at the point it made it.
    pub fn push_at(&mut self, sfx: Sfx, at: Vec3) {
        self.queue(SoundEvent { sfx, at: Some(at) });
    }

    fn queue(&mut self, event: SoundEvent) {
        if self.events.len() < QUEUE_LIMIT {
            self.events.push(event);
        }
    }

    /// Empties the queue, handing back what was in it.
    pub fn drain(&mut self) -> Vec<SoundEvent> {
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

/// The pitch spread applied to every take, matching `LUNA_PITCH_MIN/MAX`.
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

/// How far apart the listener's ears are, in metres.
///
/// Wider than a head, and deliberately so: it is the mixer underneath that
/// wants it. A spatial sound reaches rodio, which gives each channel a gain of
/// `1/distance²` to that ear -- clamped to 1 -- times a second term that *rises*
/// with how far away that ear is. With ears a head's width apart the two
/// distances barely differ, the second term wins, and the sound leans towards
/// the wrong side. Ears far enough apart that the near one falls inside the
/// clamp and the far one does not put the panning back the way round it
/// belongs. This gap and [`PAN_RADIUS`] are chosen together to do that: a sound
/// hard to one side comes out about three to one in favour of that side, and one
/// dead ahead comes out even.
pub const EAR_GAP: f32 = 2.6;

/// How far from the listener a placed sound is actually put, in metres.
///
/// Direction and distance are carried separately. The emitter goes on the
/// bearing to whatever made the noise but always at this fixed radius, which is
/// what keeps the panning the same shape whether a slime died four metres away
/// or forty; how far it was is carried entirely by [`attenuation`]. Placing the
/// emitter at its true distance instead would hand the panning back to the
/// `1/distance²` term above, which flattens to nothing a few metres out.
pub const PAN_RADIUS: f32 = 0.8;

/// Makes up for what the panning costs.
///
/// The two channel gains at [`PAN_RADIUS`] run from 0.32 apiece for a sound
/// dead ahead to 0.60/0.21 for one hard to one side, so a placed sound is much
/// quieter than a flat one at the same volume. This is the multiplier that puts
/// them back on comparable footing: an on-axis sound lands about three decibels
/// under a flat one, and the loud channel of a hard-panned sound stays just
/// under unity at the default `sfx_volume`, which is what keeps a kill right
/// beside the ear from clipping.
pub const PAN_GAIN: f32 = 2.2;

/// The quietest a placed sound may be and still be worth a voice. A crowd dying
/// half a field away is dozens of samples nobody can hear, and each of them is
/// a sink, a decoder and an entity.
const CULL_GAIN: f32 = 0.08;

/// How loud a sound `distance` metres off plays, as a fraction of the same
/// sound at the listener.
///
/// Flat inside `range` and inverse-*linear* past it -- half as loud each
/// doubling, rather than the quarter inverse-square would give. Inverse-square
/// is the honest physics and the wrong choice here: the field is forty metres
/// across and a slime dying at the far end of it would be four percent of one
/// dying at your feet, which is silence with extra steps.
pub fn attenuation(distance: f32, range: f32) -> f32 {
    // A range of zero would divide by it, and the console can be typed into.
    let range = range.max(0.1);
    if distance <= range {
        1.0
    } else {
        range / distance
    }
}

/// Whether a sound that far off is worth playing at all. See [`CULL_GAIN`].
pub fn audible(distance: f32, range: f32) -> bool {
    attenuation(distance, range) >= CULL_GAIN
}

/// Where to put the emitter for a sound that happened at `at`: on the bearing
/// from the listener to it, [`PAN_RADIUS`] out, in the listener's own space.
///
/// In the listener's space rather than the world's because the emitter is
/// parented to the ears. A sound that outlives the frame it started on then
/// keeps the side it started on while the player runs and turns, instead of
/// sweeping across the stereo field as the camera leaves a point less than a
/// metre away behind. These are one-shots of well under a second; where they
/// came from is settled when they start.
pub fn pan_offset(listener: &GlobalTransform, at: Vec3) -> Vec3 {
    listener
        .affine()
        .inverse()
        .transform_point3(at)
        .normalize_or_zero()
        * PAN_RADIUS
}

#[cfg(any(feature = "sound", windows))]
pub use backend::{listener, play, preload};

/// Playback against Bevy's audio device. Present only where the `bevy_audio`
/// backend is compiled in; see the `sound` feature in Cargo.toml.
#[cfg(any(feature = "sound", windows))]
mod backend {
    use super::{
        attenuation, audible, pan_offset, takes, Rng, SoundQueue, EAR_GAP, PAN_GAIN, PITCH_SPREAD,
    };
    use crate::{console::GameTuning, GameState};
    use bevy::{
        audio::{PlaybackMode, SpatialListener, Volume},
        platform::collections::HashMap,
        prelude::*,
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

    /// The ears. Goes on whatever hears the world -- the camera -- and is an
    /// empty bundle in a build with no audio backend, so the caller names it
    /// unconditionally. See [`EAR_GAP`] for why the ears are as far apart as
    /// they are.
    pub fn listener() -> impl Bundle {
        SpatialListener::new(EAR_GAP)
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
        listener: Query<(Entity, &GlobalTransform), With<SpatialListener>>,
        mut rng: Local<Rng>,
    ) {
        let ears = listener.iter().next();
        for event in queue.drain() {
            // Where it is heard from, if it is heard from anywhere. An event
            // that carries a position, in a run that has ears to hear it with,
            // becomes an emitter hung off those ears; everything else -- the
            // player's own noises, and every sound in a headless run, which has
            // a queue and no camera -- plays flat.
            let placed = match (event.at, ears) {
                (Some(at), Some((entity, listener))) => {
                    let distance = at.distance(listener.translation());
                    if !audible(distance, tuning.sfx_range) {
                        continue;
                    }
                    Some((
                        entity,
                        pan_offset(listener, at),
                        attenuation(distance, tuning.sfx_range),
                    ))
                }
                _ => None,
            };
            for path in takes(state.active, event.sfx, &mut rng) {
                let Some(source) = bank.samples.get(path) else {
                    continue;
                };
                // Two components rather than one bundle, and a volume that
                // says which scale it is on: `Linear` is the plain multiplier
                // the old relative volume was, as against the decibel form the
                // same type now also offers.
                let sound = commands
                    .spawn((
                        AudioPlayer(source.clone()),
                        PlaybackSettings {
                            mode: PlaybackMode::Despawn,
                            volume: Volume::Linear(match placed {
                                Some((_, _, fade)) => tuning.sfx_volume * fade * PAN_GAIN,
                                None => tuning.sfx_volume,
                            }),
                            speed: 1.0 + rng.signed() * PITCH_SPREAD,
                            spatial: placed.is_some(),
                            ..default()
                        },
                    ))
                    .id();
                if let Some((ears, offset, _)) = placed {
                    // Parented, so the offset is read in the listener's space
                    // and the pan holds while the camera moves under it. The
                    // transform has to be there before the sink is built, which
                    // is why it goes on here rather than a frame later.
                    commands
                        .entity(sound)
                        .insert((Transform::from_translation(offset), ChildOf(ears)));
                }
            }
        }
    }
}

/// The no-device build: the queue is still drained, so gameplay behaves the
/// same and nothing accumulates.
#[cfg(not(any(feature = "sound", windows)))]
pub fn preload(_commands: &mut Commands, _assets: &AssetServer) {}

/// No backend, no ears: nothing to put on the camera.
#[cfg(not(any(feature = "sound", windows)))]
pub fn listener() -> impl Bundle {}

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
    fn a_placed_event_carries_the_point_it_happened_at() {
        let mut queue = SoundQueue::default();
        queue.push(Sfx::Jump);
        queue.push_at(Sfx::Defeat, Vec3::new(3.0, 0.0, -9.0));
        assert_eq!(
            queue.drain(),
            vec![
                SoundEvent {
                    sfx: Sfx::Jump,
                    at: None
                },
                SoundEvent {
                    sfx: Sfx::Defeat,
                    at: Some(Vec3::new(3.0, 0.0, -9.0))
                },
            ]
        );
    }

    #[test]
    fn a_sound_holds_its_volume_inside_the_range_and_halves_each_doubling_past_it() {
        assert_eq!(attenuation(0.0, 12.0), 1.0);
        assert_eq!(attenuation(12.0, 12.0), 1.0);
        assert!((attenuation(24.0, 12.0) - 0.5).abs() < 1e-6);
        assert!((attenuation(48.0, 12.0) - 0.25).abs() < 1e-6);
        // The console can be typed into, and a range of zero divides by itself.
        assert!(attenuation(5.0, 0.0).is_finite());
    }

    #[test]
    fn a_sound_far_enough_off_is_not_given_a_voice_at_all() {
        // A crowd dying across the field is dozens of samples nobody can hear.
        assert!(audible(12.0, 12.0));
        assert!(audible(100.0, 12.0));
        assert!(!audible(200.0, 12.0));
    }

    #[test]
    fn which_side_a_sound_is_heard_on_is_the_bearing_to_it() {
        // Bevy's camera looks down -Z with +X to its right, which is the side
        // `SpatialListener` puts the right ear on.
        let listener = GlobalTransform::from(Transform::from_xyz(0.0, 2.0, 0.0));
        let right = pan_offset(&listener, Vec3::new(20.0, 2.0, 0.0));
        assert!((right.x - PAN_RADIUS).abs() < 1e-4, "{right:?}");
        let left = pan_offset(&listener, Vec3::new(-20.0, 2.0, 0.0));
        assert!((left.x + PAN_RADIUS).abs() < 1e-4, "{left:?}");

        // Turning to face the same sound puts it dead ahead instead -- down the
        // listener's own -Z, even in both ears.
        let turned = GlobalTransform::from(
            Transform::from_xyz(0.0, 2.0, 0.0).looking_at(Vec3::new(20.0, 2.0, 0.0), Vec3::Y),
        );
        let ahead = pan_offset(&turned, Vec3::new(20.0, 2.0, 0.0));
        assert!(ahead.x.abs() < 1e-4 && ahead.z < 0.0, "{ahead:?}");

        // Distance never reaches the offset: it is carried by volume, and the
        // emitter sits at one fixed radius however far off the sound was.
        for far in [4.0, 40.0, 400.0] {
            let offset = pan_offset(&listener, Vec3::new(far, 2.0, -far));
            assert!((offset.length() - PAN_RADIUS).abs() < 1e-4, "{offset:?}");
        }

        // A sound on top of the listener has no side, and normalising it must
        // not hand back a NaN.
        assert_eq!(pan_offset(&listener, Vec3::new(0.0, 2.0, 0.0)), Vec3::ZERO);
    }

    #[test]
    fn every_event_resolves_to_at_least_one_take() {
        let mut rng = Rng::default();
        for character in [ActiveCharacter::Luna, ActiveCharacter::Mario] {
            for sfx in [
                Sfx::Jump,
                Sfx::Land,
                Sfx::Step,
                Sfx::Attack,
                Sfx::Hurt,
                Sfx::Defeat,
                Sfx::Splash,
                Sfx::Stroke,
                Sfx::Shoot,
                Sfx::Draw,
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
        // Luna's voice would substitute one character's shout for the other's.
        for sfx in [Sfx::Jump, Sfx::Attack, Sfx::Hurt, Sfx::Step, Sfx::Land] {
            for path in layers(ActiveCharacter::Mario, sfx)
                .iter()
                .flat_map(|l| l.iter())
            {
                assert!(path.starts_with("mario64/"), "Mario plays {path}");
            }
            for path in layers(ActiveCharacter::Luna, sfx)
                .iter()
                .flat_map(|l| l.iter())
            {
                assert!(
                    path.starts_with("se_zelda/") || path.starts_with("vc_zelda/"),
                    "Luna plays {path}"
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
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/sounds");
        for path in all_paths() {
            assert!(root.join(path).is_file(), "missing sample: sounds/{path}");
        }
    }
}
