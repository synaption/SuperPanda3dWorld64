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

use std::time::{Duration, Instant};

use bevy::{
    camera::{ImageRenderTarget, RenderTarget},
    image::ImageSampler,
    prelude::*,
    render::{
        render_resource::{Extent3d, TextureFormat, TextureUsages},
        view::Msaa,
    },
    window::{MonitorSelection, PresentMode, PrimaryWindow, WindowMode},
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

/// How many physical pixels each world pixel occupies in each axis.
pub const SCALES: [u32; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// The frame rates the cap offers, in Hz. Zero is no cap at all.
///
/// A list rather than a dial because the useful values are a panel's refresh
/// rate and its halves, not a continuum -- and the one to pick is the panel's
/// own rate a few frames short. On a variable-refresh display that is the whole
/// point: inside the panel's range the display follows the game, and above it
/// the game is drawing frames that are torn in half and thrown away.
pub const FRAME_CAPS: [u32; 7] = [0, 30, 60, 90, 120, 144, 240];

/// The cap a fresh game starts at: 60 fps.
const DEFAULT_FRAME_CAP: usize = 2;

/// How long [`cap_frames`] spins rather than sleeps at the end of a frame.
///
/// `sleep` promises "at least", never "exactly", and the overshoot is enough to
/// miss a 120 Hz frame even with the millisecond timer resolution `main` asks
/// Windows for. So the bulk of the wait is slept and the last of it is spun,
/// which is what a frame pacer is: a sleep that stops early and a spin that
/// finishes the job.
const SPIN: Duration = Duration::from_micros(1000);

/// The scale a fresh game starts at: none at all.
///
/// Full resolution is what the game did before this module existed, so it is
/// what it still does until somebody chooses otherwise. There is no settings
/// file yet, so this is also what it goes back to on every launch.
const DEFAULT_SCALE: usize = 0;

/// The player's display choices.
#[derive(Resource)]
pub struct DisplaySettings {
    /// Index into [`SCALES`], never out of range: the menu wraps rather than
    /// walking off either end.
    pub scale: usize,
    /// Index into [`FRAME_CAPS`], the same way.
    pub frame_cap: usize,
    /// Whether every creature on the field wears its hit points over its head.
    ///
    /// **Off, and it is a display setting rather than a game one**, because
    /// what it changes is what you are told and not what is true: the numbers
    /// are being kept either way -- see [`crate::health`] -- and this decides
    /// whether they are drawn. Off by default because a lawn of two hundred
    /// creatures with a bar over each is a lawn you cannot see, and because the
    /// fight it describes is meant to be read off the creatures themselves.
    /// It is the setting to reach for when a number is in question rather than
    /// one to leave on.
    pub unit_health_bars: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            scale: DEFAULT_SCALE,
            frame_cap: DEFAULT_FRAME_CAP,
            unit_health_bars: false,
        }
    }
}

impl DisplaySettings {
    pub fn pixel_scale(&self) -> u32 {
        SCALES[self.scale]
    }

    /// Steps through [`SCALES`] and wraps, so holding one direction cycles
    /// rather than sticking at an end with no sign that it did anything.
    pub fn step_scale(&mut self, direction: i32) {
        let count = SCALES.len() as i32;
        self.scale = (((self.scale as i32 + direction) % count + count) % count) as usize;
    }

    /// The cap in Hz, or `None` for no cap.
    pub fn frame_cap_hz(&self) -> Option<u32> {
        Some(FRAME_CAPS[self.frame_cap]).filter(|hz| *hz > 0)
    }

