//! The camera workbench: the real game, headless, stepped one render frame
//! at a time at an exact frame rate, with scripted input and a recording of
//! everything the picture is made of.
//!
//! This exists because every camera bug so far survived the unit tests. The
//! unit tests call [`crate::camera::update`] by hand with a synthetic pose,
//! and the integration tests pump `App::update` as fast as the machine goes
//! -- neither runs the regime the player actually watches: sixty render
//! frames a second interleaved two-to-one with thirty fixed ticks, every
//! system in the real schedule, on the real terrain. Stutter lives in that
//! interleaving, and a harness that does not reproduce it cannot see it.
//!
//! The other thing the unit tests never measured is the *picture*. The eye
//! does not watch the camera transform; it watches drawn terrain, which goes
//! through [`crate::flatten::Curve`] on its way to the screen. So the bench
//! records the curve alongside the camera every frame, and its metrics
//! project world points through bend-then-camera -- the same trip a vertex
//! makes -- in screen units. A jump the player can see is a jump in that
//! series, wherever in the pipeline it started.

use crate::{
    camera::FollowCamera,
    console::GameTuning,
    flatten::Curve,
    gravity::Gravity,
    input::InputState,
    player::{Controller, Player, RenderPose},
    world::{LevelId, LevelLoad, LoadLevel},
};
use bevy::gltf::GltfPlugin;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;

/// The real game, headless, with the glTF loader bolted on.
///
/// [`crate::tests::headless`] deliberately has no loader: every system that
/// waits on an asset is exercised down its not-ready path there, which is the
/// path the real game takes on its first frames. This is the opposite game --
/// the one where assets arrive -- shared by the bench and by `world`'s own
/// load tests.
pub fn game() -> App {
    let mut app = crate::tests::headless();
    app.add_plugins((
        bevy::scene::ScenePlugin,
        bevy::world_serialization::WorldSerializationPlugin,
        bevy::image::ImagePlugin::default(),
        bevy::animation::AnimationPlugin,
        GltfPlugin::default(),
    ))
    // Reached for by the glTF loader whenever a mesh has a skeleton, which
    // every actor in this game does. `bevy_render` provides it in the real
    // build; here it would be an unexplained panic on an IO thread.
    .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>();
    // `GltfPlugin` registers its loader in `finish` rather than in `build`,
    // and `App::update` does not call it -- only `App::run` does. Without
    // this every asset sits at `Loading` for ever with nothing to say why.
    app.finish();
    app.cleanup();
    app
}

/// Everything one drawn frame is made of, as the bench saw it.
#[derive(Clone, Copy)]
pub struct Sample {
    /// Where the camera ended the frame.
    pub camera: Vec3,
    /// And which way it points.
    pub aim: Quat,
    /// The focus the camera orbits, the boom fraction the walls left it, and
    /// the view the horizon is squared to.
    pub focus: Vec3,
    pub clearance: f32,
    pub view: Quat,
    /// The pose the player is drawn at, and the tick pose underneath it.
    pub pose: Vec3,
    pub body: Vec3,
    /// Which way gravity said up was at the drawn pose.
    pub up: Vec3,
    /// Whether the simulation had him on the ground at the last tick.
    pub grounded: bool,
    /// The frame's flat-map, exactly as the terrain shader was handed it.
    pub curve: Curve,
}

/// A run of frames and the arithmetic asked of them.
pub struct Trace(pub Vec<Sample>);

/// The vertical field of view the projection below assumes, in radians.
/// Screen units come out with the full picture height spanning 2.0.
const FOV: f32 = std::f32::consts::FRAC_PI_3;

impl Trace {
    /// Where `world` lands on the screen in frame `i`, in a space where the
    /// picture is two units tall; `None` behind the camera. `bent` runs the
    /// point through that frame's flat-map first, which is the trip every
    /// drawn terrain vertex makes.
    pub fn screen(&self, i: usize, world: Vec3, bent: bool) -> Option<Vec2> {
        let sample = &self.0[i];
        let place = if bent {
            sample.curve.bend(world)
        } else {
            world
        };
        let eye = sample.aim.inverse() * (place - sample.camera);
        // A point closer than a couple of metres is being walked *through*:
        // its projection blows up as it crosses the lens, and a metric fed
        // that explosion reports a thousand-screen jump no eye ever saw.
        if eye.z >= -2.0 {
            return None;
        }
        Some(Vec2::new(eye.x, eye.y) / (-eye.z * (FOV * 0.5).tan()))
    }

