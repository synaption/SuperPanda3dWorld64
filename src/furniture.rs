//! What a level has in it, placed in Blender rather than written down here.
//!
//! Everything a level is made of used to be a literal. The three warp pipes
//! and what each produced, the five enemies standing about, where the player
//! was put down, which way down was, and the two boxes of water -- those were
//! arrays in [`crate::world`], constants in [`crate::water`] and a field of the
//! decomp's collision data. Moving a warp pipe six metres was a code change,
//! and the only way to see where one was going to end up was to run the game.
//!
//! So they are authored in `assets/levels/<level>.blend` instead, as empties
//! you can drag around against the level's own geometry, and
//! `tools/export_level_furniture.py` carries them across into the two files
//! this module reads. That tool's docstring is the authority on what an object
//! has to be called; this one is the authority on what the game does with it.
//!
//! Two files, because the game wants the two halves at different times:
//!
//! - the JSON is embedded with `include_str!` the way `assets/bevy/castle.bin`
//!   is embedded, so gravity, the spawn point and the water are known in the
//!   frame the level comes up. A level whose "down" arrived three frames late
//!   is a level the player falls out of.
//! - the .glb holds the surface meshes, and is loaded like any other asset.
//!   Those are scenery and can arrive when they arrive.
//!
//! Nothing here spawns anything. It reads a file and hands back types the rest
//! of the port already speaks -- [`Gravity`], [`WaterBox`], [`pipe::Spawn`] --
//! and [`crate::world`] does the spawning, because putting a level up and
//! taking it down again is its job and not this one's.

use crate::{enemy, gravity::Gravity, level::WaterBox, pipe};
use bevy::prelude::*;
use serde::Deserialize;

/// The castle's furniture, as the exporter wrote it.
///
/// `include_str!` rather than an asset load, and the reasoning is
/// `src/level.rs`'s: this is small, the game cannot start without it, and a
/// build that has it is a build that cannot be missing it. It also means
/// rustc tracks the file, so re-exporting the .blend rebuilds the crate.
const CASTLE: &str = include_str!("../assets/bevy/castle_furniture.json");

/// One level's worth of placements.
#[derive(Deserialize, Debug, Clone)]
pub struct Furniture {
    /// Which level this is, for the error message when it is the wrong one.
    pub level: String,
    /// Where the player is put down when the level comes up.
    spawn: [f32; 3],
    gravity: GravitySpec,
    #[serde(default)]
    water: Vec<WaterSpec>,
    #[serde(default)]
    pipes: Vec<PipeSpec>,
    #[serde(default)]
    actors: Vec<ActorSpec>,
    #[serde(default)]
    surfaces: Vec<SurfaceSpec>,
}

/// Which way down is, as one empty in the .blend says it.
///
/// Tagged on `mode` so that an unknown one fails to parse rather than falling
/// back on flat gravity, which on a planet is a level nobody can stand on.
#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(tag = "mode", rename_all = "lowercase")]
enum GravitySpec {
    Down {
        #[serde(default)]
        accel: Option<f32>,
    },
    Radial {
        centre: [f32; 3],
        #[serde(default)]
        accel: Option<f32>,
    },
}

/// A water box's footprint and the height its surface sits at.
///
/// Its own type rather than [`WaterBox`] with a derive on it: this is a file
/// format, written by a Python tool that cannot be recompiled against a field
/// rename, and the conversion below is where the two are held together on
/// purpose.
#[derive(Deserialize, Debug, Clone, Copy)]
struct WaterSpec {
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
    surface_y: f32,
}

#[derive(Deserialize, Debug, Clone, Copy)]
struct PipeSpec {
    spawns: Kind,
    interval: f32,
    at: [f32; 3],
    /// Which way it is turned, about the vertical, in radians.
    #[serde(default)]
    yaw: f32,
    /// How big. `warp_pipe.glb` is 307 units across and a warp pipe is three
    /// metres, so this is 0.01 unless somebody has resized one.
    #[serde(default = "unscaled")]
    scale: f32,
}

fn unscaled() -> f32 {
    1.0
}

#[derive(Deserialize, Debug, Clone, Copy)]
struct ActorSpec {
    kind: Kind,
    at: [f32; 3],
}

/// What can be placed, and what a pipe can produce.
///
/// One list for both, because a slime is a slime whether it walked out of a
/// pipe or was standing there when the level came up.
///
/// An enum rather than the string it is in the file, so that a name this game
/// does not have fails to parse. That turns a typo in Blender into a refusal
/// to start, naming the file and the word -- which is the loudest this can be
/// made, and much better than the alternative: a warp pipe that silently
/// produces nothing, in a game where a pipe often has nothing to produce
/// because it is already at its quota.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Mario,
    Slime,
    Ant,
}

