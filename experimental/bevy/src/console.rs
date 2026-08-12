//! In-game command console and live gameplay tuning.
//!
//! Command parsing and value clamping live independently of Bevy UI, making
//! the part that can change gameplay deterministic and headless-testable.

use crate::{
    enemy::Enemy,
    player::{Controller, Player},
    GameState,
};
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    input::mouse::{MouseScrollUnit, MouseWheel},
    prelude::*,
    window::ReceivedCharacter,
};
use std::collections::VecDeque;

const LOG_LIMIT: usize = 200;

#[derive(Clone, Copy)]
pub struct TunableSpec {
    pub name: &'static str,
    pub low: f32,
    pub high: f32,
    pub step: f32,
    pub doc: &'static str,
}

pub const SPECS: &[TunableSpec] = &[
    TunableSpec {
        name: "hero_speed",
        low: 1.0,
        high: 30.0,
        step: 0.2,
        doc: "Hero running speed",
    },
    TunableSpec {
        name: "mario_speed",
        low: 1.0,
        high: 30.0,
        step: 0.2,
        doc: "Mario running speed",
    },
    TunableSpec {
        name: "walk_accel",
        low: 1.0,
        high: 60.0,
        step: 0.5,
        doc: "ground acceleration",
    },
    TunableSpec {
        name: "decel",
        low: 0.0,
        high: 30.0,
        step: 0.5,
        doc: "ground deceleration",
    },
    TunableSpec {
        name: "jump_velocity",
        low: 1.0,
        high: 30.0,
        step: 0.2,
        doc: "jump take-off speed",
    },
    TunableSpec {
        name: "skate_speed",
        low: 1.0,
        high: 40.0,
        step: 0.2,
        doc: "Hero skate speed",
    },
    TunableSpec {
        name: "skate_accel",
        low: 0.1,
        high: 30.0,
        step: 0.2,
        doc: "Hero skate acceleration",
    },
    TunableSpec {
        name: "jet_thrust",
        low: 0.0,
        high: 8.0,
        step: 0.1,
        doc: "booster lift per tick",
    },
    TunableSpec {
        name: "jet_rise",
        low: 0.0,
        high: 20.0,
        step: 0.2,
        doc: "booster terminal rise speed",
    },
    TunableSpec {
        name: "mario_swim",
        low: 0.5,
        high: 15.0,
        step: 0.2,
        doc: "Mario swim speed",
    },
    TunableSpec {
        name: "hero_wade",
        low: 0.5,
        high: 12.0,
        step: 0.2,
        doc: "Hero deep-water speed",
    },
    TunableSpec {
        name: "cam_distance",
        low: 2.0,
        high: 24.0,
        step: 0.2,
        doc: "third-person camera distance",
    },
    TunableSpec {
        name: "cam_aim_distance",
        low: 2.0,
        high: 16.0,
        step: 0.2,
        doc: "aim camera distance",
    },
    TunableSpec {
        name: "cam_height",
        low: 0.0,
        high: 6.0,
        step: 0.1,
        doc: "camera focus height",
    },
    TunableSpec {
        name: "cam_smooth",
        low: 0.01,
        high: 1.0,
        step: 0.01,
        doc: "camera position response",
    },
    TunableSpec {
        name: "mouse_sens",
        low: 0.0005,
        high: 0.02,
        step: 0.0005,
        doc: "horizontal mouse sensitivity",
    },
    TunableSpec {
        name: "pad_look",
        low: 0.0,
        high: 8.0,
        step: 0.1,
        doc: "gamepad look speed in radians per second",
    },
    TunableSpec {
        name: "sfx_volume",
        low: 0.0,
        high: 1.0,
        step: 0.05,
        doc: "sound effect volume",
    },
    TunableSpec {
        name: "enemy_speed",
        low: 0.0,
        high: 10.0,
        step: 0.1,
        doc: "enemy chase speed",
    },
    TunableSpec {
        name: "enemy_lod_near",
        low: 5.0,
        high: 150.0,
        step: 5.0,
        doc: "distance where enemy AI drops to 15 Hz",
    },
    TunableSpec {
        name: "enemy_lod_far",
        low: 10.0,
        high: 250.0,
        step: 5.0,
        doc: "distance where enemy AI drops to 7.5 Hz",
    },
    TunableSpec {
        name: "enemy_draw",
        low: 20.0,
        high: 500.0,
        step: 10.0,
        doc: "distance where skinned enemies are hidden",
    },
    TunableSpec {
        name: "enemy_rate",
        low: 1.0,
        high: 60.0,
        step: 1.0,
        doc: "pipe spawn interval in seconds",
    },
    TunableSpec {
        name: "enemy_limit",
        low: 0.0,
        high: 500.0,
        step: 1.0,
        doc: "global live enemy cap",
    },
];

