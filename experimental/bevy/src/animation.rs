use crate::{player::Motion, ActiveCharacter};
use bevy::prelude::*;

#[derive(Resource)]
pub struct CharacterAnimations {
    hero: [Handle<AnimationClip>; 7],
    mario: [Handle<AnimationClip>; 7],
}

#[derive(Component)]
pub struct AnimationOwner(pub ActiveCharacter);

const IDLE: usize = 0;
const RUN: usize = 1;
const JUMP: usize = 2;
const FALL: usize = 3;
const SKATE: usize = 4;
const FLY: usize = 5;
const ATTACK: usize = 6;

impl CharacterAnimations {
    pub fn load(assets: &AssetServer) -> Self {
        let hero = [4, 14, 13, 8, 14, 13, 0]
            .map(|index| assets.load(format!("hero/hero.glb#Animation{index}")));
        let mario = [197, 114, 77, 86, 210, 77, 103]
            .map(|index| assets.load(format!("mario/mario.glb#Animation{index}")));
        Self { hero, mario }
    }

    fn clip(&self, character: ActiveCharacter, motion: Motion) -> Handle<AnimationClip> {
        let index = match motion {
            Motion::Idle => IDLE,
            Motion::Run => RUN,
            Motion::Jump => JUMP,
            Motion::Fall => FALL,
            Motion::Skate => SKATE,
            Motion::Fly => FLY,
            Motion::Attack => ATTACK,
        };
        match character {
            ActiveCharacter::Hero => self.hero[index].clone_weak(),
            ActiveCharacter::Mario => self.mario[index].clone_weak(),
        }
    }
}

pub fn claim_players(
    mut commands: Commands,
    animations: Res<CharacterAnimations>,
    hierarchy: Query<&Parent>,
    characters: Query<&ActiveCharacter>,
    enemies: Query<&crate::enemy::Enemy>,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
) {
    for (entity, mut player) in &mut players {
        let mut ancestor = entity;
        let owner = loop {
            if let Ok(character) = characters.get(ancestor) {
                break Some(*character);
            }
            if let Ok(enemy) = enemies.get(ancestor) {
                player.play(enemy.animation.clone_weak()).repeat();
                break None;
            }
            let Ok(parent) = hierarchy.get(ancestor) else {
                break None;
            };
            ancestor = parent.get();
        };
        let Some(character) = owner else { continue };
        player
            .play(animations.clip(character, Motion::Idle))
            .repeat();
        commands.entity(entity).insert(AnimationOwner(character));
    }
}

pub fn update(
    animations: Res<CharacterAnimations>,
    controller: Query<&crate::player::Controller>,
    mut players: Query<(&AnimationOwner, &mut AnimationPlayer)>,
) {
    let motion = controller.single().motion;
    for (owner, mut player) in &mut players {
        let clip = animations.clip(owner.0, motion);
        if !player.is_playing_clip(&clip) {
            player.play_with_transition(clip, std::time::Duration::from_millis(100));
        }
        if motion == Motion::Attack {
            player.set_repeat(bevy::animation::RepeatAnimation::Never);
        } else {
            player.repeat();
        }
    }
}
