//! What the X button does, and the Tab menu that decides.
//!
//! The game grew four things worth doing with one hand -- commanding the
//! squad, planting a mast, putting up a stellarator, and sweeping up what a
//! fight left on the ground -- and bound the first three to three different
//! keys as each arrived. That works at a desk with a keyboard and does not
//! work anywhere else: a pad has one comfortable face button left, and a player
//! who has to remember which of B, G and X is which is a player reading a
//! manual rather than playing.
//!
//! So there is **one** action button, and a picker that says what it is aimed
//! at. Tab opens the picker, left and right walk it, and Tab, Enter or Escape
//! close it; the number keys jump straight to one. The old keys still work --
//! `B` still builds and `G` still plants, whatever the picker says -- because
//! taking a shortcut away from somebody who has learned it buys nothing.
//!
//! **Four modes rather than the three that were asked for**, and the extra one
//! is not a liberty: "builds buildings" is two buildings, a mast and a machine,
//! which are laid out differently, cost differently and are wanted at different
//! moments. One button cannot ask which without a second gesture, and a second
//! gesture is the thing this module exists to remove.
//!
//! Nothing here decides what any mode *does*. It routes one held button and one
//! release onto whichever pair of flags in [`crate::input::InputState`] the
//! chosen mode already reads, so [`crate::squad::whistle`],
//! [`crate::pylon::place`] and [`crate::stellarator::place`] are untouched and
//! do not know a picker exists.
//!
//! The picker is read straight off the keyboard rather than out of
//! `InputState`, the way [`crate::menu`] is and for the same reason: that
//! snapshot is player intent and goes neutral while the console is open, which
//! is exactly when a stray Tab must not change what the game is aimed at.

use bevy::prelude::*;

use crate::{console::ConsoleState, input::InputState, menu::MenuState};

/// What the action button is pointed at.
///
/// Ordered as the picker draws them, and that order is a claim about how often
/// each is wanted: the squad first because it is the one that is always
/// useful, then the two buildings in the order a base gets built, then the
/// broom.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Whistle the squad up, and send it somewhere. See [`crate::squad`].
    #[default]
    Squad,
    /// Plant a mast. See [`crate::pylon::place`].
    Pylon,
    /// Put up a machine. See [`crate::stellarator::place`].
    Stellarator,
    /// Open a circle on the ground and call every loose ball in it to heel.
    /// See [`crate::nuclonium::call`].
    Nuclonium,
    /// Lock the flight computer onto the body under the crosshair. Only
    /// means anything over the solar system, which is exactly why it lives
    /// in the picker rather than on a key of its own: on every other level
    /// it is a row you never select. See [`crate::autopilot`].
    Autopilot,
}

impl Mode {
    /// Every mode, in picker order.
    pub const ALL: [Mode; 5] = [
        Mode::Squad,
        Mode::Pylon,
        Mode::Stellarator,
        Mode::Nuclonium,
        Mode::Autopilot,
    ];

    /// What the picker calls it.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Squad => "Squad",
            Mode::Pylon => "Pylon",
            Mode::Stellarator => "Stellarator",
            Mode::Nuclonium => "Nuclonium",
            Mode::Autopilot => "Autopilot",
        }
    }

    /// What holding the button does, in the picker's own words.
    ///
    /// Written as the *verb*, because the thing a player wants to know while
    /// the picker is open is what will happen when they press the button, not
    /// what the mode is called.
    pub fn hint(self) -> &'static str {
        match self {
            Mode::Squad => "tap sends the squad, hold gathers one",
            Mode::Pylon => "hold to open a site, release to plant a mast",
            Mode::Stellarator => "hold to aim, release to build a machine",
            Mode::Nuclonium => "hold to open a circle, release to call what is in it",
            Mode::Autopilot => "aim at a body and press to lock on; empty sky lets go",
        }
    }

    /// The one after this, wrapping. Used by both arrows, one of them backwards.
    fn step(self, forward: bool) -> Mode {
        let at = Mode::ALL.iter().position(|mode| *mode == self).unwrap_or(0);
        let count = Mode::ALL.len();
        Mode::ALL[match forward {
            true => (at + 1) % count,
            false => (at + count - 1) % count,
        }]
    }
}