#[derive(Resource, Clone, Debug)]
pub struct GameTuning {
    pub hero_speed: f32,
    pub mario_speed: f32,
    pub walk_accel: f32,
    pub decel: f32,
    pub jump_velocity: f32,
    pub skate_speed: f32,
    pub skate_accel: f32,
    pub jet_thrust: f32,
    pub jet_rise: f32,
    pub mario_swim: f32,
    pub hero_wade: f32,
    pub cam_distance: f32,
    pub cam_aim_distance: f32,
    pub cam_height: f32,
    pub cam_smooth: f32,
    pub mouse_sens: f32,
    pub pad_look: f32,
    pub sfx_volume: f32,
    pub enemy_speed: f32,
    pub enemy_lod_near: f32,
    pub enemy_lod_far: f32,
    pub enemy_draw: f32,
    pub enemy_rate: f32,
    pub enemy_limit: f32,
}

impl Default for GameTuning {
    fn default() -> Self {
        Self {
            hero_speed: 11.4,
            mario_speed: 9.6,
            walk_accel: 22.0,
            decel: 10.0,
            jump_velocity: 12.6,
            skate_speed: 16.8,
            skate_accel: 9.0,
            jet_thrust: 2.4,
            jet_rise: 6.0,
            mario_swim: 5.5,
            hero_wade: 3.2,
            cam_distance: 9.5,
            cam_aim_distance: 5.7,
            cam_height: 1.35,
            cam_smooth: 0.24,
            mouse_sens: 0.003,
            pad_look: 2.6,
            sfx_volume: 0.7,
            enemy_speed: 1.8,
            enemy_lod_near: 35.0,
            enemy_lod_far: 70.0,
            enemy_draw: 140.0,
            enemy_rate: 7.0,
            enemy_limit: 12.0,
        }
    }
}

impl GameTuning {
    pub fn get(&self, name: &str) -> Option<f32> {
        Some(match name {
            "hero_speed" => self.hero_speed,
            "mario_speed" => self.mario_speed,
            "walk_accel" => self.walk_accel,
            "decel" => self.decel,
            "jump_velocity" => self.jump_velocity,
            "skate_speed" => self.skate_speed,
            "skate_accel" => self.skate_accel,
            "jet_thrust" => self.jet_thrust,
            "jet_rise" => self.jet_rise,
            "mario_swim" => self.mario_swim,
            "hero_wade" => self.hero_wade,
            "cam_distance" => self.cam_distance,
            "cam_aim_distance" => self.cam_aim_distance,
            "cam_height" => self.cam_height,
            "cam_smooth" => self.cam_smooth,
            "mouse_sens" => self.mouse_sens,
            "pad_look" => self.pad_look,
            "sfx_volume" => self.sfx_volume,
            "enemy_speed" => self.enemy_speed,
            "enemy_lod_near" => self.enemy_lod_near,
            "enemy_lod_far" => self.enemy_lod_far,
            "enemy_draw" => self.enemy_draw,
            "enemy_rate" => self.enemy_rate,
            "enemy_limit" => self.enemy_limit,
            _ => return None,
        })
    }

    pub fn set(&mut self, name: &str, value: f32) -> Result<(f32, f32), String> {
        let Some(spec) = SPECS.iter().find(|spec| spec.name == name) else {
            return Err(format!("no such variable: {name}  (try `vars`)"));
        };
        let previous = self.get(name).unwrap();
        let value = value.clamp(spec.low, spec.high);
        match name {
            "hero_speed" => self.hero_speed = value,
            "mario_speed" => self.mario_speed = value,
            "walk_accel" => self.walk_accel = value,
            "decel" => self.decel = value,
            "jump_velocity" => self.jump_velocity = value,
            "skate_speed" => self.skate_speed = value,
            "skate_accel" => self.skate_accel = value,
            "jet_thrust" => self.jet_thrust = value,
            "jet_rise" => self.jet_rise = value,
            "mario_swim" => self.mario_swim = value,
            "hero_wade" => self.hero_wade = value,
            "cam_distance" => self.cam_distance = value,
            "cam_aim_distance" => self.cam_aim_distance = value,
            "cam_height" => self.cam_height = value,
            "cam_smooth" => self.cam_smooth = value,
            "mouse_sens" => self.mouse_sens = value,
            "pad_look" => self.pad_look = value,
            "sfx_volume" => self.sfx_volume = value,
            "enemy_speed" => self.enemy_speed = value,
            "enemy_lod_near" => self.enemy_lod_near = value,
            "enemy_lod_far" => self.enemy_lod_far = value,
            "enemy_draw" => self.enemy_draw = value,
            "enemy_rate" => self.enemy_rate = value,
            "enemy_limit" => self.enemy_limit = value,
            _ => unreachable!(),
        }
        Ok((previous, value))
    }
}

