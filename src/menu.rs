//! The pause menu: Escape, and what is behind it.
//!
//! Three pages deep and no deeper -- the root, its options, and the display
//! settings -- because a game with one screen of settings does not need a tree
//! and a menu you can get lost in is worse than no menu. Each page is a list of
//! rows; a row either does something when it is chosen or holds a value that
//! left and right change.
//!
//! It is drawn the way the console is: a fixed set of text nodes spawned once
//! at startup and rewritten every frame, rather than entities despawned and
//! respawned as pages change. A menu that spawns is a menu that flickers for a
//! frame while the layout catches up, and it makes every page a lifetime
//! question about who owns which node.
//!
//! Nothing here reads [`crate::input::InputState`]. That snapshot is player
//! intent -- it goes neutral while the game is paused, which is exactly when
//! this needs to be read -- so the keyboard and the pad are read directly, the
//! same way `toggle_fullscreen` reads F11.

use crate::{
    console::ConsoleState,
    display::{self, DisplaySettings, SceneTarget},
};
use bevy::{
    app::AppExit,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

/// How many text rows are spawned for the menu to write into.
///
/// The longest page has three, and one spare costs a text node that is hidden
/// on every page rather than a fourth page's worth of respawning.
const ROWS: usize = 4;

/// Which page is showing.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    #[default]
    Root,
    Options,
    Display,
}

/// One line of a page.
///
/// The value rows ([`Item::RenderScale`], [`Item::WindowMode`]) are also
/// choosable: Enter on them steps the value forward, which is what a player who
/// never tries the arrow keys will do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Item {
    Resume,
    Options,
    Quit,
    Display,
    RenderScale,
    WindowMode,
    Back,
}

impl Page {
    fn items(self) -> &'static [Item] {
        match self {
            Page::Root => &[Item::Resume, Item::Options, Item::Quit],
            Page::Options => &[Item::Display, Item::Back],
            Page::Display => &[Item::RenderScale, Item::WindowMode, Item::Back],
        }
    }

    fn title(self) -> &'static str {
        match self {
            Page::Root => "PAUSED",
            Page::Options => "OPTIONS",
            Page::Display => "DISPLAY",
        }
    }

    /// Where Escape, or a row called Back, goes from here. The root has
    /// nowhere left to go, which is what shuts the menu.
    fn parent(self) -> Option<Page> {
        match self {
            Page::Root => None,
            Page::Options => Some(Page::Root),
            Page::Display => Some(Page::Options),
        }
    }
}

/// Whether the menu is up, and where in it the player is.
#[derive(Resource, Default)]
pub struct MenuState {
    pub open: bool,
    /// Set for the one frame the menu shuts on, so the press that shut it is
    /// not also read as a press in the game -- the same trick, and for the same
    /// reason, as [`ConsoleState::closed_this_frame`].
    pub closed_this_frame: bool,
    page: Page,
    /// Which row is highlighted. Always a valid index for `page`: every place
    /// that changes the page resets it.
    row: usize,
}

impl MenuState {
    fn selected(&self) -> Item {
        let items = self.page.items();
        items[self.row.min(items.len() - 1)]
    }

    fn move_row(&mut self, direction: i32) {
        let count = self.page.items().len() as i32;
        self.row = (((self.row as i32 + direction) % count + count) % count) as usize;
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.row = 0;
    }

    fn open(&mut self) {
        self.open = true;
        self.go(Page::Root);
    }

    fn close(&mut self) {
        self.open = false;
        self.closed_this_frame = true;
        self.go(Page::Root);
    }
}

/// The run condition gameplay is gated on, alongside the console's.
pub fn is_closed(menu: Res<MenuState>) -> bool {
    !menu.open
}

/// Marks the dimming sheet the menu is drawn on, which is what is shown and
/// hidden -- its children come and go with it.
#[derive(Component)]
pub struct MenuRoot;

#[derive(Component)]
pub struct MenuTitle;

/// One rewritable row, numbered so [`draw`] can find the nth.
#[derive(Component)]
pub struct MenuRow(usize);

