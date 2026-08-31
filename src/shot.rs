//! Taking a picture of the running game without a screen.
//!
//! ```text
//! cargo run --release -- screenshot out.png [crowd] [x,y,z] [look-x,look-y,look-z]
//! SHOT_LEVEL=planet cargo run --release -- screenshot planet.png
//! ```
//!
//! `SHOT_LEVEL` names a level from [`crate::world::LevelId`] and waits for it
//! before taking the picture. It is the only way to see the planet without a
//! screen: its collision is read out of its glTF over however many frames that
//! takes, so a shot on the usual settle count is a shot of an empty sky. With
//! no camera coordinates given it frames whoever is standing on the level,
//! which on a planet is the only sensible default -- the spot the player is put
//! down on is chosen at load time and is not a number anyone could pass in.
//!
//! This exists because the game is developed in an environment that has no
//! display: WSL with no `/dev/dri`, where the renderer is lavapipe and there is
//! no window to look at. Without it the only way to know whether a change looks
//! right is to build the Windows package and go and run it, which is a slow
//! loop and one nobody follows for a change they believe is cosmetic.
//!
//! It runs the real `setup` and the real schedules and reads back the same
//! offscreen target [`crate::display`] gives the world camera, so what comes out
//! is what the game draws, less the stretch onto the window and the UI over the
//! top of it.

use crate::{
    console,
    display::SceneTarget,
    gravity::Gravity,
    player::Player,
    world::{LevelId, LevelLoad, LoadLevel},
};
use bevy::{
    prelude::*,
    render::gpu_readback::{Readback, ReadbackComplete},
    window::ExitCondition,
};

/// Frames run before the picture is taken.
///
/// The world arrives over several frames -- the glTFs load, the scenes spawn,
/// the materials are swapped onto [`crate::n64::N64Material`] and the pipelines
/// compile -- and a picture taken too early is a picture of a half-built level.
const SETTLE: usize = 120;

/// Where the shot is written and what should be in it.
#[derive(Resource)]
struct Shot {
    path: std::path::PathBuf,
    eye: Vec3,
    at: Vec3,
    /// Which way is up for the shot. `+Y` on a flat level; on a planet, the
    /// direction out of the ground being photographed -- a picture framed with
    /// the world's `Y` up there is a picture taken sideways, and one taken from
    /// directly over a pole is not a picture at all.
    up: Vec3,
    left: usize,
    asked: bool,
}

/// Puts the camera where the shot wants it, every frame.
///
/// Every frame rather than once, because `camera::update` is in the running
/// schedule and will happily drag the camera back to the player between the
/// placement and the picture.
#[allow(clippy::type_complexity)]
fn aim(
    shot: Res<Shot>,
    mut camera: Query<
        (&mut Transform, Option<&mut crate::camera::FollowCamera>),
        (With<Camera3d>, Without<crate::portal::PortalView>),
    >,
) {
    for (mut view, follow) in &mut camera {
        *view = Transform::from_translation(shot.eye).looking_at(shot.at, shot.up);
        let Some(mut follow) = follow else {
            continue;
        };
        // The eye is placed outright here, so there is no boom -- and a boom is
        // the whole of what `portal::carry_camera` asks about. Pinning the
        // focus to the eye makes that boom zero-length, which is the honest
        // description of this camera and stops a gate between the requested eye
        // and the player flying the picture to the other end of the pair. A
        // screenshot goes where it was told to go.
        follow.focus = Some(shot.eye);
        follow.eye = *view;
    }
}

/// Asks for the target's pixels once the world has settled.
fn capture(
    mut commands: Commands,
    mut shot: ResMut<Shot>,
    target: Option<Res<SceneTarget>>,
    camera: Query<Entity, (With<Camera3d>, Without<crate::portal::PortalView>)>,
) {
    let Some(target) = target else {
        return;
    };
    if shot.left > 0 {
        shot.left -= 1;
        return;
    }
    if shot.asked {
        return;
    }
    let Some(camera) = camera.iter().next() else {
        return;
    };
    commands
        .entity(camera)
        .insert(Readback::texture(target.0.clone()));
    shot.asked = true;
}

/// Writes the picture and stops.
fn keep(
    trigger: On<ReadbackComplete>,
    shot: Res<Shot>,
    images: Res<Assets<Image>>,
    target: Option<Res<SceneTarget>>,
    shot_stats: Res<crate::impostor::ImpostorStats>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(target) = target else {
        return;
    };
    let Some(image) = images.get(&target.0) else {
        return;
    };
    let size = image.texture_descriptor.size;
    let pixels = &trigger.event().data;
    let wanted = (size.width * size.height * 4) as usize;
    if pixels.len() < wanted {
        eprintln!(
            "screenshot: short readback, {} of {wanted} bytes",
            pixels.len()
        );
        exit.write(AppExit::error());
        return;
    }
    let crowd = *shot_stats;
    println!(
        "screenshot: {} sprites + {} skinned = {} drawn, {} MISSING",
        crowd.sprites,
        crowd.skinned,
        crowd.sprites + crowd.skinned,
        crowd.missing
    );
    match image::RgbaImage::from_raw(size.width, size.height, pixels[..wanted].to_vec()) {
        Some(picture) => match picture.save_with_format(&shot.path, image::ImageFormat::Png) {
            Ok(()) => println!(
                "screenshot: wrote {} ({}x{})",
                shot.path.display(),
                size.width,
                size.height
            ),
            Err(error) => eprintln!(
                "screenshot: could not write {}: {error}",
                shot.path.display()
            ),
        },
        None => eprintln!("screenshot: the readback was not a picture"),
    }
    exit.write(AppExit::Success);
}

