//! Which clip each character plays, and how fast.
//!
//! Ported from `sm64py/hero/animations.py` and `sm64py/mario/animations.py`.
//! Clips are looked up **by name** rather than by export index: the Hero's
//! names are the Blender action names, spaces, capitals, trailing space and
//! all -- `Idle ` really does end in a space -- and Mario's are `anim_XX`
//! after the decomp's `MARIO_ANIM_*` hex id. Names are the source of truth in
//! both exporters, and an index silently shifts the moment a clip is added.
//!
//! Three things beyond clip choice come across with it: clips that play once
//! and hold their last pose rather than cycling, a walk whose playback rate
//! tracks how fast the ground is going by so the feet do not slide, and an
//! idle fidget that replaces the idle after standing still for a while.

use crate::{
    player::{Controller, Motion},
    ActiveCharacter,
};
use bevy::{
    animation::RepeatAnimation, gltf::Gltf, platform::collections::HashMap, prelude::*,
};

// -- the Hero's clips, verbatim out of Blender ------------------------------
const IDLE: &str = "Idle ";
const IDLE_VAR: &str = "idle var";
const WALK: &str = "walk Var1";
const RUN: &str = "Running normal";
const JUMP_RISE: &str = "jump up";
const JUMP_FALL: &str = "jump down";
const LAND: &str = "jump Impact";
const ATTACK1: &str = "Attack 1 beta";
const ATTACK2: &str = "Attack 2";

// -- Mario's clips, named after the decomp's animation ids ------------------
const M_IDLE: &str = "anim_C5"; // IDLE_HEAD_CENTER
/// SM64 keeps three idles apart by where the head is looking, so Mario's
/// fidget is the head turning rather than a separate performance. It has to be
/// one of those: `anim_0E` is A_POSE, which the decomp uses as the fallback
/// for an action it has no clip for, and a Mario standing in it looks exactly
/// like the bug it normally means.
const M_IDLE_VAR: &str = "anim_C4"; // IDLE_HEAD_RIGHT
const M_WALK: &str = "anim_48"; // WALKING
const M_RUN: &str = "anim_72"; // RUNNING
const M_JUMP: &str = "anim_4D"; // SINGLE_JUMP
const M_FALL: &str = "anim_56"; // GENERAL_FALL
const M_LAND: &str = "anim_57"; // GENERAL_LAND
const M_PUNCH1: &str = "anim_67"; // FIRST_PUNCH
const M_PUNCH2: &str = "anim_68"; // SECOND_PUNCH
const M_SWIM: &str = "anim_AC"; // FLUTTERKICK
const M_WATER_IDLE: &str = "anim_B2"; // WATER_IDLE
const M_SKATE: &str = "anim_skate_stride"; // authored, not from the decomp

/// Clips that play once and hold their last pose. Everything else cycles.
const NON_LOOPING: [&str; 9] = [
    JUMP_RISE, LAND, ATTACK1, ATTACK2, IDLE_VAR, M_IDLE_VAR, M_LAND, M_PUNCH1, M_PUNCH2,
];

/// How long a clip takes to blend into the one before it.
const TRANSITION: std::time::Duration = std::time::Duration::from_millis(120);

/// How long a character stands still before the fidget plays instead, in
/// seconds. The Panda3D build counts 240 frames at 30 Hz; long enough that it
/// reads as boredom rather than as a twitch.
const IDLE_VAR_AFTER: f32 = 240.0 / 30.0;

/// Ground speed above which the walk becomes a run.
const RUN_SPEED: f32 = 6.0;

/// Divisor turning ground speed into a walk playback rate, so one cycle of the
/// clip plays in exactly the time one stride covers and the feet stay planted.
/// Measured per character: Mario's walk is 77 frames of a cartoon stride, the
/// Hero's 40 frames of a human one, so they cannot share a number.
const HERO_WALK_DIVISOR: f32 = 4.11;
const MARIO_WALK_DIVISOR: f32 = 3.30;

/// The ground speed at which Mario's walk plays at its authored rate, and so
/// the speed at which his feet do not slide at all. Anyone moving him for
/// reasons of their own -- the allies ambling around the lawn, for one -- can
/// pick this and get a walk that looks right for free.
pub const MARIO_STRIDE_SPEED: f32 = MARIO_WALK_DIVISOR;

