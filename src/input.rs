//! Keyboard, mouse and gamepad merged into one snapshot per rendered frame.
//!
//! This mirrors `app/gamepad.py`: the pad is polled rather than listened to,
//! once per rendered frame, and the controls whose *press* matters rather than
//! their hold are latched here instead of in the simulation. That latching is
//! the point. Gameplay runs at a fixed 30 Hz while input arrives at the render
//! rate, so a frame may contain two fixed steps or none, and `just_pressed`
//! read from inside `FixedUpdate` would fire a jump twice on a slow frame and
//! swallow it entirely on a fast one. A latched edge fires exactly once, on
//! the next fixed step, whenever that step happens to run.
//!
//! Nothing here requires a pad. With none attached every pad field reads
//! neutral and the keyboard is untouched.

use crate::{console::ConsoleState, menu::MenuState};
use bevy::{input::mouse::MouseMotion, prelude::*};

/// How far a stick leaves centre before it counts. Sticks rest a little off
/// zero and the rest wears with use. The look stick gets a tighter gate: a
/// quarter of the travel is a lot to give away on the control that aims.
pub const STICK_DEADZONE: f32 = 0.18;
pub const CAMERA_DEADZONE: f32 = 0.12;

/// Analog triggers report as an axis on some drivers and a button on others,
/// and as both on a few. Half pressed is pressed. The booster trigger sits
/// lower because it is a control you fly with: a finger resting on it should
/// not be a finger flying, but touching it should be immediate.
pub const TRIGGER_THRESHOLD: f32 = 0.5;
pub const THRUST_THRESHOLD: f32 = 0.3;

/// One frame of player intent, independent of the device that produced it.
///
/// The `bool` fields split into two kinds. Held controls (`boost`, `aim`) are
/// rewritten every frame and read freely. Latched edges (`jump`, `attack`,
/// `swap`, `debug`, `recenter`) stay set until the system that acts on them
/// calls [`InputState::take`], so no press is lost or double-counted across the
/// fixed-step boundary.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct InputState {
    /// Camera-relative movement wish: x right, y forward, length at most 1.
    pub move_axis: Vec2,
    /// Mouse motion accumulated this frame, in pixels.
    pub look_mouse: Vec2,
    /// Look stick after its deadzone, each axis -1..1, y positive is up.
    pub look_stick: Vec2,
    pub jump: bool,
    pub attack: bool,
    pub boost: bool,
    pub aim: bool,
    pub recenter: bool,
    /// The squad button, held. Its *release* is the command, and how long it
    /// was down is what tells a whistle from an order, so the falling edge is
    /// published alongside the hold.
    pub squad: bool,
    pub squad_released: bool,
    pub swap: bool,
    pub debug: bool,
    /// Whether a pad was seen this frame, for the debug readout.
    pub pad: bool,
}

impl InputState {
    /// Reads a latched edge and clears it, so it fires exactly once.
    pub fn take(flag: &mut bool) -> bool {
        std::mem::replace(flag, false)
    }

    /// Everything centred and released, keeping latched edges cleared.
    ///
    /// Called while the console or the pause menu holds the input. A direction
    /// held at that moment must not stay held forever: unlike a key, there is
    /// no release event arriving later for a stick that was pushed when the
    /// console opened.
    fn neutral(&mut self) {
        let pad = self.pad;
        *self = Self::default();
        self.pad = pad;
    }
}

/// Rescales a stick past its deadzone, keeping the direction it points.
///
/// Taken on the magnitude rather than per axis, which is what keeps the gate
/// circular: a per-axis deadzone squares off the centre and makes a gentle
/// diagonal impossible to hold.
pub fn apply_deadzone(value: Vec2, deadzone: f32) -> Vec2 {
    let magnitude = value.length();
    if magnitude <= deadzone {
        return Vec2::ZERO;
    }
    let scale = ((magnitude - deadzone) / (1.0 - deadzone)).min(1.0) / magnitude;
    value * scale
}

