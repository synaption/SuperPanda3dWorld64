//! Taking a picture of the running game without a screen.
//!
//! ```text
//! cargo run --release -- screenshot out.png [crowd] [x,y,z] [look-x,look-y,look-z]
//! ```
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

use crate::{console, display::SceneTarget};
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
    left: usize,
    asked: bool,
}

/// Puts the camera where the shot wants it, every frame.
///
/// Every frame rather than once, because `camera::update` is in the running
/// schedule and will happily drag the camera back to the player between the
/// placement and the picture.
fn aim(shot: Res<Shot>, mut camera: Query<&mut Transform, With<Camera3d>>) {
    for mut view in &mut camera {
        *view = Transform::from_translation(shot.eye).looking_at(shot.at, Vec3::Y);
    }
}

/// Asks for the target's pixels once the world has settled.
fn capture(
    mut commands: Commands,
    mut shot: ResMut<Shot>,
    target: Option<Res<SceneTarget>>,
    camera: Query<Entity, With<Camera3d>>,
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

/// Runs the game long enough to photograph it.
pub fn run(path: &std::path::Path, crowd: usize, eye: Vec3, at: Vec3) {
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
        eye,
        at,
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
        (aim, capture)
            .chain()
            .before(bevy::transform::TransformSystems::Propagate),
    )
    .add_observer(keep);

    app.finish();
    app.cleanup();
    app.update();
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