/// Rates outside this range are compressed and then clamped, so a tuning
/// change to running speed cannot turn the legs into a blur or a slideshow.
const MIN_STRIDE_SPEED: f32 = 2.0;
const MIN_PLAY_RATE: f32 = 1.0 / 16.0;
const MAX_PLAY_RATE: f32 = 1.6;
const SLIDE_COMPRESSION: f32 = 0.25;

/// The clip names each character can play, resolved once its glTF has loaded.
///
/// A player no longer takes a clip handle. It takes a *node index* into an
/// `AnimationGraph` asset it has been given, so the name tables resolve to
/// indices and each character carries the one graph every clip of his was
/// added to. Selection is still by name -- what changed underneath is only
/// what a name resolves to.
#[derive(Resource, Default)]
pub struct CharacterAnimations {
    hero: HashMap<String, AnimationNodeIndex>,
    mario: HashMap<String, AnimationNodeIndex>,
    hero_graph: Handle<AnimationGraph>,
    mario_graph: Handle<AnimationGraph>,
    hero_source: Handle<Gltf>,
    mario_source: Handle<Gltf>,
}

#[derive(Component)]
pub struct AnimationOwner(pub ActiveCharacter);

/// One graph per enemy *clip*, not per enemy.
///
/// Every player needs a graph before an index into one means anything, and an
/// enemy's whole animation is a single looping clip. Building one graph each
/// would put an asset in the table for every slime on the field, and the field
/// cap goes to five thousand; keying by the clip the kind loads means two
/// graphs however many enemies are alive, because `AssetServer::load` hands
/// back the same handle for the same path.
#[derive(Resource, Default)]
pub struct EnemyGraphs(HashMap<AssetId<AnimationClip>, (Handle<AnimationGraph>, AnimationNodeIndex)>);

impl EnemyGraphs {
    fn get_or_add(
        &mut self,
        clip: &Handle<AnimationClip>,
        graphs: &mut Assets<AnimationGraph>,
    ) -> (Handle<AnimationGraph>, AnimationNodeIndex) {
        self.0
            .entry(clip.id())
            .or_insert_with(|| {
                let (graph, node) = AnimationGraph::from_clip(clip.clone());
                (graphs.add(graph), node)
            })
            .clone()
    }
}

/// An animation player inside an ally's model, and the ally it belongs to.
///
/// Allies are Marios and carry the same [`ActiveCharacter`] the playable one
/// does, so without this marker the player's clip would be pushed onto every
/// ally in the field the moment the player switched to Mario.
#[derive(Component)]
pub struct AllyAnimationRoot(pub Entity);

impl CharacterAnimations {
    pub fn load(assets: &AssetServer) -> Self {
        Self {
            hero_source: assets.load("hero/hero.glb"),
            mario_source: assets.load("mario/mario.glb"),
            ..default()
        }
    }

    fn clips(&self, character: ActiveCharacter) -> &HashMap<String, AnimationNodeIndex> {
        match character {
            ActiveCharacter::Hero => &self.hero,
            ActiveCharacter::Mario => &self.mario,
        }
    }

    /// The graph node for a clip name, if that character's glTF carries one.
    pub fn named(&self, character: ActiveCharacter, name: &str) -> Option<AnimationNodeIndex> {
        self.clips(character).get(name).copied()
    }

    /// The graph a player must be given before any of those indices mean
    /// anything to it. An index is only a position in *this* graph.
    pub fn graph(&self, character: ActiveCharacter) -> Handle<AnimationGraph> {
        match character {
            ActiveCharacter::Hero => self.hero_graph.clone(),
            ActiveCharacter::Mario => self.mario_graph.clone(),
        }
    }

    pub fn ready(&self, character: ActiveCharacter) -> bool {
        !self.clips(character).is_empty()
    }
}