    /// The largest single-frame step a screen track makes, in screen units,
    /// with the frame it happened on. Steps across `None` (the point off
    /// screen) are skipped rather than counted.
    pub fn worst_step(&self, world: Vec3, bent: bool) -> (f32, usize) {
        let mut worst = (0.0_f32, 0);
        let mut last: Option<Vec2> = None;
        for i in 0..self.0.len() {
            let now = self.watched(i, world, bent);
            if let (Some(a), Some(b)) = (last, now) {
                let step = (b - a).length();
                if step > worst.0 {
                    worst = (step, i);
                }
            }
            last = now;
        }
        worst
    }

    /// The largest *second difference* a screen track shows, with its frame:
    /// the stutter number. A smooth pan moves a point across the screen at a
    /// steadily-changing rate and its second difference sits near zero
    /// however fast the pan; a hitch -- one frame where something in the
    /// pipeline jumped -- spikes it by exactly the height of the jump.
    pub fn worst_jerk(&self, world: Vec3, bent: bool) -> (f32, usize) {
        let track: Vec<_> = (0..self.0.len())
            .map(|i| self.watched(i, world, bent))
            .collect();
        jerk(&track)
    }

    /// [`Self::screen`], but `None` once the point is too close or too far
    /// off screen to be something the eye is tracking: walking right up to a
    /// point sweeps it off the bottom of the picture at a rate that is all
    /// perspective and no stutter, and a metric that counts that reports
    /// geometry, not judder.
    fn watched(&self, i: usize, world: Vec3, bent: bool) -> Option<Vec2> {
        let sample = &self.0[i];
        if (world - sample.camera).length() < 10.0 {
            return None;
        }
        self.screen(i, world, bent)
            .filter(|p| p.x.abs() < 1.5 && p.y.abs() < 1.5)
    }

    /// The same stutter number for the drawn player himself: his pose
    /// projected through each frame's own camera. The one thing the eye
    /// tracks more closely than the ground.
    pub fn worst_pose_jerk(&self) -> (f32, usize) {
        let track: Vec<_> = (0..self.0.len())
            .map(|i| {
                let pose = self.0[i].pose;
                self.screen(i, pose, false)
            })
            .collect();
        jerk(&track)
    }

    /// The largest single-frame turn the aim makes, in radians, with its
    /// frame.
    pub fn worst_aim_step(&self) -> (f32, usize) {
        let mut worst = (0.0_f32, 0);
        for i in 1..self.0.len() {
            let step = self.0[i].aim.angle_between(self.0[i - 1].aim);
            if step > worst.0 {
                worst = (step, i);
            }
        }
        worst
    }

    /// How far the drawn player strays from the focus, at its worst -- the
    /// "camera lags way behind" number. Metres.
    pub fn worst_player_trail(&self) -> (f32, usize) {
        let mut worst = (0.0_f32, 0);
        for (i, sample) in self.0.iter().enumerate() {
            let trail = (sample.pose - sample.focus).length();
            if trail > worst.0 {
                worst = (trail, i);
            }
        }
        worst
    }

    /// The whole recording as CSV, for looking at a failure rather than
    /// re-deriving it.
    pub fn csv(&self, path: &str) {
        use std::fmt::Write;
        let mut out = String::from(
            "frame,cam_x,cam_y,cam_z,aim_x,aim_y,aim_z,aim_w,focus_x,focus_y,focus_z,\
             clearance,pose_x,pose_y,pose_z,body_x,body_y,body_z,grounded,up_x,up_y,up_z\n",
        );
        for (i, s) in self.0.iter().enumerate() {
            writeln!(
                out,
                "{i},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                s.camera.x,
                s.camera.y,
                s.camera.z,
                s.aim.x,
                s.aim.y,
                s.aim.z,
                s.aim.w,
                s.focus.x,
                s.focus.y,
                s.focus.z,
                s.clearance,
                s.pose.x,
                s.pose.y,
                s.pose.z,
                s.body.x,
                s.body.y,
                s.body.z,
                s.grounded as u8,
                s.up.x,
                s.up.y,
                s.up.z,
            )
            .expect("writing to a string");
        }
        let _ = std::fs::create_dir_all(
            std::path::Path::new(path)
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        );
        std::fs::write(path, out).expect("the trace should be writable");
    }
}