#[derive(Resource)]
pub struct ConsoleState {
    pub open: bool,
    pub closed_this_frame: bool,
    input: String,
    log: VecDeque<String>,
    history: Vec<String>,
    history_at: usize,
    pinned: Vec<String>,
    selected: Option<String>,
    scroll: usize,
    defaults: GameTuning,
}

impl Default for ConsoleState {
    fn default() -> Self {
        let mut log = VecDeque::new();
        log.push_back("Bevy tuning console ready. Type `help`.".into());
        Self {
            open: false,
            closed_this_frame: false,
            input: String::new(),
            log,
            history: Vec::new(),
            history_at: 0,
            pinned: Vec::new(),
            selected: None,
            scroll: 0,
            defaults: GameTuning::default(),
        }
    }
}

impl ConsoleState {
    fn echo(&mut self, message: impl Into<String>) {
        for line in message.into().lines() {
            if self.log.len() == LOG_LIMIT {
                self.log.pop_front();
            }
            self.log.push_back(line.to_owned());
        }
    }

    pub fn execute(&mut self, line: &str, tuning: &mut GameTuning) {
        let words: Vec<_> = line.split_whitespace().collect();
        if words.is_empty() {
            return;
        }
        self.echo(format!("> {line}"));
        match words[0].to_ascii_lowercase().as_str() {
            "help" | "?" => self.echo("commands: <name> [value], vars, reset <name|all>, close <name|all>, clear\nSelect a variable then use Left/Right (Shift = 10x) to tune it. Wheel/PageUp/PageDown scroll the log."),
            "vars" | "list" => {
                for spec in SPECS {
                    self.echo(format!("  {:<18} {:>7.3}  [{:.3} .. {:.3}]  {}", spec.name, tuning.get(spec.name).unwrap(), spec.low, spec.high, spec.doc));
                }
            }
            "clear" => self.log.clear(),
            "reset" => self.reset(&words[1..], tuning),
            "close" | "hide" => self.close(&words[1..]),
            "set" | "slider" | "var" if words.len() > 1 => self.tunable(words[1], &words[2..], tuning),
            name if tuning.get(name).is_some() => self.tunable(name, &words[1..], tuning),
            name => {
                let matches: Vec<_> = SPECS.iter().filter(|s| s.name.starts_with(name)).map(|s| s.name).collect();
                let hint = if matches.is_empty() { String::new() } else { format!(" -- did you mean {}?", matches.join(", ")) };
                self.echo(format!("unknown: {}{}  (try `help`)", words[0], hint));
            }
        }
    }

    fn tunable(&mut self, name: &str, args: &[&str], tuning: &mut GameTuning) {
        if let Some(raw) = args.first() {
            match raw.parse::<f32>() {
                Ok(value) => match tuning.set(name, value) {
                    Ok((old, new)) => self.echo(format!("{name} = {new:.3}  (was {old:.3})")),
                    Err(error) => self.echo(error),
                },
                Err(_) => self.echo(format!("{name}: {raw:?} is not a number")),
            }
        } else {
            if !self.pinned.iter().any(|item| item == name) {
                self.pinned.push(name.to_owned());
            }
            self.selected = Some(name.to_owned());
            self.echo(format!(
                "{name} = {:.3}  -- control added",
                tuning.get(name).unwrap()
            ));
        }
    }

    fn reset(&mut self, args: &[&str], tuning: &mut GameTuning) {
        let names: Vec<_> = if args.first() == Some(&"all") {
            SPECS.iter().map(|s| s.name).collect()
        } else {
            args.to_vec()
        };
        if names.is_empty() {
            self.echo("reset: needs a variable name, or `all`");
            return;
        }
        for name in names {
            if let Some(value) = self.defaults.get(name) {
                let _ = tuning.set(name, value);
                self.echo(format!("{name} = {value:.3}  (default)"));
            } else {
                self.echo(format!("no such variable: {name}"));
            }
        }
    }