/// The camera positions a shot of the castle falls back on: the view down the
/// path from the spawn, which is what every screenshot of this game has been.
const CASTLE_EYE: Vec3 = Vec3::new(-13.0, 8.0, 60.0);
const CASTLE_AT: Vec3 = Vec3::new(-13.0, 3.0, 46.0);

/// Runs the game long enough to photograph it.
pub fn run(path: &std::path::Path, crowd: usize, eye: Option<Vec3>, at: Option<Vec3>) {
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
                file_path: crate::asset_path().to_string_lossy().into_owned(),
                ..default()
            }),
    );
    crate::add_game(&mut app);
    app.insert_resource(Shot {
        path: path.to_path_buf(),
        eye: eye.unwrap_or(CASTLE_EYE),
        at: at.unwrap_or(CASTLE_AT),
        up: Vec3::Y,
        left: std::env::var("SHOT_SETTLE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(SETTLE),
        asked: false,
    })
    // In `PostUpdate` and before the transforms are propagated, so this is the
    // last word on where the camera is: `camera::update` runs in `Update` and
    // would otherwise drag it straight back to the player.
    .add_systems(
        PostUpdate,
        // `portal::aim_cameras` between the two, and it has to be *here*
        // rather than left to the copy `add_game` already registered. Both are
        // in `PostUpdate` before the transforms propagate and neither is
        // ordered against the other, so which one runs first is down to
        // whichever thread is free -- and a portal camera derived from the
        // camera position `aim` is about to overwrite is a portal showing the
        // view from wherever the player happened to be standing. Running it
        // twice in a frame costs a pair of matrix multiplies and writes the
        // same answer, which is a great deal cheaper than a screenshot that is
        // right four times out of five.
        (
            aim,
            crate::portal::carry_camera,
            crate::portal::aim_cameras,
            capture,
        )
            .chain()
            .before(bevy::transform::TransformSystems::Propagate),
    )
    .add_observer(keep);

    app.finish();
    app.cleanup();
    app.update();
    photograph_the_level(&mut app, eye, at);
    // Console commands, so the shot is set up through the same path a player
    // would use. `SHOT_SETUP="enemy_draw 12"` is how the impostor swap is
    // photographed: pull the skinned models in until the far crowd is sprites.
    let mut lines: Vec<String> = Vec::new();
    if crowd > 0 {
        lines.push(format!("crowd {crowd} mix"));
    }
    if let Ok(extra) = std::env::var("SHOT_SETUP") {
        lines.extend(extra.split(';').map(|line| line.trim().to_string()));
    }
    for line in lines {
        let mut tuning = app.world().resource::<console::GameTuning>().clone();
        app.world_mut()
            .resource_mut::<console::ConsoleState>()
            .execute(&line, &mut tuning);
        // Written back, because `execute` tunes a copy: it is deliberately
        // free of the `World` so the whole command table can be tested without
        // a renderer.
        *app.world_mut().resource_mut::<console::GameTuning>() = tuning;
    }
    let mut frames = 0;
    while app.should_exit().is_none() {
        app.update();
        frames += 1;
        if frames > SETTLE + 600 {
            eprintln!("screenshot: gave up after {frames} frames with no pixels");
            break;
        }
    }
}

/// Puts the level named by `SHOT_LEVEL` up, waits for it, and frames whoever is
/// standing on it.
///
/// The waiting is the point. A level whose collision comes out of a glTF is not
/// there on the frame it was asked for, and the settle count is measured from
/// *after* it arrives rather than from the start of the run -- otherwise the
/// picture is of the sky where the planet is going to be.
fn photograph_the_level(app: &mut App, eye: Option<Vec3>, at: Option<Vec3>) {
    let Ok(name) = std::env::var("SHOT_LEVEL") else {
        return;
    };
    let name = name.trim().to_ascii_lowercase();
    let Some(wanted) = LevelId::ALL
        .into_iter()
        .find(|id| id.name().to_ascii_lowercase().starts_with(&name))
    else {
        eprintln!(
            "screenshot: SHOT_LEVEL={name:?} is not a level; try one of {:?}",
            LevelId::ALL.map(LevelId::name)
        );
        return;
    };
    app.world_mut().write_message(LoadLevel(wanted));
    let mut frames = 0;
    while app.world().resource::<LevelLoad>().busy() || frames == 0 {
        app.update();
        frames += 1;
        if frames > 6_000 {
            eprintln!("screenshot: {} never finished loading", wanted.name());
            return;
        }
    }
    println!(
        "screenshot: {} came up after {frames} frames",
        wanted.name()
    );
    // Framed on the player unless the caller said where to stand. Over his
    // shoulder and a little above him, along whichever tangent the local up
    // happens to give -- on a planet there is no "north" for a default to mean.
    let standing = app
        .world_mut()
        .query_filtered::<&Transform, With<Player>>()
        .single(app.world())
        .map(|transform| transform.translation);
    let Ok(standing) = standing else {
        return;
    };
    let up = app.world().resource::<Gravity>().up(standing);
    let mut shot = app.world_mut().resource_mut::<Shot>();
    shot.up = up;
    shot.at = at.unwrap_or(standing + up * 1.0);
    shot.eye = eye.unwrap_or(standing + up * 4.0 + up.any_orthonormal_vector() * 14.0);
    // The clock starts again now that there is something to photograph.
    shot.left = std::env::var("SHOT_SETTLE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(SETTLE);
}