    /// Steps through [`FRAME_CAPS`] and wraps, exactly as `step_scale` does.
    pub fn step_frame_cap(&mut self, direction: i32) {
        let count = FRAME_CAPS.len() as i32;
        self.frame_cap = (((self.frame_cap as i32 + direction) % count + count) % count) as usize;
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
    // `new_target_texture` asks for the three usages a render target needs and
    // no more, and copying *out* of one is not among them. Without this the
    // world can be drawn but never read back, which is what the screenshot tool
    // does -- and the failure is not a black picture but a render-world error
    // that takes the whole app down with it. Free otherwise: a usage flag costs
    // nothing that is not used.
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
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
/// Both dimensions use the same integer divisor, preserving the window's
/// aspect ratio. Ceiling division keeps tiny windows at least one pixel wide.
pub fn internal_size(window: UVec2, pixel_scale: u32) -> UVec2 {
    UVec2::new(
        window.x.div_ceil(pixel_scale).max(1),
        window.y.div_ceil(pixel_scale).max(1),
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
    let wanted = internal_size(physical, settings.pixel_scale());
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

/// How long the last frame was held back, so the frame chart can take it off.
///
/// A frame that slept is not a frame that computed, and a chart that cannot
/// tell the difference reports a capped game as a slow one -- which is the
/// exact confusion this whole investigation started with. [`cap_frames`] runs
/// after `frame_chart::finish` so the chart never sees the wait at all; this is
/// for the phase probes, which bracket the whole of `Main` and would.
#[derive(Resource, Default)]
pub struct FramePacing {
    pub slept: Duration,
}

/// When the frame now ending is allowed to end, given the deadline the last one
/// left behind.
///
/// Split out from [`cap_frames`] because it is the whole of the pacing policy
/// and none of the waiting: a wall-clock test of the system around it can only
/// assert loose bounds and flakes under a loaded machine, while this can be
/// asked about a late frame, an early one and a first one exactly.
fn deadline(due: Option<Instant>, now: Instant, period: Duration) -> Instant {
    match due {
        // The normal case, and the mild overrun with it: a deadline already
        // gone by is still the right thing to measure the next one from, so a
        // frame that ran long is followed by a short one rather than by a full
        // period stacked on top of the overrun.
        Some(at) if now < at + period => at,
        // No deadline, or more than a whole frame past one -- the first capped
        // frame, a level load, a window drag, the cap having just changed. This
        // frame ends now and the next is measured from it. Waiting here would
        // be holding back a frame nothing was waiting for, and repaying a debt
        // by stalling is how a hitch becomes two.
        _ => now,
    }
}

/// Holds the frame back to the rate the player asked for.
///
/// Runs at the end of `Last`, after the frame chart has stopped its clock, so
/// the wait is not counted as work. The deadline is carried forward rather than
/// recomputed from "now" each frame: pacing from the end of the previous frame
/// is what keeps a run of slightly-long frames from drifting the rate down, and
/// the resync below is what stops a run of *very* long ones from being repaid
/// as a burst of uncapped frames.
pub fn cap_frames(
    settings: Res<DisplaySettings>,
    tuning: Res<crate::console::GameTuning>,
    mut pacing: ResMut<FramePacing>,
    mut due: Local<Option<Instant>>,
) {
    pacing.slept = Duration::ZERO;
    let Some(hz) = settings.frame_cap_hz() else {
        // Forgotten rather than kept, so turning the cap back on paces from
        // the frame that turned it on rather than from a deadline set minutes
        // ago and long since passed.
        *due = None;
        return;
    };
    let period = Duration::from_secs_f64(1.0 / f64::from(hz));
    let now = Instant::now();
    let target = deadline(*due, now, period);
    if target > now {
        let wait = target - now;
        // `frame_spin` holds the whole wait rather than the last millisecond
        // of it. See its entry in the console's table: it is there to tell one
        // cause of the post-pacing stall from the other, not to be left on.
        if wait > SPIN && tuning.frame_spin < 0.5 {
            std::thread::sleep(wait - SPIN);
        }
        while Instant::now() < target {
            std::hint::spin_loop();
        }
        pacing.slept = Instant::now().saturating_duration_since(now);
    }
    *due = Some(target + period);
}

/// Whether a fresh window waits for the display.
///
/// On, so a fresh window avoids tearing and paces rendering to the display.
/// Variable-refresh displays and diagnostics can turn it off from the pause
/// menu's Display page.
pub const DEFAULT_PRESENT_MODE: PresentMode = PresentMode::AutoVsync;

/// Whether the window waits for the display before it shows a frame.
///
/// Asked of the mode rather than tracked separately, so the menu row and the
/// window can never disagree about what is on: there is one answer and the
/// window holds it.
pub fn is_vsync(mode: PresentMode) -> bool {
    matches!(
        mode,
        PresentMode::AutoVsync | PresentMode::Fifo | PresentMode::FifoRelaxed
    )
}

/// The other of the two present modes this game offers.
///
/// Off is not a performance setting and is not offered as one. The game is
/// drawn at whatever rate the display runs at and the simulation is fixed-step
/// underneath that, so uncapping it buys no smoothness -- it buys a torn
/// picture and a hot GPU. What it is for is telling the two halves of a slow
/// frame apart: with the wait in place, the present block sits on the shared
/// task pool and any main-world system that wants a worker thread queues behind
/// it, so the frame chart reads a stall that is really the display setting the
/// pace. Turning this off is how you find out which one a tall bar was.
pub fn other_present_mode(mode: PresentMode) -> PresentMode {
    if is_vsync(mode) {
        PresentMode::AutoNoVsync
    } else {
        PresentMode::AutoVsync
    }
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

    /// The cap steps through the list and wraps, and `Off` is a real entry in
    /// it rather than a state beside it.
    #[test]
    fn the_frame_cap_wraps_through_its_choices() {
        let mut settings = DisplaySettings::default();
        assert_eq!(settings.frame_cap_hz(), Some(60), "a fresh game is 60 fps");

        settings.step_frame_cap(1);
        assert_eq!(settings.frame_cap_hz(), Some(FRAME_CAPS[3]));

        settings.step_frame_cap(-1);
        assert_eq!(settings.frame_cap_hz(), Some(60));
        // Off the near end and round to the far one, the way the scale does.
        settings.frame_cap = 0;
        settings.step_frame_cap(-1);
        assert_eq!(settings.frame_cap_hz(), Some(*FRAME_CAPS.last().unwrap()));
        settings.step_frame_cap(1);
        assert_eq!(settings.frame_cap_hz(), None);
    }

    /// The pacing policy, asked about each case it exists for.
    ///
    /// No clock and no sleeping, so it says the same thing on a loaded machine
    /// as an idle one -- see [`deadline`] for why that is worth splitting out.
    #[test]
    fn the_frame_cap_decides_when_a_frame_may_end() {
        let period = Duration::from_secs_f64(1.0 / 60.0);
        let now = Instant::now();

        assert_eq!(
            deadline(None, now, period),
            now,
            "the first capped frame has nothing to wait for and must not wait"
        );

        let ahead = now + period / 2;
        assert_eq!(
            deadline(Some(ahead), now, period),
            ahead,
            "a deadline still to come is the one to wait for"
        );

        // Overran, but by less than a frame: the deadline stands, so the next
        // frame is short rather than the rate slipping by the overrun.
        let just_missed = now - period / 2;
        assert_eq!(
            deadline(Some(just_missed), now, period),
            just_missed,
            "a deadline just missed is still what the next frame is measured from"
        );

        // Overran by more than a frame. Keeping this deadline would mean
        // running flat out to repay it, which is a stutter answered with a
        // burst. It is written off instead.
        let long_gone = now - period * 4;
        assert_eq!(
            deadline(Some(long_gone), now, period),
            now,
            "a deadline long past is abandoned rather than caught up on"
        );
    }

    /// And the system around it really does sleep, and says how long for.
    ///
    /// Driven through a `Schedule` rather than `run_system_once`, and that is
    /// the point rather than a detail: the deadline lives in a `Local`, which
    /// belongs to the system *instance*. `run_system_once` builds a fresh one
    /// per call, so every frame would look like the first and the pacer would
    /// never appear to pace.
    ///
    /// The assertion allows for the machine having taken a whole frame between
    /// the two runs on its own -- in which case there was correctly nothing to
    /// wait for. Asserting a duration instead would be asserting that the test
    /// machine was idle, which is not something a test can know.
    #[test]
    fn the_frame_cap_sleeps_and_reports_it() {
        let slowest = *FRAME_CAPS.iter().filter(|hz| **hz > 0).min().unwrap();
        let period = Duration::from_secs_f64(1.0 / f64::from(slowest));
        let mut world = World::new();
        world.insert_resource(DisplaySettings {
            frame_cap: FRAME_CAPS.iter().position(|hz| *hz == slowest).unwrap(),
            ..default()
        });
        world.init_resource::<FramePacing>();
        // The pacer asks the console whether it should spin rather than sleep.
        world.init_resource::<crate::console::GameTuning>();
        let mut schedule = Schedule::default();
        schedule.add_systems(cap_frames);

        // The first sets the deadline; the second is the one held to it.
        schedule.run(&mut world);
        let between = Instant::now();
        schedule.run(&mut world);
        let gap = between.elapsed();

        let slept = world.resource::<FramePacing>().slept;
        assert!(
            slept > Duration::ZERO || gap >= period,
            "the second frame neither slept nor had a reason not to: it took \
             {gap:?} against a {period:?} period"
        );
    }

    #[test]
    fn scale_steps_wrap_in_both_directions() {
        let mut settings = DisplaySettings::default();
        assert_eq!(settings.pixel_scale(), 1);
        settings.step_scale(1);
        assert_eq!(settings.pixel_scale(), 2);
        settings.step_scale(-1);
        assert_eq!(settings.pixel_scale(), 1, "and back again");
        settings.step_scale(-1);
        assert_eq!(settings.pixel_scale(), 8, "past the start comes the end");
    }

    #[test]
    fn internal_size_uses_square_pixel_multipliers() {
        assert_eq!(
            internal_size(UVec2::new(1920, 1080), 3),
            UVec2::new(640, 360)
        );
        // Full scale is the window itself, exactly, whatever the arithmetic
        // does in between.
        assert_eq!(
            internal_size(UVec2::new(1366, 768), 1),
            UVec2::new(1366, 768)
        );
        assert_eq!(internal_size(UVec2::new(2, 2), 8), UVec2::new(1, 1));
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
        // Spread from the default rather than written out: what this test is
        // about is the render scale, and a field added beside it should not be
        // a compile error here.
        world.insert_resource(DisplaySettings {
            scale: 1,
            ..default()
        });
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
            internal_size(UVec2::new(1600, 900), SCALES[1]),
            "the first frame corrects whatever size the target was made at"
        );

        world.resource_mut::<DisplaySettings>().scale = DEFAULT_SCALE;
        world.run_system_once(resize).unwrap();
        assert_eq!(
            size(&world),
            UVec2::new(1600, 900),
            "full scale is the window, pixel for pixel"
        );

        // Nothing changed, so nothing is touched: a resize costs a texture
        // rebuild, and doing one every frame would be worse than the setting
        // is worth.
        let before = world
            .resource::<Assets<Image>>()
            .get(&handle)
            .unwrap()
            .data
            .as_ref()
            .map(Vec::len);
        world.run_system_once(resize).unwrap();
        assert_eq!(
            before,
            world
                .resource::<Assets<Image>>()
                .get(&handle)
                .unwrap()
                .data
                .as_ref()
                .map(Vec::len)
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
