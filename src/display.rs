//! What the game is drawn *into*, as opposed to what is drawn.
//!
//! The console rendered at 320x240 and the television magnified it. A modern
//! window is fifteen times that many pixels, and filling them all is both the
//! wrong look and the whole cost of a frame: everything expensive here --
//! overdraw on the water, the vertex-lit surfaces, the billboards -- scales
//! with the pixel count and with nothing else.
//!
//! So the world is not drawn to the window. It is drawn to an image of the
//! player's chosen size, and the image is stretched over the window by a second
//! camera. The stretch is nearest-neighbour, because the point of asking for
//! half resolution is to see half-resolution pixels rather than a blur of them.
//!
//! Two things stay at the window's own resolution and are meant to. The UI is
//! drawn by the second camera, after the stretch, so the HUD and the menu are
//! crisp whatever the world is rendered at -- an eighth-scale font is not a
//! retro font, it is an unreadable one. And the window itself is never resized:
//! the internal image follows the window's aspect ratio exactly, so the stretch
//! is uniform and a circle stays a circle.

use bevy::{
    camera::{ImageRenderTarget, RenderTarget},
    image::ImageSampler,
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureFormat},
        view::Msaa,
    },
    window::{MonitorSelection, PrimaryWindow, WindowMode},
};

/// Fails the build if the UI ever stops being drawn.
///
/// `bevy_ui` lays nodes out and draws none of them: the pass that puts one on
/// the screen is `bevy_ui_render`, which `DefaultPlugins` adds only when the
/// feature of that name is on. Losing it is silent -- every node is still
/// measured, still positioned, and simply never appears, which is the HUD, the
/// frame-rate readout, the console and the whole of this menu going missing at
/// once with nothing logged. Naming the type here turns that into a compile
/// error in the game rather than a black screen in the packaged build.
const _: fn() = || {
    let _ = core::mem::size_of::<bevy::ui_render::UiRenderPlugin>();
};

/// The resolutions on offer, as a percentage of the window's own.
///
/// Percentages rather than fixed heights like 240p, because the window can be
/// any shape and a fixed height would decide the aspect ratio for it. The
/// menu shows what each one works out to in pixels, which is the number
/// anybody actually wants to read.
pub const SCALES: [u32; 7] = [25, 33, 50, 66, 75, 85, 100];

/// The scale a fresh game starts at: none at all.
///
/// Full resolution is what the game did before this module existed, so it is
/// what it still does until somebody chooses otherwise. There is no settings
/// file yet, so this is also what it goes back to on every launch.
const DEFAULT_SCALE: usize = SCALES.len() - 1;

/// The player's display choices.
#[derive(Resource)]
pub struct DisplaySettings {
    /// Index into [`SCALES`], never out of range: the menu wraps rather than
    /// walking off either end.
    pub scale: usize,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            scale: DEFAULT_SCALE,
        }
    }
}

impl DisplaySettings {
    pub fn percent(&self) -> u32 {
        SCALES[self.scale]
    }

    /// Steps through [`SCALES`] and wraps, so holding one direction cycles
    /// rather than sticking at an end with no sign that it did anything.
    pub fn step_scale(&mut self, direction: i32) {
        let count = SCALES.len() as i32;
        self.scale = (((self.scale as i32 + direction) % count + count) % count) as usize;
    }
}

/// The image the world is drawn into.
///
/// Held as a resource rather than only on the camera because two other places
/// need it: the full-screen node that shows it, and [`resize`], which has to
/// find it in `Assets<Image>` every time the window or the setting changes.
#[derive(Resource, Deref)]
pub struct SceneTarget(pub Handle<Image>);

/// Order for the camera that shows the image and draws the UI.
///
/// Higher than the world camera's zero, which is what makes it run second --
/// a stretch of an image the world camera has not drawn yet is a stretch of
/// last frame.
const PRESENTATION_ORDER: isize = 1;

/// Creates the render target, at the size the window starts out.
///
/// The size here barely matters: [`resize`] corrects it on the first frame
/// against the window's real size, which on a borderless-fullscreen start is
/// the monitor's rather than the requested 1280x720. It matters that it is not
/// zero, which is not a texture.
pub fn create_target(images: &mut Assets<Image>) -> Handle<Image> {
    // Bevy no longer offers a "default" format to ask for -- the one it used
    // to hand back was this, and it is what the presentation pass expects to
    // sample: eight-bit colour, already display-referred, which is what the
    // console's framebuffer held too.
    let mut image = Image::new_target_texture(1280, 720, TextureFormat::Rgba8UnormSrgb, None);
    // The whole reason for the module. A linear filter would average the
    // low-resolution pixels back into a smear, which looks like a game running
    // out of focus rather than a game running at 320x240.
    image.sampler = ImageSampler::nearest();
    images.add(image)
}