/// What the action button is aimed at, and whether the picker is up.
#[derive(Resource, Default)]
pub struct Action {
    pub mode: Mode,
    /// Whether the picker is showing. The game keeps running underneath it:
    /// this is a two-second decision, not a pause.
    pub picking: bool,
}

/// One frame of picker input, so the keys and the decision are separable.
///
/// The same shape [`crate::menu`] uses, and for the same reason -- what the
/// keys were is a fact about a keyboard, and what they mean is a fact about a
/// picker, and a test wants to write down the second without owning the first.
#[derive(Default, Clone, Copy, Debug)]
pub struct Press {
    pub toggle: bool,
    pub left: bool,
    pub right: bool,
    pub close: bool,
    /// A mode chosen outright by its number, if one was.
    pub direct: Option<Mode>,
}

/// What one press does to the picker.
///
/// Pure, so every rule about opening, walking and closing it can be asserted
/// without a window. Choosing a mode by number both selects it *and* closes,
/// because a player who knows the number does not want a second keystroke.
pub fn apply(action: &mut Action, press: Press) {
    if let Some(mode) = press.direct {
        action.mode = mode;
        action.picking = false;
        return;
    }
    if press.toggle {
        action.picking = !action.picking;
        return;
    }
    if !action.picking {
        return;
    }
    if press.close {
        action.picking = false;
    }
    if press.left {
        action.mode = action.mode.step(false);
    }
    if press.right {
        action.mode = action.mode.step(true);
    }
}

/// Reads the keyboard for the picker.
///
/// Left and right only. Up and down are deliberately not bound: this is a row,
/// and a row that also answered to the keys that walk a column would be a
/// picker that moves when somebody is trying to walk backwards with the menu
/// shut.
fn keyboard(keys: &ButtonInput<KeyCode>) -> Press {
    let numbers = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
    ];
    Press {
        toggle: keys.just_pressed(KeyCode::Tab),
        left: keys.just_pressed(KeyCode::ArrowLeft),
        right: keys.just_pressed(KeyCode::ArrowRight),
        close: keys.any_just_pressed([KeyCode::Enter, KeyCode::NumpadEnter, KeyCode::Escape]),
        direct: numbers
            .iter()
            .position(|key| keys.just_pressed(*key))
            .and_then(|at| Mode::ALL.get(at).copied()),
    }
}

/// Opens, walks and closes the picker.
///
/// Does nothing at all while the console or the pause menu is up, which is what
/// stops a Tab typed into the console from re-aiming the game.
pub fn choose(
    keys: Res<ButtonInput<KeyCode>>,
    console: Res<ConsoleState>,
    menu: Res<MenuState>,
    mut action: ResMut<Action>,
) {
    if console.open || menu.open {
        return;
    }
    apply(&mut action, keyboard(&keys));
}

/// Points the action button at whatever the picker settled on.
///
/// Runs after the input snapshot is taken and before anything reads it. The
/// button arrives as one held flag and one release; what leaves is the pair the
/// chosen mode's own system already reads, so nothing downstream changed.
///
/// The flags are `|=` rather than `=`, which is what leaves `B` and `G` working
/// as they always did: the direct keys have already written themselves in, and
/// this only ever adds.
pub fn route(action: Res<Action>, mut input: ResMut<InputState>) {
    aim(action.mode, action.picking, &mut input);
}

/// Where one frame of the action button lands. The whole of [`route`], as a
/// function of its arguments, so the wiring can be asserted without a world.
pub fn aim(mode: Mode, picking: bool, input: &mut InputState) {
    let (held, released) = (input.action, input.action_released);
    input.action = false;
    input.action_released = false;
    // While the picker is open the button is inert. Pressing it to choose a
    // mode and having that same press fire the mode you were leaving is the
    // one way a picker like this goes wrong.
    if picking {
        return;
    }
    match mode {
        Mode::Squad => {
            input.squad |= held;
            input.squad_released |= released;
        }
        Mode::Pylon => {
            input.pylon |= held;
            input.pylon_released |= released;
        }
        Mode::Stellarator => {
            input.build |= held;
            input.build_released |= released;
        }
        // The same hold-and-release the other three are: the hold opens a
        // circle on the ground and grows it, and the release takes whatever is
        // standing inside it.
        Mode::Nuclonium => {
            input.grab |= held;
            input.grab_released |= released;
        }
        Mode::Autopilot => {
            input.autopilot |= held;
            input.autopilot_released |= released;
        }
    }
}