/// Spawns the menu, hidden.
pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            MenuRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            // Dimmed rather than opaque: the display settings change what is
            // behind the menu, and a player choosing a render scale should be
            // able to see what they chose without shutting the menu first.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
            // Above the console's panel, so a console left open behind the
            // menu does not cover it.
            GlobalZIndex(200),
            Visibility::Hidden,
        ))
        .with_children(|panel| {
            panel
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::FlexStart,
                        padding: UiRect::axes(Val::Px(28.0), Val::Px(22.0)),
                        row_gap: Val::Px(6.0),
                        min_width: Val::Px(420.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.015, 0.02, 0.04, 0.94)),
                ))
                .with_children(|rows| {
                    rows.spawn((
                        MenuTitle,
                        Text::new(""),
                        TextFont {
                            font_size: FontSize::Px(30.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.85, 0.9, 1.0)),
                        Node {
                            margin: UiRect::bottom(Val::Px(10.0)),
                            ..default()
                        },
                    ));
                    for index in 0..ROWS {
                        rows.spawn((
                            MenuRow(index),
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(22.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                        ));
                    }
                    rows.spawn((
                        Text::new(
                            "up/down choose  ·  left/right change  ·  Enter select  ·  Esc back",
                        ),
                        TextFont {
                            font_size: FontSize::Px(15.0),
                            ..default()
                        },
                        TextColor(Color::srgb(0.45, 0.5, 0.6)),
                        Node {
                            margin: UiRect::top(Val::Px(14.0)),
                            ..default()
                        },
                    ));
                });
        });
}

/// What one press does, independent of which device made it.
#[derive(Default, Clone, Copy)]
struct Press {
    toggle: bool,
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    select: bool,
    back: bool,
}

fn keyboard(keys: &ButtonInput<KeyCode>) -> Press {
    Press {
        toggle: keys.just_pressed(KeyCode::Escape),
        // WASD as well as the arrows, because they are what the hand is
        // already on when the game is paused.
        up: keys.any_just_pressed([KeyCode::ArrowUp, KeyCode::KeyW]),
        down: keys.any_just_pressed([KeyCode::ArrowDown, KeyCode::KeyS]),
        left: keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::KeyA]),
        right: keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::KeyD]),
        select: keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Space]),
        // Escape is both, and which one it is depends on the page: the
        // root closes, anything deeper goes up one. `apply` decides.
        back: false,
    }
}

/// The pad's half of the same. Select -- the small button beside Start -- opens
/// it, because Start is already how the pad swaps character.
fn gamepad(pad: &Gamepad) -> Press {
    Press {
        toggle: pad.just_pressed(GamepadButton::Select),
        up: pad.just_pressed(GamepadButton::DPadUp),
        down: pad.just_pressed(GamepadButton::DPadDown),
        left: pad.just_pressed(GamepadButton::DPadLeft),
        right: pad.just_pressed(GamepadButton::DPadRight),
        select: pad.just_pressed(GamepadButton::South),
        back: pad.just_pressed(GamepadButton::East),
    }
}

impl Press {
    fn or(self, other: Press) -> Self {
        Self {
            toggle: self.toggle || other.toggle,
            up: self.up || other.up,
            down: self.down || other.down,
            left: self.left || other.left,
            right: self.right || other.right,
            select: self.select || other.select,
            back: self.back || other.back,
        }
    }
}

/// Reads the menu's keys and acts on them.
///
/// Runs in `PreUpdate` after the console, and does nothing on a frame the
/// console is open or has just shut: Escape closes the console too, and one
/// press should not both shut the console and open this.
#[allow(clippy::too_many_arguments)]
pub fn input(
    keys: Res<ButtonInput<KeyCode>>,
    pads: Query<&Gamepad>,
    console: Res<ConsoleState>,
    mut menu: ResMut<MenuState>,
    mut settings: ResMut<DisplaySettings>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    menu.closed_this_frame = false;
    if console.open || console.closed_this_frame {
        return;
    }
    let press = pads
        .iter()
        .fold(keyboard(&keys), |press, pad| press.or(gamepad(pad)));

    let was_open = menu.open;
    if press.toggle {
        // Escape inside the menu means "back", and back out of the root is
        // what shuts it. One key, one meaning, however deep you are.
        match (menu.open, menu.page.parent()) {
            (false, _) => menu.open(),
            (true, Some(parent)) => menu.go(parent),
            (true, None) => menu.close(),
        }
    } else if menu.open {
        if press.back {
            match menu.page.parent() {
                Some(parent) => menu.go(parent),
                None => menu.close(),
            }
        }
        if press.up {
            menu.move_row(-1);
        }
        if press.down {
            menu.move_row(1);
        }
        let step = i32::from(press.right) - i32::from(press.left);
        if step != 0 {
            adjust(menu.selected(), step, &mut settings, &mut windows);
        }
        if press.select {
            match menu.selected() {
                Item::Resume => menu.close(),
                Item::Options => menu.go(Page::Options),
                Item::Display => menu.go(Page::Display),
                Item::Quit => {
                    exit.write(AppExit::Success);
                }
                Item::Back => match menu.page.parent() {
                    Some(parent) => menu.go(parent),
                    None => menu.close(),
                },
                // A value row chosen rather than nudged steps forward, and
                // wraps, so Enter alone can reach every setting.
                value => adjust(value, 1, &mut settings, &mut windows),
            }
        }
    }

    if menu.open != was_open {
        // The menu is a mouse-shaped thing even though it is driven by keys:
        // a player who opens it wants their cursor back, and one who resumes
        // wants it out of the way and captured again.
        if let Ok(mut cursor) = cursor.single_mut() {
            cursor.grab_mode = if menu.open {
                CursorGrabMode::None
            } else {
                CursorGrabMode::Locked
            };
            cursor.visible = menu.open;
        }
    }
}