    fn close(&mut self, args: &[&str]) {
        if args.first() == Some(&"all") {
            self.pinned.clear();
            self.selected = None;
            self.echo("all controls closed");
        } else if let Some(name) = args.first() {
            self.pinned.retain(|item| item != name);
            if self.selected.as_deref() == Some(name) {
                self.selected = self.pinned.last().cloned();
            }
            self.echo(format!("{name} control closed"));
        } else {
            self.echo("close: needs a control name, or `all`");
        }
    }

    fn complete(&mut self) {
        let prefix = self.input.split_whitespace().last().unwrap_or("");
        let matches: Vec<_> = SPECS
            .iter()
            .filter(|s| s.name.starts_with(prefix))
            .collect();
        if matches.len() == 1 {
            let start = self.input.len() - prefix.len();
            self.input.replace_range(start.., matches[0].name);
        } else if !matches.is_empty() {
            self.echo(
                matches
                    .iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join("  "),
            );
        }
    }
}

#[derive(Component)]
pub struct ConsolePanel;
#[derive(Component)]
pub struct TuningTray;

/// The console panel's marker and UI bundle. Keeping all built-in TextBundle
/// fields inside the bundle prevents duplicate-component panics at startup.
pub fn panel_bundle() -> (ConsolePanel, TextBundle) {
    (
        ConsolePanel,
        TextBundle {
            style: Style {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                right: Val::Px(12.0),
                top: Val::Px(10.0),
                padding: UiRect::all(Val::Px(14.0)),
                ..default()
            },
            text: Text::from_section(
                "",
                TextStyle {
                    font_size: 17.0,
                    color: Color::rgb(0.88, 0.92, 1.0),
                    ..default()
                },
            ),
            background_color: BackgroundColor(Color::rgba(0.015, 0.02, 0.04, 0.94)),
            z_index: ZIndex::Global(100),
            visibility: Visibility::Hidden,
            ..default()
        },
    )
}

/// Persistent controls shown below the console while gameplay is running.
pub fn tuning_tray_bundle() -> (TuningTray, TextBundle) {
    (
        TuningTray,
        TextBundle {
            style: Style {
                position_type: PositionType::Absolute,
                right: Val::Px(18.0),
                bottom: Val::Px(18.0),
                padding: UiRect::all(Val::Px(8.0)),
                ..default()
            },
            text: Text::from_section(
                "",
                TextStyle {
                    font_size: 16.0,
                    color: Color::rgb(1.0, 0.92, 0.62),
                    ..default()
                },
            ),
            background_color: BackgroundColor(Color::rgba(0.02, 0.025, 0.05, 0.72)),
            z_index: ZIndex::Global(90),
            ..default()
        },
    )
}

pub fn is_closed(console: Res<ConsoleState>) -> bool {
    !console.open
}

pub fn input(
    keys: Res<Input<KeyCode>>,
    mut chars: EventReader<ReceivedCharacter>,
    mut wheel: EventReader<MouseWheel>,
    mut console: ResMut<ConsoleState>,
    mut tuning: ResMut<GameTuning>,
) {
    console.closed_this_frame = false;
    if keys.just_pressed(KeyCode::Grave) {
        console.open = !console.open;
        if console.open {
            console.echo("simulation paused");
        }
    }
    if !console.open {
        let direction = i8::from(keys.just_pressed(KeyCode::BracketRight))
            - i8::from(keys.just_pressed(KeyCode::BracketLeft));
        adjust_selected(direction, &keys, &console, &mut tuning);
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        console.open = false;
        console.closed_this_frame = true;
        return;
    }
    if keys.just_pressed(KeyCode::Return) {
        let line = std::mem::take(&mut console.input);
        if !line.trim().is_empty() {
            console.scroll = 0;
            console.history.push(line.clone());
            console.history_at = console.history.len();
            console.execute(&line, &mut tuning);
        }
    }
    if keys.just_pressed(KeyCode::Back) {
        console.input.pop();
    }
    if keys.just_pressed(KeyCode::Tab) {
        console.complete();
    }
    if keys.just_pressed(KeyCode::Up) && !console.history.is_empty() {
        console.history_at = console.history_at.saturating_sub(1);
        console.input = console.history[console.history_at].clone();
    }
    if keys.just_pressed(KeyCode::Down) {
        console.history_at = (console.history_at + 1).min(console.history.len());
        console.input = if console.history_at == console.history.len() {
            String::new()
        } else {
            console.history[console.history_at].clone()
        };
    }
    let wheel_lines: f32 = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => event.y * 3.0,
            MouseScrollUnit::Pixel => event.y / 20.0,
        })
        .sum();
    let max_scroll = console.log.len().saturating_sub(16);
    if wheel_lines > 0.0 || keys.just_pressed(KeyCode::PageUp) {
        let amount = if keys.just_pressed(KeyCode::PageUp) {
            16
        } else {
            wheel_lines.round() as usize
        };
        console.scroll = (console.scroll + amount).min(max_scroll);
    } else if wheel_lines < 0.0 || keys.just_pressed(KeyCode::PageDown) {
        let amount = if keys.just_pressed(KeyCode::PageDown) {
            16
        } else {
            (-wheel_lines).round() as usize
        };
        console.scroll = console.scroll.saturating_sub(amount);
    }
    let direction =
        i8::from(keys.just_pressed(KeyCode::Right)) - i8::from(keys.just_pressed(KeyCode::Left));
    adjust_selected(direction, &keys, &console, &mut tuning);
    for event in chars.read() {
        let character = event.char;
        if !character.is_control() && character != '`' && character != '~' {
            console.input.push(character);
        }
    }
}