/// Copies the name-to-clip tables out of each glTF once it has loaded.
///
/// Bevy exposes named animations only on the whole `Gltf` asset, so the file
/// is loaded a second time as a `Gltf` alongside the `#Scene0` the characters
/// are spawned from. The asset server hands back the same parsed file, so this
/// costs a lookup rather than a second parse.
pub fn resolve_clips(
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut animations: ResMut<CharacterAnimations>,
) {
    if animations.hero.is_empty() {
        if let Some(gltf) = gltfs.get(&animations.hero_source) {
            let (table, graph) = graph_of(gltf, &mut graphs);
            animations.hero = table;
            animations.hero_graph = graph;
        }
    }
    if animations.mario.is_empty() {
        if let Some(gltf) = gltfs.get(&animations.mario_source) {
            let (table, graph) = graph_of(gltf, &mut graphs);
            animations.mario = table;
            animations.mario_graph = graph;
        }
    }
}

/// Puts every named clip of one glTF into a single flat graph, and hands back
/// the name-to-node table beside it.
///
/// Flat -- every clip a direct child of the root -- because this port picks one
/// clip at a time and blends between them through `AnimationTransitions`. The
/// graph's own blend weights would be a second, competing way to do that.
fn graph_of(
    gltf: &Gltf,
    graphs: &mut Assets<AnimationGraph>,
) -> (HashMap<String, AnimationNodeIndex>, Handle<AnimationGraph>) {
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let table = gltf
        .named_animations
        .iter()
        .map(|(name, clip)| (name.to_string(), graph.add_clip(clip.clone(), 1.0, root)))
        .collect();
    (table, graphs.add(graph))
}

/// The clip a character plays for the state it is in, and the rate to play it
/// at.
///
/// Split out from the systems so the whole table is exercised without a
/// renderer: this is the part that can point a character at the wrong clip.
pub fn resolve(character: ActiveCharacter, state: &AnimationState) -> (&'static str, f32) {
    let hero = character == ActiveCharacter::Hero;
    let clip = match state.motion {
        Motion::Idle if state.still_for > IDLE_VAR_AFTER => {
            if hero {
                IDLE_VAR
            } else {
                M_IDLE_VAR
            }
        }
        Motion::Idle => {
            if hero {
                IDLE
            } else {
                M_IDLE
            }
        }
        // Walk and run are one state in the port, chosen by speed here the
        // same way both source tables choose theirs.
        Motion::Run if state.speed > RUN_SPEED => {
            if hero {
                RUN
            } else {
                M_RUN
            }
        }
        Motion::Run => {
            if hero {
                WALK
            } else {
                M_WALK
            }
        }
        Motion::Jump => {
            if hero {
                JUMP_RISE
            } else {
                M_JUMP
            }
        }
        Motion::Fall => {
            if hero {
                JUMP_FALL
            } else {
                M_FALL
            }
        }
        Motion::Land => {
            if hero {
                LAND
            } else {
                M_LAND
            }
        }
        // There is no flying clip in either set, and inventing one would look
        // worse than this: the rising jump holds a body already off the ground
        // with its legs gathered, which is what thrust should look like, and
        // coming back down is any other descent.
        Motion::Fly => {
            if state.rising {
                if hero {
                    JUMP_RISE
                } else {
                    M_JUMP
                }
            } else if hero {
                JUMP_FALL
            } else {
                M_FALL
            }
        }
        // The Hero has no skate clip either, so he runs; the cadence below is
        // what sells it, since a skate is the one place where the ground is
        // meant to go by faster than the stride covers it.
        Motion::Skate => {
            if hero {
                RUN
            } else {
                M_SKATE
            }
        }
        Motion::Swim if state.speed > 0.5 => {
            if hero {
                WALK
            } else {
                M_SWIM
            }
        }
        Motion::Swim => {
            if hero {
                IDLE
            } else {
                M_WATER_IDLE
            }
        }
        // Attacks alternate, so holding the button reads as a combo rather
        // than as one swing played twice.
        Motion::Attack => match (hero, state.combo) {
            (true, 0) => ATTACK1,
            (true, _) => ATTACK2,
            (false, 0) => M_PUNCH1,
            (false, _) => M_PUNCH2,
        },
    };
    (clip, play_rate(character, clip, state))
}