/// The mode readout, and the picker's rows.
#[derive(Component)]
pub struct Readout;

#[derive(Component)]
pub struct Row(usize);

/// Puts the readout and one row per mode on the screen, once.
///
/// Spawned rather than drawn, and rewritten rather than respawned, the way the
/// console and the menu are: a picker that spawns is a picker that flickers for
/// a frame every time it opens.
/// Which corner the picker lives in, in pixels from the edges.
///
/// Bottom *right*. The left is taken: `health::spawn` stacks Luna's bar and
/// `energy::spawn` puts the jetpack's under it, both anchored to the bottom
/// left corner, and a mode readout drawn over the top of them is a readout
/// nobody can read and a bar nobody can either.
const CORNER: f32 = 16.0;

pub fn spawn(commands: &mut Commands) {
    commands.spawn((
        Readout,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::srgb(0.75, 0.9, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            right: Val::Px(CORNER),
            bottom: Val::Px(CORNER),
            ..default()
        },
    ));
    for (index, _) in Mode::ALL.iter().enumerate() {
        commands.spawn((
            Row(index),
            Text::new(""),
            TextFont {
                font_size: FontSize::Px(20.0),
                ..default()
            },
            TextColor(Color::WHITE),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(CORNER),
                bottom: Val::Px(CORNER + 30.0 + 24.0 * (Mode::ALL.len() - 1 - index) as f32),
                ..default()
            },
            Visibility::Hidden,
        ));
    }
}

