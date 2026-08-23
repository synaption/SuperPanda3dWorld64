//! The pause menu: Escape, and what is behind it.
//!
//! Two pages deep and no deeper -- the root, the level list, its options and
//! the display settings -- because a game with one screen of settings does not
//! need a tree and a menu you can get lost in is worse than no menu. Each page
//! is a list of rows; a row either does something when it is chosen or holds a
//! value that left and right change.
//!
//! The level list is the one page that does more than set a number. Choosing a
//! row there takes the world down and puts another one up, which
//! [`crate::world`] does; all this page contributes is the choice, and the
//! patience to stay up while a level that cannot arrive in one frame arrives.
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
    world::{LevelId, LevelLoad, LoadLevel},
};
use bevy::{
    app::AppExit,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

/// What the line under the rows says when there is nothing wrong.
const CONTROLS: &str = "mouse or up/down choose  ·  click/Enter select  ·  Esc back";

/// How many text rows are spawned for the menu to write into.
///
/// The longest page is the root's four, and two spare cost two text nodes that
/// are hidden on every page rather than a page's worth of respawning. The
/// spares are what the level list grows into: it is the one page whose length
/// is a property of the game rather than of the menu.
const ROWS: usize = 6;

/// Which page is showing.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    #[default]
    Root,
    Levels,
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
    Levels,
    Options,
    Quit,
    /// One row per level. The level travels in the row rather than the row
    /// being the nth of a list, for the same reason animation clips are chosen
    /// by name: an index shifts the moment a level is added between two others.
    Level(LevelId),
    Display,
    RenderScale,
    WindowMode,
    Back,
}

/// The level page's rows: one per level the game has, then a way back.
///
/// Built out of [`LevelId::ALL`] rather than written out, so a level added to
/// the catalogue appears in the menu without anyone having to remember this
/// list exists. `items` has to hand back a `&'static [Item]`, which is why this
/// is a const rather than something built when the page is drawn.
const LEVEL_ROWS: [Item; LevelId::ALL.len() + 1] = {
    let mut rows = [Item::Back; LevelId::ALL.len() + 1];
    let mut at = 0;
    while at < LevelId::ALL.len() {
        rows[at] = Item::Level(LevelId::ALL[at]);
        at += 1;
    }
    rows
};