/// Changes a value row. Rows that are not values ignore it.
fn adjust(
    item: Item,
    step: i32,
    settings: &mut DisplaySettings,
    windows: &mut Query<&mut Window, With<PrimaryWindow>>,
) {
    match item {
        Item::RenderScale => settings.step_scale(step),
        Item::WindowMode => {
            if let Ok(mut window) = windows.single_mut() {
                window.mode = display::other_mode(window.mode);
            }
        }
        _ => {}
    }
}

/// Writes the page into the nodes [`spawn`] made.
///
/// Runs whether the menu is open or shut, because hiding it is something this
/// has to do rather than something that happens when it stops running.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn draw(
    menu: Res<MenuState>,
    settings: Res<DisplaySettings>,
    target: Res<SceneTarget>,
    images: Res<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut root: Query<&mut Visibility, With<MenuRoot>>,
    mut title: Query<&mut Text, (With<MenuTitle>, Without<MenuRow>)>,
    mut rows: Query<(&MenuRow, &mut Text, &mut TextColor), Without<MenuTitle>>,
) {
    if let Ok(mut visibility) = root.single_mut() {
        *visibility = if menu.open {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
    if !menu.open {
        return;
    }
    if let Ok(mut text) = title.single_mut() {
        **text = menu.page.title().to_string();
    }
    // The size read back out of the image rather than recomputed, so the
    // number on the screen is the texture that exists rather than the one the
    // setting asks for -- they differ for the frame after a window resize.
    let rendered = images
        .get(&target.0)
        .map(|image| {
            let size = image.texture_descriptor.size;
            UVec2::new(size.width, size.height)
        })
        .unwrap_or_default();
    let windowed = windows
        .single()
        .map(|window| display::is_windowed(window.mode))
        .unwrap_or(false);

    let items = menu.page.items();
    for (row, mut text, mut colour) in &mut rows {
        let Some(item) = items.get(row.0) else {
            **text = String::new();
            continue;
        };
        let chosen = row.0 == menu.row;
        let label = match item {
            Item::Resume => "Resume".to_string(),
            Item::Options => "Options".to_string(),
            Item::Quit => "Quit".to_string(),
            Item::Display => "Display".to_string(),
            Item::RenderScale => format!(
                "Render resolution      < {:>3}%  {} x {} >",
                settings.percent(),
                rendered.x,
                rendered.y
            ),
            Item::WindowMode => format!(
                "Window mode            < {} >",
                if windowed { "Windowed" } else { "Fullscreen" }
            ),
            Item::Back => "Back".to_string(),
        };
        **text = format!("{} {label}", if chosen { ">" } else { " " });
        *colour = TextColor(if chosen {
            Color::srgb(1.0, 0.92, 0.5)
        } else {
            Color::srgb(0.75, 0.78, 0.85)
        });
    }
}

/// Holds the animations still while the menu is up, the way the console does
/// while it is open. Without it a paused game is a paused simulation with every
/// character still walking on the spot.
pub fn pause_animations(menu: Res<MenuState>, mut players: Query<&mut AnimationPlayer>) {
    if !menu.open {
        return;
    }
    for mut player in &mut players {
        player.pause_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// A world with everything [`input`] reads and a window to grab the cursor
    /// in, and no renderer behind any of it.
    fn paused() -> World {
        let mut world = World::new();
        world.init_resource::<ButtonInput<KeyCode>>();
        world.init_resource::<ConsoleState>();
        world.init_resource::<MenuState>();
        world.init_resource::<DisplaySettings>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<Messages<AppExit>>();
        let target = crate::display::create_target(&mut world.resource_mut::<Assets<Image>>());
        world.insert_resource(SceneTarget(target));
        // `CursorOptions` comes along with `Window` as a required component,
        // which is the pair `input` reaches for when the menu opens.
        world.spawn((Window::default(), PrimaryWindow));
        world
    }

    /// One press, read once: the button is released again afterwards, the way
    /// Bevy's own input system would have on the next frame.
    fn press(world: &mut World, key: KeyCode) {
        world.resource_mut::<ButtonInput<KeyCode>>().press(key);
        world.run_system_once(input).expect("the menu did not run");
        world.resource_mut::<ButtonInput<KeyCode>>().release(key);
        world.resource_mut::<ButtonInput<KeyCode>>().clear();
    }

    fn cursor(world: &mut World) -> CursorOptions {
        world
            .query_filtered::<&CursorOptions, With<PrimaryWindow>>()
            .single(world)
            .expect("the window lost its cursor options")
            .clone()
    }

    #[test]
    fn escape_opens_the_menu_and_gives_the_mouse_back() {
        let mut world = paused();
        assert_eq!(
            cursor(&mut world).grab_mode,
            CursorGrabMode::None,
            "a fresh window in a test starts ungrabbed; the game grabs it in setup"
        );

        press(&mut world, KeyCode::Escape);
        assert!(world.resource::<MenuState>().open);
        let released = cursor(&mut world);
        assert_eq!(released.grab_mode, CursorGrabMode::None);
        assert!(released.visible, "a menu you cannot point at is not a menu");

        press(&mut world, KeyCode::Escape);
        let menu = world.resource::<MenuState>();
        assert!(!menu.open);
        assert!(
            menu.closed_this_frame,
            "the press that shut the menu must not also reach the game"
        );
        let grabbed = cursor(&mut world);
        assert_eq!(grabbed.grab_mode, CursorGrabMode::Locked);
        assert!(!grabbed.visible);
    }

    /// The whole path the request describes: Escape, into Options, into
    /// Display, and change the internal render resolution.
    #[test]
    fn the_display_page_changes_the_render_resolution() {
        let mut world = paused();
        let full = world.resource::<DisplaySettings>().percent();

        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        assert_eq!(world.resource::<MenuState>().page, Page::Options);

        press(&mut world, KeyCode::Enter);
        assert_eq!(world.resource::<MenuState>().page, Page::Display);

        press(&mut world, KeyCode::ArrowRight);
        assert_ne!(
            world.resource::<DisplaySettings>().percent(),
            full,
            "right on the render resolution row changes it"
        );
        // And the menu is still up, on the same row: changing a value is not
        // choosing it.
        let menu = world.resource::<MenuState>();
        assert!(menu.open);
        assert_eq!(menu.selected(), Item::RenderScale);
    }

    /// Escape unwinds one page per press rather than shutting the menu from
    /// wherever it happens to be.
    #[test]
    fn escape_walks_back_out_of_the_pages_it_walked_into() {
        let mut world = paused();
        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        press(&mut world, KeyCode::Enter);
        assert_eq!(world.resource::<MenuState>().page, Page::Display);

        press(&mut world, KeyCode::Escape);
        assert_eq!(world.resource::<MenuState>().page, Page::Options);
        press(&mut world, KeyCode::Escape);
        assert_eq!(world.resource::<MenuState>().page, Page::Root);
        assert!(world.resource::<MenuState>().open);
        press(&mut world, KeyCode::Escape);
        assert!(!world.resource::<MenuState>().open);
    }

    /// Escape belongs to whichever overlay is already up. The console reads it
    /// first and closes on it, and the menu must not open on the same press.
    #[test]
    fn escape_that_shuts_the_console_does_not_open_the_menu() {
        let mut world = paused();
        world.resource_mut::<ConsoleState>().closed_this_frame = true;
        press(&mut world, KeyCode::Escape);
        assert!(!world.resource::<MenuState>().open);
    }

    #[test]
    fn quit_asks_the_app_to_exit() {
        let mut world = paused();
        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowUp);
        assert_eq!(
            world.resource::<MenuState>().selected(),
            Item::Quit,
            "up from Resume wraps to the bottom of the root page"
        );
        press(&mut world, KeyCode::Enter);
        assert!(
            !world.resource::<Messages<AppExit>>().is_empty(),
            "nothing asked the game to close"
        );
    }

    #[test]
    fn every_page_has_rows_and_room_for_them() {
        for page in [Page::Root, Page::Options, Page::Display] {
            let items = page.items();
            assert!(!items.is_empty(), "{page:?} has no rows");
            assert!(
                items.len() <= ROWS,
                "{page:?} has more rows than {ROWS} nodes to write them into"
            );
        }
    }

    #[test]
    fn the_row_wraps_and_stays_in_range() {
        let mut menu = MenuState::default();
        assert_eq!(menu.selected(), Item::Resume);
        menu.move_row(-1);
        assert_eq!(menu.selected(), Item::Quit, "up from the top is the bottom");
        menu.move_row(1);
        assert_eq!(menu.selected(), Item::Resume);
    }

    #[test]
    fn changing_page_puts_the_cursor_at_the_top() {
        let mut menu = MenuState::default();
        menu.move_row(1);
        menu.go(Page::Display);
        assert_eq!(menu.row, 0);
        assert_eq!(menu.selected(), Item::RenderScale);
    }

}