impl Kind {
    fn spawn(self) -> pipe::Spawn {
        match self {
            Kind::Mario => pipe::Spawn::Mario,
            Kind::Slime => pipe::Spawn::Enemy(enemy::Kind::Slime),
            Kind::Ant => pipe::Spawn::Enemy(enemy::Kind::Ant),
        }
    }
}

/// One warp pipe, ready to spawn.
#[derive(Debug, Clone, Copy)]
pub struct Pipe {
    pub spawns: pipe::Spawn,
    pub interval: f32,
    pub at: Transform,
}

/// A drawn surface: a mesh in the .glb, and what makes it move.
#[derive(Deserialize, Debug, Clone)]
pub struct SurfaceSpec {
    /// The glTF node's name, which is the Blender object's name. This is what
    /// finds the mesh once the file has loaded.
    pub node: String,
    /// Texture repeats a second, along S and along T.
    #[serde(default)]
    pub drift: [f32; 2],
    /// How much of what is behind it shows through.
    #[serde(default = "opaque")]
    pub alpha: f32,
}

fn opaque() -> f32 {
    1.0
}

/// The castle's, parsed.
///
/// Panics if the embedded file will not parse, which is the right response to
/// it: the file is compiled into the binary, so a build that gets here at all
/// has already shipped it, and the alternative to stopping is a castle with no
/// water, no pipes and nobody on it that looks like a gameplay bug.
pub fn castle() -> Furniture {
    let furniture: Furniture = serde_json::from_str(CASTLE).unwrap_or_else(|error| {
        panic!("assets/bevy/castle_furniture.json does not parse: {error}")
    });
    assert_eq!(
        furniture.level, "castle",
        "castle_furniture.json is some other level's"
    );
    furniture
}

impl Furniture {
    pub fn spawn(&self) -> Vec3 {
        point(self.spawn)
    }

    pub fn gravity(&self) -> Gravity {
        match self.gravity {
            GravitySpec::Down { accel } => Gravity::Down {
                accel: accel.unwrap_or(crate::gravity::FALL),
            },
            GravitySpec::Radial { centre, accel } => Gravity::Radial {
                centre: point(centre),
                accel: accel.unwrap_or(crate::gravity::FALL),
            },
        }
    }

    /// The water, in the shape the collision keeps it in.
    pub fn water_boxes(&self) -> Vec<WaterBox> {
        self.water
            .iter()
            .map(|w| WaterBox {
                // Ordered here rather than trusted from the file: a plane
                // dragged past itself in Blender has a bounding box that is
                // still the right box, and the exporter reads corners rather
                // than sides, but a hand-edited file need not.
                min_x: w.min_x.min(w.max_x),
                max_x: w.min_x.max(w.max_x),
                min_z: w.min_z.min(w.max_z),
                max_z: w.min_z.max(w.max_z),
                surface_y: w.surface_y,
            })
            .collect()
    }

    /// The warp pipes: what each produces, how often, and how it stands.
    pub fn pipes(&self) -> Vec<Pipe> {
        self.pipes
            .iter()
            .map(|p| Pipe {
                spawns: p.spawns.spawn(),
                interval: p.interval,
                // The whole transform, which no other placement gets. A pipe
                // is drawn and not collided with, so nothing depends on how
                // big it is and the level is free to say -- which is what
                // moves the last of its numbers out of the source.
                at: Transform::from_translation(point(p.at))
                    .with_rotation(Quat::from_rotation_y(p.yaw))
                    .with_scale(Vec3::splat(p.scale)),
            })
            .collect()
    }

    /// Who is standing about when the level comes up.
    pub fn actors(&self) -> Vec<(pipe::Spawn, Vec3)> {
        self.actors
            .iter()
            .map(|a| (a.kind.spawn(), point(a.at)))
            .collect()
    }

    pub fn surfaces(&self) -> &[SurfaceSpec] {
        &self.surfaces
    }
}