/// The largest second difference along a screen track, skipping any triple
/// with a gap in it.
fn jerk(track: &[Option<Vec2>]) -> (f32, usize) {
    let mut worst = (0.0_f32, 0);
    for i in 2..track.len() {
        if let (Some(a), Some(b), Some(c)) = (track[i - 2], track[i - 1], track[i]) {
            let spike = (c - b * 2.0 + a).length();
            if spike > worst.0 {
                worst = (spike, i);
            }
        }
    }
    worst
}

/// The bench itself: a loaded level and a frame counter.
pub struct Bench {
    pub app: App,
}

impl Bench {
    /// The real game with `level` loaded and the clock switched to exact
    /// sixtieths, so every `frame` below is one render frame of a 60 fps
    /// machine and every second one carries a fixed tick.
    pub fn on(level: LevelId) -> Self {
        let mut app = game();
        app.update();
        app.world_mut().write_message(LoadLevel(level));
        let started = std::time::Instant::now();
        let mut frames = 0;
        while app.world().resource::<LevelLoad>().busy() || frames == 0 {
            app.update();
            frames += 1;
            assert!(
                started.elapsed().as_secs() < 60,
                "the level never finished loading"
            );
        }
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_nanos(
            16_666_667,
        )));
        Self { app }
    }

    /// Re-paces the bench: how long every following frame claims to last.
    /// The default is a sixtieth; a bench can ask for 144 fps, or for the
    /// ragged 48 of a struggling machine.
    pub fn pace(&mut self, frame: Duration) {
        self.app
            .insert_resource(TimeUpdateStrategy::ManualDuration(frame));
    }

    /// One render frame: the script writes the input, the whole real schedule
    /// runs once, and the frame is measured. The input starts from empty
    /// every frame, so a held button is a script that keeps holding it.
    pub fn frame(&mut self, drive: impl FnOnce(&mut InputState)) -> Sample {
        {
            let mut input = self.app.world_mut().resource_mut::<InputState>();
            *input = InputState::default();
            drive(&mut input);
        }
        self.app.update();
        self.sample()
    }

    /// `count` frames under one script, recorded. The script sees the frame
    /// number, so "walk, and jump on frame 300" is a closure with an `if` in
    /// it.
    pub fn run(&mut self, count: usize, mut drive: impl FnMut(usize, &mut InputState)) -> Trace {
        let mut samples = Vec::with_capacity(count);
        for i in 0..count {
            samples.push(self.frame(|input| drive(i, input)));
        }
        Trace(samples)
    }

    /// What this frame put on screen, read back out of the world.
    pub fn sample(&mut self) -> Sample {
        let world = self.app.world_mut();
        let (camera, follow) = {
            let mut cameras = world.query::<(&Transform, &FollowCamera)>();
            let (transform, follow) = cameras
                .single(world)
                .expect("the bench world has one camera");
            (*transform, follow.clone())
        };
        let (body, grounded) = {
            let mut players = world.query_filtered::<(&Transform, &Controller), With<Player>>();
            let (transform, controller) = players
                .single(world)
                .expect("the bench world has one player");
            (transform.translation, controller.grounded)
        };
        let pose = world.resource::<RenderPose>().translation;
        let up = world.resource::<Gravity>().up(pose);
        let curve = *world.resource::<Curve>();
        Sample {
            camera: camera.translation,
            aim: camera.rotation,
            focus: follow.focus.unwrap_or(pose),
            clearance: follow.clearance,
            view: follow.view,
            pose,
            body,
            up,
            grounded,
            curve,
        }
    }

    /// The player's drawn position, for aiming scripts at the world he is in.
    pub fn pose(&self) -> Vec3 {
        self.app.world().resource::<RenderPose>().translation
    }

    /// The tuning knob block, for scripts that need the numbers the player
    /// plays with.
    pub fn tuning(&self) -> GameTuning {
        self.app.world().resource::<GameTuning>().clone()
    }

    /// The gravity field, for scripts that reason about up.
    pub fn up(&self, at: Vec3) -> Vec3 {
        self.app.world().resource::<Gravity>().up(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of walking forward after the drop-in, before anything is
    /// measured: the landing, the view settling, the walk finding its rhythm.
    const SETTLE: usize = 240;

    /// A terrain point a stride or two ahead of the player, projected near
    /// the middle of the screen -- the thing the eye actually watches while
    /// walking.
    fn watched_ground(bench: &mut Bench) -> Vec3 {
        let sample = bench.sample();
        let forward = sample.aim * Vec3::NEG_Z;
        let level = (forward - sample.up * forward.dot(sample.up)).normalize_or(Vec3::X);
        sample.pose + level * 50.0
    }

    /// The regression the workbench exists for, as one deterministic run on
    /// the real planet: walking must not stutter the picture, and jumping
    /// and jetpacking must not leave the camera behind. Every number here
    /// was measured broken before it was measured fixed:
    ///
    /// * ground jerk hit 0.034 screens -- an eighteen-pixel hitch -- from
    ///   the focus's terrain guard chattering against the height ease at
    ///   30 Hz, and from `Quat::from_rotation_arc` quantizing the view's
    ///   turn into 0.7-milliradian clicks. Fixed, it measures ~0.002.
    /// * the jetpack climb left the focus 6.7 m under the player when the
    ///   height follow was lazy in the air. Fixed, under half a metre.
    #[test]
    fn the_picture_holds_steady_walking_jumping_and_jetpacking() {
        let mut bench = Bench::on(LevelId::Planet);
        bench.run(SETTLE, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        let ahead = watched_ground(&mut bench);
        let walk = bench.run(360, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        for (what, (jerk, frame)) in [
            ("true ground", walk.worst_jerk(ahead, false)),
            ("drawn ground", walk.worst_jerk(ahead, true)),
        ] {
            assert!(
                jerk < 0.003,
                "walking hitched the {what} by {jerk} screens at frame {frame}"
            );
        }
        let (turn, frame) = walk.worst_aim_step();
        assert!(
            turn < 0.002,
            "walking ticked the aim {turn} rad in one frame, at frame {frame}"
        );
        let (trail, frame) = walk.worst_player_trail();
        assert!(
            trail < 3.0,
            "walking left the player {trail} m from the focus at frame {frame}"
        );

        // Stopping at whatever elevation the walk reached: the camera's
        // height must come to the player rather than drift there -- the
        // walk-time laziness is a bump filter, not a settling speed.
        let cam_height = bench.tuning().cam_height;
        let stop = bench.run(120, |_, _| {});
        let last = stop.0.last().expect("the stop recorded nothing");
        let gap = (last.pose + last.up * cam_height - last.focus).dot(last.up);
        assert!(
            gap.abs() < 0.4,
            "two seconds after stopping, the camera height is still {gap} m off the player's"
        );

        let jump = bench.run(240, |i, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
            input.jump = (10..14).contains(&i) || (120..124).contains(&i);
        });
        let (trail, frame) = jump.worst_player_trail();
        assert!(
            trail < 3.0,
            "a jump left the player {trail} m from the focus at frame {frame}"
        );

        let jet = bench.run(360, |i, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
            input.jump = (0..4).contains(&i);
            input.boost = i >= 6;
        });
        let (trail, frame) = jet.worst_player_trail();
        assert!(
            trail < 4.0,
            "the jetpack left the player {trail} m from the focus at frame {frame}"
        );
        let sunk = jet
            .0
            .iter()
            .map(|s| (s.pose - s.focus).dot(s.up))
            .fold(f32::MIN, f32::max);
        assert!(
            sunk < 1.0,
            "the jetpack climb left the focus {sunk} m below the player"
        );

        // And the same walk on the *orbiting* planet, where the ground
        // spins under the camera as well as rolling under the player. The
        // spin turns the local up by about half of `from_rotation_arc`'s
        // parallel cutoff every frame, so before [`crate::camera::arc`] the
        // view clicked at exactly 30 Hz here -- the stutter as the user saw
        // it, on the level the user plays. No ground-point jerk: a
        // world-fixed point only stays watchable for moments over terrain
        // moving at ninety metres a second, so the aim's own smoothness is
        // the assertion.
        let mut bench = Bench::on(LevelId::PlanetOrbit);
        bench.run(SETTLE * 2, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        let orbit = bench.run(300, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        let (turn, frame) = orbit.worst_aim_step();
        assert!(
            turn < 0.002,
            "walking the orbiting planet ticked the aim {turn} rad at frame {frame}"
        );
        let (trail, frame) = orbit.worst_player_trail();
        assert!(
            trail < 3.0,
            "the orbiting walk left the player {trail} m from the focus at frame {frame}"
        );
    }

    /// The stutter investigation, in numbers: walk, jump and jetpack on the
    /// real planet at an exact sixty frames a second, and print what the
    /// picture did. Ignored because it is a report, not a verdict -- run it
    /// with `cargo test bench_report -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_report() {
        let mut bench = Bench::on(LevelId::Planet);
        let walked = bench.run(SETTLE, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        drop(walked);
        let ahead = watched_ground(&mut bench);
        let walk = bench.run(600, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        walk.csv("target/workbench/walk.csv");
        {
            use std::fmt::Write;
            let mut out = String::from("frame,x,y,dist,aim_step\n");
            for i in 0..walk.0.len() {
                let s = &walk.0[i];
                let track = walk.screen(i, ahead, false);
                let step = if i > 0 {
                    walk.0[i].aim.angle_between(walk.0[i - 1].aim)
                } else {
                    0.0
                };
                let (x, y) = track.map_or((f32::NAN, f32::NAN), |p| (p.x, p.y));
                writeln!(out, "{i},{x},{y},{},{step}", (ahead - s.camera).length()).unwrap();
            }
            std::fs::write("target/workbench/walk_track.csv", out).unwrap();
        }
        println!("--- walk, 60 fps, 600 frames ---");
        println!("ground jerk (true)  {:?}", walk.worst_jerk(ahead, false));
        println!("ground jerk (bent)  {:?}", walk.worst_jerk(ahead, true));
        println!("ground step (true)  {:?}", walk.worst_step(ahead, false));
        println!("pose jerk           {:?}", walk.worst_pose_jerk());
        println!("aim step            {:?}", walk.worst_aim_step());
        println!("player trail        {:?}", walk.worst_player_trail());

        let jump = bench.run(240, |i, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
            input.jump = (10..14).contains(&i) || (120..124).contains(&i);
        });
        jump.csv("target/workbench/jump.csv");
        println!("--- two jumps mid-walk ---");
        println!("pose jerk           {:?}", jump.worst_pose_jerk());
        println!("player trail        {:?}", jump.worst_player_trail());

        let jet = bench.run(360, |i, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
            input.jump = (0..4).contains(&i);
            input.boost = i >= 6;
        });
        jet.csv("target/workbench/jetpack.csv");
        println!("--- jetpack climb ---");
        println!("pose jerk           {:?}", jet.worst_pose_jerk());
        println!("player trail        {:?}", jet.worst_player_trail());
        let heights: Vec<f32> = jet.0.iter().map(|s| (s.pose - s.focus).dot(s.up)).collect();
        println!(
            "focus below player  {:.2} m at worst",
            heights.iter().cloned().fold(f32::MIN, f32::max)
        );

        let mut bench = Bench::on(LevelId::PlanetOrbit);
        bench.run(SETTLE * 2, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        let ahead = watched_ground(&mut bench);
        let orbit = bench.run(360, |_, input| {
            input.move_axis = Vec2::new(0.0, 1.0);
        });
        orbit.csv("target/workbench/orbit_walk.csv");
        println!("--- walk on the orbiting planet ---");
        println!("ground jerk (true)  {:?}", orbit.worst_jerk(ahead, false));
        println!("pose jerk           {:?}", orbit.worst_pose_jerk());
        println!("aim step            {:?}", orbit.worst_aim_step());
        println!("player trail        {:?}", orbit.worst_player_trail());
    }
}