/// What to put on the world camera so it draws into the image instead of the
/// window.
pub fn world_camera_target(target: &Handle<Image>) -> (RenderTarget, Camera) {
    (
        RenderTarget::Image(ImageRenderTarget {
            handle: target.clone(),
            // The image is already sized in physical pixels, so it needs no
            // second scaling of its own.
            scale_factor: 1.0,
        }),
        Camera {
            order: 0,
            ..default()
        },
    )
}

/// The camera that draws the image over the window, and the UI over that.
///
/// It is the only camera left targeting the window, which is what makes it the
/// default UI camera without anything being marked: Bevy picks the highest
/// order camera whose target is the primary window, and the world camera's
/// target is no longer the window at all.
pub fn presentation_camera() -> (Camera2d, Camera, Msaa) {
    (
        Camera2d,
        Camera {
            order: PRESENTATION_ORDER,
            ..default()
        },
        // Nothing this camera draws has an edge that multisampling would help:
        // one screen-filling quad, and text that is antialiased in its own
        // glyph atlas. Cameras default to four samples, which here is four
        // times the memory for the same picture.
        Msaa::Off,
    )
}

/// Marks the full-screen node the world is shown in.
#[derive(Component)]
pub struct SceneView;

/// The node that shows the image, stretched over the whole window.
///
/// Behind everything else in the UI: a negative `GlobalZIndex` puts it under
/// every node that does not ask for one, which is all of them -- the HUD, the
/// crosshair, the console and the menu.
pub fn scene_view_bundle(target: &Handle<Image>) -> (SceneView, ImageNode, Node, GlobalZIndex) {
    (
        SceneView,
        ImageNode::new(target.clone()),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        GlobalZIndex(-10),
    )
}

/// The internal resolution a window of this size renders at.
///
/// Rounded rather than truncated, and floored at one pixel: a scale that works
/// out to zero on a minimised window is a texture the GPU refuses to make.
pub fn internal_size(window: UVec2, percent: u32) -> UVec2 {
    UVec2::new(
        (window.x * percent).div_ceil(100).max(1),
        (window.y * percent).div_ceil(100).max(1),
    )
}

/// Keeps the render target at the size the window and the setting ask for.
///
/// Runs every frame and writes nothing when nothing changed, which is the
/// common case: touching the image at all costs a texture rebuild and a
/// re-upload, so the comparison is the point of the system rather than an
/// optimisation on top of it.
pub fn resize(
    settings: Res<DisplaySettings>,
    target: Res<SceneTarget>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let physical = window.physical_size();
    // A minimised window reports zero, and the frame is not drawn anyway.
    // Resizing to a floor of one pixel here would only mean rebuilding the
    // texture twice more when it comes back.
    if physical.x == 0 || physical.y == 0 {
        return;
    }
    let Some(mut image) = images.get_mut(&target.0) else {
        return;
    };
    let wanted = internal_size(physical, settings.percent());
    let current = image.texture_descriptor.size;
    if current.width == wanted.x && current.height == wanted.y {
        return;
    }
    image.resize(Extent3d {
        width: wanted.x,
        height: wanted.y,
        depth_or_array_layers: 1,
    });
}

/// Whether the window is currently a window rather than the whole screen.
pub fn is_windowed(mode: WindowMode) -> bool {
    matches!(mode, WindowMode::Windowed)
}