/// Walk cycles are scaled by speed; everything else plays as authored.
fn play_rate(character: ActiveCharacter, clip: &str, state: &AnimationState) -> f32 {
    let divisor = match (character, clip) {
        (ActiveCharacter::Hero, WALK) => HERO_WALK_DIVISOR,
        (ActiveCharacter::Mario, M_WALK) => MARIO_WALK_DIVISOR,
        // Wading has no clip of its own and borrows the walk. Played slowly it
        // reads as pushing through water; tying it to speed as well would
        // leave it crawling almost to a stop.
        (_, _) if state.motion == Motion::Swim => return 0.6,
        _ => return 1.0,
    };
    // Floored, because a character sliding to a halt still has some speed and
    // legs that have all but stopped moving read as broken rather than as
    // slow. Low enough that an ordinary walking pace is still played at the
    // rate it covers ground at: above this the feet are planted.
    let ideal = state.speed.max(MIN_STRIDE_SPEED) / divisor;
    let ideal = if ideal > 1.0 {
        ideal.powf(SLIDE_COMPRESSION)
    } else {
        ideal
    };
    ideal.clamp(MIN_PLAY_RATE, MAX_PLAY_RATE)
}

fn loops(clip: &str) -> bool {
    !NON_LOOPING.contains(&clip)
}

/// Pushes a clip onto one animation player, if it is not already the one
/// running there.
///
/// Shared by the playable characters and by the allies so the looping rule
/// cannot drift apart between them: a clip that plays once -- a landing, a
/// swing, the idle fidget -- must not be told to repeat, or it cycles for as
/// long as the character stands there. `restart` replays a clip that is
/// already running, which is how a state re-entered onto the same clip is
/// seen at all.
/// The cross-fade between clips now lives in a component beside the player
/// rather than in a method on it, so the caller passes both.
pub fn apply(
    player: &mut AnimationPlayer,
    transitions: &mut AnimationTransitions,
    clip: AnimationNodeIndex,
    name: &str,
    rate: f32,
    restart: bool,
) {
    if restart || !player.is_playing_animation(clip) {
        transitions.play(player, clip, TRANSITION);
    }
    // Repeat and speed are per-*animation* now rather than settings on the
    // whole player, so they are re-applied to the running one every call
    // instead of once at the point it starts. That is not just bookkeeping:
    // the walk's rate tracks ground speed and has to keep following it while
    // the same clip goes on playing.
    if let Some(active) = player.animation_mut(clip) {
        active.set_repeat(if loops(name) {
            RepeatAnimation::Forever
        } else {
            RepeatAnimation::Never
        });
        active.set_speed(rate);
    }
}

/// What clip selection needs to know about a character, gathered once so the
/// table above can be resolved without a `World`.
#[derive(Clone, Copy, Debug)]
pub struct AnimationState {
    pub motion: Motion,
    pub speed: f32,
    pub rising: bool,
    /// Seconds spent standing still, for the idle fidget.
    pub still_for: f32,
    /// Which swing of the combo this is.
    pub combo: u8,
}

impl Default for AnimationState {
    fn default() -> Self {
        Self {
            motion: Motion::Idle,
            speed: 0.0,
            rising: false,
            still_for: 0.0,
            combo: 0,
        }
    }
}