/// Writes the readout and the rows.
pub fn draw(
    action: Res<Action>,
    mut readout: Query<&mut Text, (With<Readout>, Without<Row>)>,
    mut rows: Query<(&Row, &mut Text, &mut TextColor, &mut Visibility)>,
) {
    if let Ok(mut text) = readout.single_mut() {
        **text = match action.picking {
            true => "Tab or Enter closes  ·  left/right chooses".to_string(),
            false => format!("X: {}   (Tab)", action.mode.name()),
        };
    }
    for (row, mut text, mut colour, mut visible) in &mut rows {
        let Some(mode) = Mode::ALL.get(row.0).copied() else {
            continue;
        };
        *visible = match action.picking {
            true => Visibility::Inherited,
            false => Visibility::Hidden,
        };
        let chosen = mode == action.mode;
        **text = format!(
            "{} {}. {} — {}",
            match chosen {
                true => ">",
                false => " ",
            },
            row.0 + 1,
            mode.name(),
            mode.hint()
        );
        colour.0 = match chosen {
            true => Color::srgb(1.0, 0.95, 0.55),
            false => Color::srgb(0.72, 0.72, 0.72),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(press: Press) -> Press {
        press
    }

    #[test]
    fn tab_opens_the_picker_and_tab_shuts_it() {
        let mut action = Action::default();
        apply(
            &mut action,
            press(Press {
                toggle: true,
                ..Press::default()
            }),
        );
        assert!(action.picking);
        apply(
            &mut action,
            press(Press {
                toggle: true,
                ..Press::default()
            }),
        );
        assert!(!action.picking);
    }

    #[test]
    fn the_arrows_walk_the_row_and_wrap_both_ways() {
        let mut action = Action {
            picking: true,
            ..Action::default()
        };
        let step = |action: &mut Action, forward: bool| {
            apply(
                action,
                Press {
                    left: !forward,
                    right: forward,
                    ..Press::default()
                },
            )
        };
        for wanted in Mode::ALL.iter().skip(1).chain(Mode::ALL.first()) {
            step(&mut action, true);
            assert_eq!(action.mode, *wanted);
        }
        // Back the other way off the first mode, which is the wrap that a
        // `- 1` on an index gets wrong.
        step(&mut action, false);
        assert_eq!(action.mode, *Mode::ALL.last().unwrap());
    }

    #[test]
    fn the_arrows_do_nothing_while_the_picker_is_shut() {
        let mut action = Action::default();
        apply(
            &mut action,
            Press {
                right: true,
                ..Press::default()
            },
        );
        assert_eq!(action.mode, Mode::Squad, "the row moved with nothing up");
    }

    #[test]
    fn a_number_chooses_and_closes_in_one_press() {
        let mut action = Action {
            picking: true,
            ..Action::default()
        };
        apply(
            &mut action,
            Press {
                direct: Some(Mode::Nuclonium),
                ..Press::default()
            },
        );
        assert_eq!(action.mode, Mode::Nuclonium);
        assert!(!action.picking, "choosing by number left the picker up");
    }

    /// The rows reach the screen, and they say which one is chosen.
    ///
    /// Headless, because everything about a menu that can be got wrong is a
    /// string and a visibility rather than a pixel: the wrong row marked, a
    /// picker that stays up when it is shut, a readout that never says what the
    /// button is aimed at.
    #[test]
    fn the_picker_draws_the_row_it_is_on_and_hides_when_it_is_shut() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(Action {
            mode: Mode::Stellarator,
            picking: true,
        });
        world
            .run_system_once(|mut commands: Commands| spawn(&mut commands))
            .expect("the picker would not spawn");
        world.run_system_once(draw).expect("draw would not run");

        let mut rows = world.query::<(&Row, &Text, &Visibility)>();
        let drawn: Vec<(usize, String, Visibility)> = rows
            .iter(&world)
            .map(|(row, text, visible)| (row.0, text.0.clone(), *visible))
            .collect();
        assert_eq!(drawn.len(), Mode::ALL.len(), "a mode has no row");
        for (index, line, visible) in &drawn {
            assert_ne!(*visible, Visibility::Hidden, "a row was drawn hidden");
            let mode = Mode::ALL[*index];
            assert!(line.contains(mode.name()), "row {index} says {line:?}");
            // Exactly the chosen one is marked, and it is the one the picker is
            // on rather than the first one drawn.
            assert_eq!(
                line.starts_with('>'),
                mode == Mode::Stellarator,
                "row {index} marked wrongly: {line:?}"
            );
        }

        // Shut it, and the rows go away while the readout starts naming the
        // mode -- which is the only thing on the screen when nothing is up.
        world.resource_mut::<Action>().picking = false;
        world.run_system_once(draw).expect("draw would not run");
        let mut rows = world.query_filtered::<&Visibility, With<Row>>();
        assert!(rows.iter(&world).all(|seen| *seen == Visibility::Hidden));
        let mut readout = world.query_filtered::<&Text, With<Readout>>();
        let line = readout.single(&world).unwrap().0.clone();
        assert!(
            line.contains(Mode::Stellarator.name()),
            "the readout does not say what X does: {line:?}"
        );
    }

    /// Every mode reaches exactly one pair of flags, and the picker being up
    /// reaches none of them.
    #[test]
    fn the_button_goes_where_the_picker_points_it() {
        let pressed = |mode: Mode, picking: bool| {
            let mut input = InputState {
                action: true,
                action_released: true,
                ..InputState::default()
            };
            aim(mode, picking, &mut input);
            input
        };
        assert!(pressed(Mode::Squad, false).squad);
        assert!(pressed(Mode::Pylon, false).pylon);
        assert!(pressed(Mode::Stellarator, false).build);
        assert!(pressed(Mode::Nuclonium, false).grab);
        assert!(pressed(Mode::Autopilot, false).autopilot_released);
        // Nothing leaks across.
        assert!(!pressed(Mode::Pylon, false).squad);
        assert!(!pressed(Mode::Squad, false).grab);
        assert!(!pressed(Mode::Nuclonium, false).autopilot_released);
        // And the press that closes the picker does not also fire.
        let inert = pressed(Mode::Nuclonium, true);
        assert!(!inert.grab && !inert.squad && !inert.build && !inert.pylon);
        assert!(!pressed(Mode::Autopilot, true).autopilot_released);
    }
}