impl Page {
    fn items(self) -> &'static [Item] {
        match self {
            Page::Root => &[Item::Resume, Item::Levels, Item::Options, Item::Quit],
            Page::Levels => &LEVEL_ROWS,
            Page::Options => &[Item::Display, Item::Back],
            Page::Display => &[Item::RenderScale, Item::WindowMode, Item::Back],
        }
    }

    fn title(self) -> &'static str {
        match self {
            Page::Root => "PAUSED",
            Page::Levels => "LEVEL",
            Page::Options => "OPTIONS",
            Page::Display => "DISPLAY",
        }
    }

    /// Where Escape, or a row called Back, goes from here. The root has
    /// nowhere left to go, which is what shuts the menu.
    fn parent(self) -> Option<Page> {
        match self {
            Page::Root => None,
            Page::Levels => Some(Page::Root),
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
    /// The level the player has chosen and is waiting for.
    ///
    /// The menu stays up until it arrives, and swallows every key while it is
    /// waiting. That is not politeness: the planet's collision is read out of
    /// its glTF over however many frames that takes, and the menu being open is
    /// what holds the simulation still meanwhile. Close it early and the player
    /// spends the load falling through a world with no ground in it yet.
    wanted: Option<LevelId>,
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
        self.wanted = None;
        self.go(Page::Root);
    }

    /// Whether the menu is holding the world still waiting for a level.
    pub fn loading(&self) -> Option<LevelId> {
        self.wanted
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

/// The line under the rows. Usually the controls; a level that would not load
/// borrows it to say why, because a reason nobody can read is not a reason.
#[derive(Component)]
pub struct MenuHint;

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
                            Interaction::default(),
                            Text::new(""),
                            TextFont {
                                font_size: FontSize::Px(22.0),
                                ..default()
                            },
                            TextColor(Color::WHITE),
                            // Give every label a full-width, comfortably tall
                            // hit target instead of making only its glyphs
                            // clickable.
                            Node {
                                width: Val::Percent(100.0),
                                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                                ..default()
                            },
                        ));
                    }
                    rows.spawn((
                        MenuHint,
                        Text::new(CONTROLS),
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
    level: Res<LevelId>,
    mut load: ResMut<LevelLoad>,
    mut levels: MessageWriter<LoadLevel>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    interactions: Query<(&MenuRow, &Interaction), Changed<Interaction>>,
    mut exit: MessageWriter<AppExit>,
) {
    menu.closed_this_frame = false;
    if console.open || console.closed_this_frame {
        return;
    }
    let was_open = menu.open;
    // A level that has been asked for owns the menu until it is up. When it
    // arrives the menu gets out of the way of it, and the block at the bottom
    // takes the cursor back with it exactly as if Resume had been chosen.
    //
    // "Nothing is loading any more" rather than "the level asked for is up",
    // because those come apart on the path that matters: a planet whose glTF
    // will not load puts the castle back instead, and a menu waiting for the
    // planet would sit over it for ever.
    if menu.wanted.is_some() {
        if !load.busy() {
            // A level that did not arrive leaves the menu open on the list it
            // was chosen from, with the reason on it. Shutting the menu on a
            // failure is what makes a broken level look like a menu row that
            // does nothing at all.
            if load.trouble().is_some() {
                menu.wanted = None;
                menu.go(Page::Levels);
            } else {
                menu.close();
            }
        }
        if menu.open != was_open {
            release_cursor(menu.open, &mut cursor);
        }
        return;
    }
    let press = pads
        .iter()
        .fold(keyboard(&keys), |press, pad| press.or(gamepad(pad)));

    if press.toggle {
        // Escape inside the menu means "back", and back out of the root is
        // what shuts it. One key, one meaning, however deep you are.
        match (menu.open, menu.page.parent()) {
            (false, _) => menu.open(),
            (true, Some(parent)) => menu.go(parent),
            (true, None) => menu.close(),
        }
    } else if menu.open {
        // Hover follows the pointer immediately; pressing a row performs the
        // same operation as choosing it with Enter. `Interaction::Pressed`
        // is an edge here because the query only contains changed values.
        let mut clicked = false;
        for (row, interaction) in &interactions {
            if row.0 >= menu.page.items().len() {
                continue;
            }
            if matches!(interaction, Interaction::Hovered | Interaction::Pressed) {
                menu.row = row.0;
            }
            clicked |= *interaction == Interaction::Pressed;
        }
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
        if press.select || clicked {
            match menu.selected() {
                Item::Resume => menu.close(),
                Item::Levels => menu.go(Page::Levels),
                // Choosing the level already up is choosing to get on with it.
                Item::Level(id) if id == *level && !load.busy() => menu.close(),
                Item::Level(id) => {
                    levels.write(LoadLevel(id));
                    menu.wanted = Some(id);
                }
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
        release_cursor(menu.open, &mut cursor);
        // The complaint is about the last attempt, not a standing state of the
        // menu: leaving takes it with you rather than having it waiting the
        // next time Escape is pressed.
        if !menu.open {
            load.failed = None;
        }
    }
}

/// The menu is a mouse-shaped thing even though it is driven by keys: a player
/// who opens it wants their cursor back, and one who resumes wants it out of
/// the way and captured again.
fn release_cursor(open: bool, cursor: &mut Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut cursor) = cursor.single_mut() {
        cursor.grab_mode = if open {
            CursorGrabMode::None
        } else {
            CursorGrabMode::Locked
        };
        cursor.visible = open;
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
    level: Res<LevelId>,
    load: Res<LevelLoad>,
    target: Res<SceneTarget>,
    images: Res<Assets<Image>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut root: Query<&mut Visibility, With<MenuRoot>>,
    mut title: Query<&mut Text, (With<MenuTitle>, Without<MenuRow>, Without<MenuHint>)>,
    mut hint: Query<(&mut Text, &mut TextColor), (With<MenuHint>, Without<MenuRow>, Without<MenuTitle>)>,
    mut rows: Query<(&MenuRow, &mut Text, &mut TextColor), (Without<MenuTitle>, Without<MenuHint>)>,
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
        **text = match (menu.loading(), load.trouble()) {
            (Some(id), _) => format!("LOADING {}", id.name().to_uppercase()),
            (None, Some(_)) if menu.page == Page::Levels => "LEVEL -- NOT LOADED".to_string(),
            (None, _) => menu.page.title().to_string(),
        };
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

    if let Ok((mut text, mut colour)) = hint.single_mut() {
        match load.trouble() {
            Some(trouble) if menu.loading().is_none() => {
                **text = trouble.to_string();
                *colour = TextColor(Color::srgb(1.0, 0.55, 0.45));
            }
            _ => {
                **text = CONTROLS.to_string();
                *colour = TextColor(Color::srgb(0.45, 0.5, 0.6));
            }
        }
    }
    let items: &[Item] = if menu.loading().is_some() {
        &[]
    } else {
        menu.page.items()
    };
    for (row, mut text, mut colour) in &mut rows {
        let Some(item) = items.get(row.0) else {
            **text = String::new();
            continue;
        };
        let chosen = row.0 == menu.row;
        let label = match item {
            Item::Resume => "Resume".to_string(),
            Item::Levels => "Level".to_string(),
            // A tick beside the one being played, because a list of levels with
            // nothing marked is a list that does not say where you are.
            Item::Level(id) => format!(
                "{} {}",
                if *id == *level { "*" } else { " " },
                id.name()
            ),
            Item::Options => "Options".to_string(),
            Item::Quit => "Quit".to_string(),
            Item::Display => "Display".to_string(),
            Item::RenderScale => format!(
                "World pixels           < {}x{}  {} x {} >",
                settings.pixel_scale(),
                settings.pixel_scale(),
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
        world.init_resource::<LevelId>();
        world.init_resource::<LevelLoad>();
        world.init_resource::<Messages<LoadLevel>>();
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
        let full = world.resource::<DisplaySettings>().pixel_scale();

        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        assert_eq!(world.resource::<MenuState>().page, Page::Options);

        press(&mut world, KeyCode::Enter);
        assert_eq!(world.resource::<MenuState>().page, Page::Display);

        press(&mut world, KeyCode::ArrowRight);
        assert_ne!(
            world.resource::<DisplaySettings>().pixel_scale(),
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
        for page in [Page::Root, Page::Levels, Page::Options, Page::Display] {
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

    #[test]
    fn hovering_and_clicking_rows_drives_the_menu() {
        let mut world = paused();
        world.resource_mut::<MenuState>().open();

        let hover = world.spawn((MenuRow(3), Interaction::Hovered)).id();
        world.run_system_once(input).expect("the menu did not run");
        assert_eq!(world.resource::<MenuState>().selected(), Item::Quit);

        // Move the same pointer target to Options and press it. The click
        // should select that row before activating it, even though Quit was
        // selected on the previous frame.
        world.entity_mut(hover).insert(MenuRow(2));
        world.entity_mut(hover).insert(Interaction::Pressed);
        world.run_system_once(input).expect("the menu did not run");
        assert_eq!(world.resource::<MenuState>().page, Page::Options);
    }

    /// Every level the game has is reachable from the menu. The page lists them
    /// by hand, which is the sort of list that quietly falls one behind.
    #[test]
    fn the_level_page_offers_every_level() {
        let listed: Vec<LevelId> = Page::Levels
            .items()
            .iter()
            .filter_map(|item| match item {
                Item::Level(id) => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(listed, LevelId::ALL.to_vec());
    }

    /// The path the request describes: Escape, into the level list, choose the
    /// planet. The menu asks for it and then holds the world still, because
    /// nothing has been loaded yet.
    #[test]
    fn choosing_a_level_asks_for_it_and_waits() {
        let mut world = paused();
        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        assert_eq!(world.resource::<MenuState>().page, Page::Levels);

        press(&mut world, KeyCode::ArrowDown);
        assert_eq!(
            world.resource::<MenuState>().selected(),
            Item::Level(LevelId::Planet)
        );
        press(&mut world, KeyCode::Enter);
        // The switch has not run yet, so nothing is loading and nothing has
        // changed: the menu must not read that as the planet having arrived.
        world.resource_mut::<LevelLoad>().pending = Some(LevelId::Planet);
        let asked: Vec<LevelId> = world
            .resource_mut::<Messages<LoadLevel>>()
            .drain()
            .map(|LoadLevel(id)| id)
            .collect();
        assert_eq!(asked, vec![LevelId::Planet], "nothing asked for the planet");
        let menu = world.resource::<MenuState>();
        assert!(menu.open, "the menu let go before the level arrived");
        assert_eq!(menu.loading(), Some(LevelId::Planet));

        // Escape does not get you out of a load, because the world behind the
        // menu has no ground in it yet.
        press(&mut world, KeyCode::Escape);
        assert!(world.resource::<MenuState>().open);

        // The level arrives; the menu gets out of the way of it and takes the
        // cursor with it.
        world.insert_resource(LevelLoad::default());
        world.insert_resource(LevelId::Planet);
        world.run_system_once(input).expect("the menu did not run");
        let menu = world.resource::<MenuState>();
        assert!(!menu.open, "the menu stayed up over a level that had arrived");
        assert_eq!(menu.loading(), None);
        assert_eq!(cursor(&mut world).grab_mode, CursorGrabMode::Locked);
    }

    /// A level that will not load leaves the menu open on the list, so the row
    /// the player chose does not merely appear to do nothing.
    ///
    /// This is the exact shape of the bug that shipped: the packaged Windows
    /// build had the planet in its menu and no glTF for it, the load failed,
    /// the castle went back up and the menu shut as though nothing had
    /// happened.
    #[test]
    fn a_level_that_will_not_load_says_so_instead_of_shutting_the_menu() {
        let mut world = paused();
        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        world.resource_mut::<LevelLoad>().pending = Some(LevelId::Planet);
        assert_eq!(world.resource::<MenuState>().loading(), Some(LevelId::Planet));

        // The load gives up and the castle stays where it was.
        {
            let mut load = world.resource_mut::<LevelLoad>();
            load.pending = None;
            load.failed = Some("bevy/planet.glb did not load".into());
        }
        world.run_system_once(input).expect("the menu did not run");
        let menu = world.resource::<MenuState>();
        assert!(menu.open, "the menu shut on a level that never arrived");
        assert_eq!(menu.page, Page::Levels, "and it shut the list too");
        assert_eq!(menu.loading(), None);

        // And the list is usable again: the row can be chosen a second time
        // rather than the menu being stuck on the complaint.
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        let menu = world.resource::<MenuState>();
        assert!(menu.open);
        assert_eq!(menu.loading(), Some(LevelId::Planet));
    }

    /// Choosing the level already being played is choosing to get on with it,
    /// rather than reloading the world out from under the player.
    #[test]
    fn choosing_the_level_you_are_on_just_resumes() {
        let mut world = paused();
        press(&mut world, KeyCode::Escape);
        press(&mut world, KeyCode::ArrowDown);
        press(&mut world, KeyCode::Enter);
        press(&mut world, KeyCode::Enter);
        assert!(!world.resource::<MenuState>().open);
        assert!(world.resource_mut::<Messages<LoadLevel>>().is_empty());
    }
}