/// Polls every device and publishes the frame's [`InputState`].
///
/// Runs in `PreUpdate` after the console and the menu, so neither one's keys
/// reach gameplay and an overlay that is up holds the pad neutral without
/// dropping the device.
#[allow(clippy::too_many_arguments)]
pub fn gather(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut mouse: MessageReader<MouseMotion>,
    // A pad is an entity now, and it carries its own button and axis state
    // rather than being an id looked up in two global resources. Reading the
    // first one there is keeps the port's behaviour: one player, whichever pad
    // arrived first, picked up mid-game without a restart.
    pads: Query<&Gamepad>,
    console: Res<ConsoleState>,
    menu: Res<MenuState>,
    mut state: ResMut<InputState>,
) {
    // Accumulated mouse motion is per frame, never latched: a look that was
    // not consumed this frame is stale by the next one.
    let mouse_delta: Vec2 = mouse.read().map(|motion| motion.delta).sum();
    let pad = pads.iter().next();
    state.pad = pad.is_some();

    // The grave key toggles the console and Escape the menu, and neither must
    // also reach the game, so the whole snapshot goes neutral for the frame
    // either one closes on too.
    if console.open || console.closed_this_frame || menu.open || menu.closed_this_frame {
        state.neutral();
        return;
    }

    let mut move_axis = Vec2::new(
        f32::from(keys.any_pressed([KeyCode::KeyD, KeyCode::ArrowRight]))
            - f32::from(keys.any_pressed([KeyCode::KeyA, KeyCode::ArrowLeft])),
        f32::from(keys.any_pressed([KeyCode::KeyW, KeyCode::ArrowUp]))
            - f32::from(keys.any_pressed([KeyCode::KeyS, KeyCode::ArrowDown])),
    );
    let mut look_stick = Vec2::ZERO;
    let mut boost = keys.pressed(KeyCode::KeyV);
    let mut aim = keys.pressed(KeyCode::KeyF) || buttons.pressed(MouseButton::Right);
    let mut jump = keys.just_pressed(KeyCode::Space);
    let mut attack =
        keys.just_pressed(KeyCode::ShiftLeft) || buttons.just_pressed(MouseButton::Left);
    let mut recenter = keys.just_pressed(KeyCode::KeyR);
    let mut swap = keys.just_pressed(KeyCode::F2);
    let mut squad = keys.pressed(KeyCode::KeyX);
    let mut squad_released = keys.just_released(KeyCode::KeyX);

    if let Some(pad) = pad {
        let axis = |kind| pad.get(kind).unwrap_or(0.0);
        let button = |kind| pad.pressed(kind);
        let just = |kind| pad.just_pressed(kind);

        let stick = Vec2::new(
            axis(GamepadAxis::LeftStickX),
            axis(GamepadAxis::LeftStickY),
        );
        let stick = if stick == Vec2::ZERO {
            // The d-pad stands in for the stick when the stick is centred, at
            // full deflection: it is the same control the arrow keys are, and
            // it is what a player reaches for to line up a jump.
            Vec2::new(
                f32::from(button(GamepadButton::DPadRight))
                    - f32::from(button(GamepadButton::DPadLeft)),
                f32::from(button(GamepadButton::DPadUp))
                    - f32::from(button(GamepadButton::DPadDown)),
            )
        } else {
            apply_deadzone(stick, STICK_DEADZONE)
        };
        if stick != Vec2::ZERO {
            move_axis = stick;
        }
        look_stick = apply_deadzone(
            Vec2::new(
                axis(GamepadAxis::RightStickX),
                axis(GamepadAxis::RightStickY),
            ),
            CAMERA_DEADZONE,
        );

        // Face buttons follow the Panda3D mapping: south jumps, east swings,
        // north holds the skates. The port drives skating and flight from one
        // control, so the booster trigger feeds the same flag as north.
        jump |= just(GamepadButton::South);
        attack |= just(GamepadButton::East);
        swap |= just(GamepadButton::Start);
        recenter |= just(GamepadButton::RightTrigger);
        squad |= button(GamepadButton::West);
        squad_released |= pad.just_released(GamepadButton::West);
        boost |= button(GamepadButton::North)
            || button(GamepadButton::LeftTrigger2)
            || axis(GamepadAxis::LeftZ).abs() > THRUST_THRESHOLD;
        aim |= button(GamepadButton::RightTrigger2)
            || axis(GamepadAxis::RightZ).abs() > TRIGGER_THRESHOLD;
    }

    state.move_axis = move_axis.clamp_length_max(1.0);
    state.look_mouse = mouse_delta;
    state.look_stick = look_stick;
    state.boost = boost;
    state.aim = aim;
    state.squad = squad;
    // Edges accumulate rather than overwrite: a press seen on a frame whose
    // consumer has not run yet must survive into the frame that consumes it.
    state.jump |= jump;
    state.attack |= attack;
    state.recenter |= recenter;
    state.squad_released |= squad_released;
    state.swap |= swap;
    state.debug |= keys.just_pressed(KeyCode::F1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_gates_and_rescales_from_zero() {
        assert_eq!(
            apply_deadzone(Vec2::new(0.1, 0.0), STICK_DEADZONE),
            Vec2::ZERO
        );
        // Just past the gate the stick reports a small value rather than
        // stepping straight to the deadzone magnitude, so a slow walk exists.
        let nudge = apply_deadzone(Vec2::new(0.2, 0.0), STICK_DEADZONE);
        assert!(nudge.x > 0.0 && nudge.x < 0.05, "{nudge:?}");
        // Full deflection stays full.
        let full = apply_deadzone(Vec2::new(1.0, 0.0), STICK_DEADZONE);
        assert!((full.x - 1.0).abs() < 1e-6, "{full:?}");
    }

    #[test]
    fn deadzone_is_circular_and_keeps_direction() {
        let diagonal = Vec2::new(0.5, 0.5);
        let gated = apply_deadzone(diagonal, STICK_DEADZONE);
        assert!(gated.length() > 0.0);
        // A per-axis gate would have shortened one axis more than the other.
        assert!((gated.x - gated.y).abs() < 1e-6, "{gated:?}");
        assert!(
            gated.normalize().dot(diagonal.normalize()) > 0.999,
            "{gated:?}"
        );
        // Over-deflection past the corner of the box clamps to unit length.
        assert!(apply_deadzone(Vec2::new(1.0, 1.0), STICK_DEADZONE).length() <= 1.0 + 1e-6);
    }

    #[test]
    fn latched_edge_fires_once() {
        let mut state = InputState {
            jump: true,
            ..default()
        };
        assert!(InputState::take(&mut state.jump));
        assert!(!InputState::take(&mut state.jump));
    }

    #[test]
    fn going_neutral_drops_held_directions_and_pending_edges() {
        let mut state = InputState {
            move_axis: Vec2::new(1.0, 0.0),
            boost: true,
            jump: true,
            pad: true,
            ..default()
        };
        state.neutral();
        assert_eq!(state.move_axis, Vec2::ZERO);
        assert!(!state.boost && !state.jump);
        // The device itself is still attached; only its state was cleared.
        assert!(state.pad);
    }
}