/// The other of the two window modes this game has. Borderless rather than
/// exclusive fullscreen, for the reason `main` gives where it asks for it.
pub fn other_mode(mode: WindowMode) -> WindowMode {
    if is_windowed(mode) {
        WindowMode::BorderlessFullscreen(MonitorSelection::Current)
    } else {
        WindowMode::Windowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_steps_wrap_in_both_directions() {
        let mut settings = DisplaySettings::default();
        assert_eq!(settings.percent(), 100);
        settings.step_scale(1);
        assert_eq!(settings.percent(), SCALES[0], "past the end comes the start");
        settings.step_scale(-1);
        assert_eq!(settings.percent(), 100, "and back again");
    }

    #[test]
    fn internal_size_scales_and_never_reaches_zero() {
        assert_eq!(
            internal_size(UVec2::new(1920, 1080), 50),
            UVec2::new(960, 540)
        );
        // Full scale is the window itself, exactly, whatever the arithmetic
        // does in between.
        assert_eq!(
            internal_size(UVec2::new(1366, 768), 100),
            UVec2::new(1366, 768)
        );
        // A quarter of a very small window is still a texture.
        assert_eq!(internal_size(UVec2::new(2, 2), 25), UVec2::new(1, 1));
    }

    /// The system that keeps the target in step with the window, run against a
    /// world with a window in it and no renderer behind it.
    #[test]
    fn the_target_follows_the_window_and_the_setting() {
        use bevy::ecs::system::RunSystemOnce;
        use bevy::window::WindowResolution;

        let mut world = World::new();
        world.init_resource::<Assets<Image>>();
        let handle = create_target(&mut world.resource_mut::<Assets<Image>>());
        world.insert_resource(SceneTarget(handle.clone()));
        world.insert_resource(DisplaySettings { scale: 0 });
        world.spawn((
            Window {
                resolution: WindowResolution::new(1600, 900),
                ..default()
            },
            PrimaryWindow,
        ));

        let size = |world: &World| {
            let image = world.resource::<Assets<Image>>().get(&handle).unwrap();
            let size = image.texture_descriptor.size;
            UVec2::new(size.width, size.height)
        };

        world.run_system_once(resize).unwrap();
        assert_eq!(
            size(&world),
            internal_size(UVec2::new(1600, 900), SCALES[0]),
            "the first frame corrects whatever size the target was made at"
        );

        world.resource_mut::<DisplaySettings>().scale = SCALES.len() - 1;
        world.run_system_once(resize).unwrap();
        assert_eq!(
            size(&world),
            UVec2::new(1600, 900),
            "full scale is the window, pixel for pixel"
        );

        // Nothing changed, so nothing is touched: a resize costs a texture
        // rebuild, and doing one every frame would be worse than the setting
        // is worth.
        let before = world.resource::<Assets<Image>>().get(&handle).unwrap().data.as_ref().map(Vec::len);
        world.run_system_once(resize).unwrap();
        assert_eq!(
            before,
            world.resource::<Assets<Image>>().get(&handle).unwrap().data.as_ref().map(Vec::len)
        );
    }

    /// The wiring itself, on a real renderer: a camera pointed at the target
    /// draws into a texture of the size the image asks for, and follows it when
    /// the size changes.
    ///
    /// Headless, with no window and no display -- the same trick
    /// `n64::tests::the_shader_compiles_on_a_real_renderer` uses, and for the
    /// same reason: everything here fails at *runtime* in the render world,
    /// where a compiling game says nothing about whether a frame draws.
    #[test]
    fn the_camera_draws_into_the_target_texture() {
        use bevy::{
            render::{render_asset::RenderAssets, texture::GpuImage, RenderApp},
            window::ExitCondition,
        };

        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .build()
                .disable::<bevy::winit::WinitPlugin>()
                // The render world is read back here, so it must stay on this
                // thread rather than being handed to the pipelined renderer.
                .disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>()
                .set(WindowPlugin {
                    primary_window: None,
                    exit_condition: ExitCondition::DontExit,
                    close_when_requested: false,
                    ..default()
                }),
        );

        let handle = create_target(&mut app.world_mut().resource_mut::<Assets<Image>>());
        app.world_mut().spawn((
            Camera3d::default(),
            world_camera_target(&handle),
            Transform::from_xyz(0.0, 0.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
        ));

        app.finish();
        app.cleanup();
        for _ in 0..4 {
            app.update();
        }

        let uploaded = |app: &App| {
            let render = app.sub_app(RenderApp);
            let images = render.world().resource::<RenderAssets<GpuImage>>();
            images
                .get(&handle)
                .map(|image| {
                    let size = image.texture_descriptor.size;
                    UVec2::new(size.width, size.height)
                })
                .expect("the render target never reached the GPU")
        };
        assert_eq!(uploaded(&app), UVec2::new(1280, 720));

        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .get_mut(&handle)
            .unwrap()
            .resize(Extent3d {
                width: 320,
                height: 240,
                depth_or_array_layers: 1,
            });
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            uploaded(&app),
            UVec2::new(320, 240),
            "changing the resolution has to rebuild the texture, not just the setting"
        );
    }

    #[test]
    fn the_two_window_modes_are_each_other() {
        assert!(is_windowed(other_mode(WindowMode::BorderlessFullscreen(
            MonitorSelection::Current
        ))));
        assert!(!is_windowed(other_mode(WindowMode::Windowed)));
    }
}