/// Tracks how long the player has been idle and which swing is next.
#[derive(Resource, Default)]
pub struct PlayerAnimation {
    pub state: AnimationState,
    /// The clip currently playing, so a change is only pushed once.
    playing: Option<(ActiveCharacter, &'static str)>,
}

/// Attaches every animation player a spawned scene brought with it to whatever
/// owns it: an ally, an enemy, or one of the two playable characters.
#[allow(clippy::too_many_arguments)]
pub fn claim_players(
    mut commands: Commands,
    mut enemy_graphs: ResMut<EnemyGraphs>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    hierarchy: Query<&ChildOf>,
    characters: Query<&ActiveCharacter>,
    allies: Query<&crate::squad::Ally>,
    enemies: Query<&crate::enemy::Enemy>,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    for (entity, mut player) in &mut players {
        let mut ancestor = entity;
        loop {
            // Allies are checked before characters: an ally carries Mario's
            // own marker, and the more specific owner has to win.
            if allies.contains(ancestor) {
                commands.entity(entity).insert((
                    AnimationOwner(ActiveCharacter::Mario),
                    AllyAnimationRoot(ancestor),
                    // The cross-fade between clips is this component's job now.
                    AnimationTransitions::new(),
                ));
                break;
            }
            if let Ok(enemy) = enemies.get(ancestor) {
                let (graph, node) = enemy_graphs.get_or_add(&enemy.animation, &mut graphs);
                player.play(node).repeat();
                commands
                    .entity(entity)
                    .insert((
                        crate::enemy::EnemyAnimationRoot {
                            owner: ancestor,
                            clip: node,
                        },
                        AnimationGraphHandle(graph),
                    ));
                break;
            }
            if let Ok(character) = characters.get(ancestor) {
                commands
                    .entity(entity)
                    .insert((AnimationOwner(*character), AnimationTransitions::new()));
                break;
            }
            let Ok(parent) = hierarchy.get(ancestor) else {
                break;
            };
            ancestor = parent.parent();
        }
    }
}

/// Hands each owned player the graph its clip indices are positions in.
///
/// Separate from [`claim_players`] rather than folded into it because the two
/// answer to different clocks: ownership is known the moment a scene spawns,
/// but the graph does not exist until that character's glTF has finished
/// loading and [`resolve_clips`] has built it. A player claimed on an earlier
/// frame than its graph was built is the normal case, not the exception, so
/// this keeps looking until there is something to give.
pub fn attach_graphs(
    mut commands: Commands,
    animations: Res<CharacterAnimations>,
    players: Query<(Entity, &AnimationOwner), Without<AnimationGraphHandle>>,
) {
    for (entity, owner) in &players {
        if !animations.ready(owner.0) {
            continue;
        }
        commands
            .entity(entity)
            .insert(AnimationGraphHandle(animations.graph(owner.0)));
    }
}

/// Gathers the player's state for clip selection, including the timers that
/// only animation cares about.
pub fn track_player(
    time: Res<Time>,
    controller: Query<&Controller, With<crate::player::Player>>,
    mut animation: ResMut<PlayerAnimation>,
) {
    let Ok(ctrl) = controller.single() else {
        return;
    };
    let speed = Vec3::new(ctrl.velocity.x, 0.0, ctrl.velocity.z).length();
    let still = ctrl.motion == Motion::Idle;
    animation.state.still_for = if still {
        animation.state.still_for + time.delta_secs()
    } else {
        0.0
    };
    animation.state.motion = ctrl.motion;
    animation.state.speed = speed;
    animation.state.rising = ctrl.velocity.y >= 0.0;
    animation.state.combo = ctrl.combo;
}

/// Pushes the resolved clip onto every animation player the character owns.
pub fn update(
    animations: Res<CharacterAnimations>,
    mut state: ResMut<PlayerAnimation>,
    game: Res<crate::GameState>,
    mut players: Query<
        (&AnimationOwner, &mut AnimationPlayer, &mut AnimationTransitions),
        Without<AllyAnimationRoot>,
    >,
) {
    let character = game.active;
    if !animations.ready(character) {
        return;
    }
    let (name, rate) = resolve(character, &state.state);
    let changed = state.playing != Some((character, name));
    state.playing = Some((character, name));
    for (owner, mut player, mut transitions) in &mut players {
        if owner.0 != character {
            continue;
        }
        let Some(clip) = animations.named(owner.0, name) else {
            continue;
        };
        apply(&mut player, &mut transitions, clip, name, rate, changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(motion: Motion, speed: f32) -> AnimationState {
        AnimationState {
            motion,
            speed,
            ..default()
        }
    }

    #[test]
    fn walking_becomes_running_with_speed() {
        for character in [ActiveCharacter::Hero, ActiveCharacter::Mario] {
            let (slow, _) = resolve(character, &state(Motion::Run, 2.0));
            let (fast, _) = resolve(character, &state(Motion::Run, 12.0));
            assert_ne!(slow, fast, "{character:?} walks and runs the same clip");
        }
    }

    #[test]
    fn the_walk_rate_tracks_speed_but_stays_bounded() {
        let slow = resolve(ActiveCharacter::Hero, &state(Motion::Run, 1.0)).1;
        let brisk = resolve(ActiveCharacter::Hero, &state(Motion::Run, 5.0)).1;
        assert!(brisk > slow, "the stride does not keep up: {slow} {brisk}");
        // Even at an absurd tuned speed the legs stay watchable.
        let silly = resolve(ActiveCharacter::Hero, &state(Motion::Run, 400.0)).1;
        assert!((MIN_PLAY_RATE..=MAX_PLAY_RATE).contains(&silly), "{silly}");
    }

    #[test]
    fn the_run_plays_at_its_authored_rate() {
        // Only the walk is speed-scaled; a run scaled too would drift away
        // from how it looks in Blender.
        assert_eq!(
            resolve(ActiveCharacter::Hero, &state(Motion::Run, 12.0)).1,
            1.0
        );
    }

    /// The one speed at which a walk is exactly right, which is what anything
    /// free to choose its own pace should walk at. The allies do.
    #[test]
    fn marios_stride_speed_plays_his_walk_at_its_authored_rate() {
        let rate = resolve(
            ActiveCharacter::Mario,
            &state(Motion::Run, MARIO_STRIDE_SPEED),
        )
        .1;
        assert!(
            (rate - 1.0).abs() < 1e-3,
            "a Mario walking at his own stride speed plays at {rate}, so his \
             feet slide"
        );
    }

    #[test]
    fn standing_still_long_enough_plays_the_fidget() {
        let mut idle = state(Motion::Idle, 0.0);
        assert_eq!(resolve(ActiveCharacter::Hero, &idle).0, IDLE);
        idle.still_for = IDLE_VAR_AFTER + 1.0;
        assert_eq!(resolve(ActiveCharacter::Hero, &idle).0, IDLE_VAR);
        // And the fidget plays once rather than looping.
        assert!(!loops(IDLE_VAR));
    }

    #[test]
    fn attacks_alternate_so_a_combo_reads_as_two_swings() {
        for character in [ActiveCharacter::Hero, ActiveCharacter::Mario] {
            let mut attack = state(Motion::Attack, 0.0);
            let first = resolve(character, &attack).0;
            attack.combo = 1;
            let second = resolve(character, &attack).0;
            assert_ne!(first, second, "{character:?} repeats one swing");
            assert!(!loops(first) && !loops(second), "a swing must not cycle");
        }
    }

    #[test]
    fn thrust_holds_the_jump_and_falling_holds_the_descent() {
        let mut flying = state(Motion::Fly, 0.0);
        flying.rising = true;
        assert_eq!(resolve(ActiveCharacter::Hero, &flying).0, JUMP_RISE);
        flying.rising = false;
        assert_eq!(resolve(ActiveCharacter::Hero, &flying).0, JUMP_FALL);
    }

    /// The tables name clips by hand against two exporters, so this is what
    /// catches a rename or a typo before it becomes a character stuck in a
    /// T-pose.
    #[test]
    fn every_named_clip_exists_in_its_glb() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let hero = clip_names(&root.join("hero/hero.glb"));
        let mario = clip_names(&root.join("mario/mario.glb"));
        let motions = [
            Motion::Idle,
            Motion::Run,
            Motion::Jump,
            Motion::Fall,
            Motion::Land,
            Motion::Skate,
            Motion::Fly,
            Motion::Swim,
            Motion::Attack,
        ];
        for character in [ActiveCharacter::Hero, ActiveCharacter::Mario] {
            let present = match character {
                ActiveCharacter::Hero => &hero,
                ActiveCharacter::Mario => &mario,
            };
            for motion in motions {
                // Every branch of every state: slow and fast, rising and
                // falling, bored and not, both swings of the combo.
                for speed in [0.0, 12.0] {
                    for still_for in [0.0, IDLE_VAR_AFTER + 1.0] {
                        for rising in [true, false] {
                            for combo in [0, 1] {
                                let (clip, _) = resolve(
                                    character,
                                    &AnimationState {
                                        motion,
                                        speed,
                                        rising,
                                        still_for,
                                        combo,
                                    },
                                );
                                assert!(
                                    present.iter().any(|name| name == clip),
                                    "{character:?} {motion:?} wants a clip its glb has \
                                     no {clip:?} for"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Clip names straight out of the glTF JSON chunk, so the check does not
    /// depend on Bevy's loader or on a renderer.
    fn clip_names(path: &std::path::Path) -> Vec<String> {
        let bytes = std::fs::read(path).expect("missing glb");
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let json: serde_json::Value =
            serde_json::from_slice(&bytes[20..20 + length]).expect("bad glb json");
        json["animations"]
            .as_array()
            .map(|clips| {
                clips
                    .iter()
                    .filter_map(|clip| clip["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
}