fn adjust_selected(
    direction: i8,
    keys: &Input<KeyCode>,
    console: &ConsoleState,
    tuning: &mut GameTuning,
) {
    if direction == 0 {
        return;
    }
    if let Some(name) = &console.selected {
        let spec = SPECS.iter().find(|spec| spec.name == name).unwrap();
        let multiplier = if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
            10.0
        } else {
            1.0
        };
        let value = tuning.get(name).unwrap() + direction as f32 * spec.step * multiplier;
        let _ = tuning.set(name, value);
    }
}

/// Bevy's animation clock continues independently of fixed gameplay ticks, so
/// explicitly pause clips while the console owns the simulation.
pub fn pause_animations(console: Res<ConsoleState>, mut players: Query<&mut AnimationPlayer>) {
    for mut player in &mut players {
        if console.open {
            player.pause();
        } else {
            player.resume();
        }
    }
}

#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn draw(
    console: Res<ConsoleState>,
    tuning: Res<GameTuning>,
    state: Res<GameState>,
    diagnostics: Res<DiagnosticsStore>,
    player: Query<(&Controller, &Transform), With<Player>>,
    enemies: Query<(), With<Enemy>>,
    mut panel: Query<(&mut Text, &mut Visibility), (With<ConsolePanel>, Without<TuningTray>)>,
    mut tray: Query<&mut Text, (With<TuningTray>, Without<ConsolePanel>)>,
) {
    let (mut text, mut visibility) = panel.single_mut();
    *visibility = if console.open {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if console.open {
        let (controller, transform) = player.single();
        let fps = diagnostics
            .get(FrameTimeDiagnosticsPlugin::FPS)
            .and_then(|diagnostic| diagnostic.smoothed())
            .unwrap_or(0.0);
        let log = console
            .log
            .iter()
            .rev()
            .skip(console.scroll)
            .take(16)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        text.sections[0].value = format!(
            "BEVY DEBUG CONSOLE  ·  paused · log scroll {}\n{:?} · {:?} · health {} · enemies {} · {fps:.1} fps\npos {:.2}, {:.2}, {:.2}\n\n{log}\n\n> {}_",
            console.scroll,
            state.active,
            controller.motion,
            controller.health,
            enemies.iter().len(),
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
            console.input
        );
    }
    tray.single_mut().sections[0].value = console
        .pinned
        .iter()
        .map(|name| {
            let marker = if console.selected.as_deref() == Some(name) {
                ">"
            } else {
                " "
            };
            format!("{marker} {name:<18} {:>7.3}", tuning.get(name).unwrap())
        })
        .collect::<Vec<_>>()
        .join("\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_sets_clamps_and_resets_tunables() {
        let mut console = ConsoleState::default();
        let mut tuning = GameTuning::default();
        console.execute("hero_speed 999", &mut tuning);
        assert_eq!(tuning.hero_speed, 30.0);
        console.execute("reset hero_speed", &mut tuning);
        assert_eq!(tuning.hero_speed, GameTuning::default().hero_speed);
    }

    #[test]
    fn bare_name_pins_a_persistent_control() {
        let mut console = ConsoleState::default();
        let mut tuning = GameTuning::default();
        console.execute("cam_distance", &mut tuning);
        assert_eq!(console.pinned, vec!["cam_distance"]);
        assert_eq!(console.selected.as_deref(), Some("cam_distance"));
        console.execute("close all", &mut tuning);
        assert!(console.pinned.is_empty());
    }

    #[test]
    fn console_ui_bundles_spawn_without_duplicate_components() {
        let mut world = World::new();
        world.spawn(panel_bundle());
        world.spawn(tuning_tray_bundle());
    }
}