fn point(value: [f32; 3]) -> Vec3 {
    Vec3::from_array(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file is compiled in, so this is the closest thing there is to a
    /// build-time check that Blender wrote something the game can read.
    #[test]
    fn the_castle_has_its_furniture() {
        let castle = castle();
        assert_eq!(castle.gravity(), Gravity::Down { accel: 36.0 });
        assert_eq!(castle.water_boxes().len(), 2, "the moat and the bay");
        assert_eq!(castle.pipes().len(), 3);
        assert_eq!(castle.actors().len(), 5);
        assert_eq!(castle.surfaces().len(), 1, "the waterfall");
    }

    /// The placements this replaced were literals in `world.rs`, and the
    /// migration's whole claim is that the level did not move. A round trip
    /// through Blender is where it would have: metres for centimetres, Y for
    /// Z, or a spawn that ends up inside the castle wall.
    #[test]
    fn the_castle_is_where_it_always_was() {
        let castle = castle();
        assert!((castle.spawn() - Vec3::new(-13.28, 3.0, 46.64)).length() < 1e-3);
        let pipes = castle.pipes();
        assert_eq!(pipes[0].spawns, pipe::Spawn::Mario);
        assert!((pipes[0].at.translation - Vec3::new(-9.15, 2.6, 46.3)).length() < 1e-3);
        assert!(pipes
            .iter()
            .any(|pipe| pipe.spawns == pipe::Spawn::Enemy(enemy::Kind::Ant)
                && (pipe.at.translation - Vec3::new(46.8, 5.4, -68.1)).length() < 1e-3));
        // `warp_pipe.glb` is 307 units across and a warp pipe is three metres.
        // The factor was a literal in `world.rs` until the .blend drew the
        // pipe at it.
        assert!((pipes[0].at.scale.x - 0.01).abs() < 1e-6);
        // The moat, from the decomp's first water box.
        let moat = castle.water_boxes()[0];
        assert!((moat.min_x - -71.29).abs() < 1e-2 && (moat.max_x - 82.53).abs() < 1e-2);
        assert!((moat.surface_y - -0.81).abs() < 1e-3);
    }

    /// A water box with its corners named the wrong way round is a box that
    /// contains nothing, and a plane in Blender can be dragged through itself
    /// without ever looking wrong.
    #[test]
    fn a_water_box_comes_back_the_right_way_round() {
        let backwards: Furniture = serde_json::from_str(
            r#"{"level":"test","spawn":[0,0,0],"gravity":{"mode":"down"},
                "water":[{"min_x":10,"max_x":-10,"min_z":4,"max_z":-4,"surface_y":1}]}"#,
        )
        .expect("that should parse");
        let box_ = backwards.water_boxes()[0];
        assert!(box_.min_x < box_.max_x && box_.min_z < box_.max_z);
    }

    /// A pipe turned and resized in Blender is turned and resized in the
    /// game. It is the one placement that can be: nothing collides with a warp
    /// pipe, so its size is nobody else's business.
    #[test]
    fn a_pipe_carries_the_turn_and_the_size_it_was_drawn_at() {
        let level: Furniture = serde_json::from_str(
            r#"{"level":"test","spawn":[0,0,0],"gravity":{"mode":"down"},
                "pipes":[{"spawns":"ant","interval":8,"at":[1,2,3],
                          "yaw":1.5707963,"scale":0.02}]}"#,
        )
        .expect("that should parse");
        let pipe = level.pipes()[0];
        assert!((pipe.at.scale - Vec3::splat(0.02)).length() < 1e-6);
        // A quarter turn about the vertical takes the model's +Z to +X.
        let turned = pipe.at.rotation * Vec3::Z;
        assert!((turned - Vec3::X).length() < 1e-4, "{turned:?}");
    }

    /// An older furniture file, or one written by hand, says neither. It must
    /// come up the size the model is rather than at nothing.
    #[test]
    fn a_pipe_that_says_no_size_is_left_alone() {
        let level: Furniture = serde_json::from_str(
            r#"{"level":"test","spawn":[0,0,0],"gravity":{"mode":"down"},
                "pipes":[{"spawns":"ant","interval":8,"at":[0,0,0]}]}"#,
        )
        .expect("that should parse");
        assert_eq!(level.pipes()[0].at.scale, Vec3::ONE);
    }

    /// The planet's gravity is not authored anywhere yet, but the format has
    /// to be able to say it before a planet furniture file can exist.
    #[test]
    fn gravity_can_point_at_a_planet() {
        let planet: Furniture = serde_json::from_str(
            r#"{"level":"planet","spawn":[0,300,0],
                "gravity":{"mode":"radial","centre":[0,0,0],"accel":20}}"#,
        )
        .expect("that should parse");
        assert_eq!(
            planet.gravity(),
            Gravity::Radial {
                centre: Vec3::ZERO,
                accel: 20.0
            }
        );
    }

    /// A pipe that spawns something this game does not have must stop the game
    /// rather than stand there producing nothing, which is indistinguishable
    /// from a pipe that has met its quota.
    #[test]
    fn a_pipe_that_spawns_a_typo_does_not_parse() {
        let error = serde_json::from_str::<Furniture>(
            r#"{"level":"test","spawn":[0,0,0],"gravity":{"mode":"down"},
                "pipes":[{"spawns":"slim","interval":8,"at":[0,0,0]}]}"#,
        )
        .expect_err("that should not parse");
        assert!(error.to_string().contains("slim"), "{error}");
    }

    /// Likewise a gravity nobody implemented: falling back on flat would put a
    /// planet's player on a trajectory off the side of it.
    #[test]
    fn an_unknown_gravity_does_not_parse() {
        assert!(serde_json::from_str::<Furniture>(
            r#"{"level":"test","spawn":[0,0,0],"gravity":{"mode":"sideways"}}"#,
        )
        .is_err());
    }
}
