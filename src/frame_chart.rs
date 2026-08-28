//! Small, always-visible frame-time history graph.

use bevy::prelude::*;
use std::{collections::VecDeque, time::Instant};

const SAMPLES: usize = 240;
const WINDOW_SECONDS: f32 = 4.0;
const WIDTH: f32 = 300.0;
const HEIGHT: f32 = 92.0;
const MAX_MS: f32 = 50.0;

#[derive(Component)]
pub(crate) struct FrameBar(usize);

#[derive(Component)]
pub(crate) struct FrameStats;

/// CPU time spent running the game's schedules, excluding VSync wait time.
#[derive(Resource, Default)]
pub(crate) struct CalculationTimes {
    started: Option<Instant>,
    samples: VecDeque<(Instant, f32)>,
}

/// Starts the CPU timer before the frame schedules run.
pub fn begin(mut times: ResMut<CalculationTimes>) {
    times.started = Some(Instant::now());
}

/// Stops the timer after the frame schedules and retains the visible window.
pub fn finish(mut times: ResMut<CalculationTimes>) {
    let now = Instant::now();
    let Some(started) = times.started.take() else {
        return;
    };
    times
        .samples
        .push_back((now, now.duration_since(started).as_secs_f32() * 1000.0));
    while times
        .samples
        .front()
        .is_some_and(|(at, _)| now.duration_since(*at).as_secs_f32() > WINDOW_SECONDS)
    {
        times.samples.pop_front();
    }
}

/// Creates the chart beneath the textual performance readout.
pub fn spawn(commands: &mut Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(16.0),
                top: Val::Px(40.0),
                width: Val::Px(WIDTH),
                height: Val::Px(HEIGHT),
                border: UiRect::all(Val::Px(1.0)),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgba(0.015, 0.02, 0.04, 0.78)),
            BorderColor::all(Color::srgba(0.55, 1.0, 0.55, 0.45)),
            GlobalZIndex(20),
        ))
        .with_children(|chart| {
            // A guide at 16.7 ms makes the 60 fps budget immediately visible.
            chart.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    bottom: Val::Px(HEIGHT * (1000.0 / 60.0) / MAX_MS),
                    width: Val::Percent(100.0),
                    height: Val::Px(1.0),
                    ..default()
                },
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.22)),
            ));
            // Half-second divisions across the four-second window.
            for half_second in 1..8 {
                chart.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(WIDTH * half_second as f32 / 8.0),
                        top: Val::Px(0.0),
                        width: Val::Px(1.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.12)),
                ));
            }
            for index in 0..SAMPLES {
                chart.spawn((
                    FrameBar(index),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(index as f32 * WIDTH / SAMPLES as f32),
                        bottom: Val::Px(0.0),
                        width: Val::Px(WIDTH / SAMPLES as f32),
                        height: Val::Px(0.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.35, 0.95, 0.45)),
                ));
            }
            chart.spawn((
                Text::new("50 ms"),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::srgba(0.8, 0.9, 0.8, 0.7)),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(4.0),
                    top: Val::Px(2.0),
                    ..default()
                },
            ));
            chart.spawn((
                FrameStats,
                Text::new(""),
                TextFont {
                    font_size: FontSize::Px(11.0),
                    ..default()
                },
                TextColor(Color::WHITE),
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(4.0),
                    top: Val::Px(2.0),
                    ..default()
                },
            ));
        });
}

/// Draws the last four seconds oldest-to-newest, with the newest at right.
pub fn update(
    times: Res<CalculationTimes>,
    mut bars: Query<(&FrameBar, &mut Node, &mut BackgroundColor)>,
    mut stats: Query<&mut Text, With<FrameStats>>,
) {
    let Some((newest, _)) = times.samples.back() else {
        return;
    };
    let values: Vec<(f32, f32)> = times
        .samples
        .iter()
        .filter_map(|(at, value)| {
            let age = newest.duration_since(*at).as_secs_f32();
            (age <= WINDOW_SECONDS).then_some((age, *value))
        })
        .collect();
    let mut columns = [0.0_f32; SAMPLES];
    for (age, ms) in &values {
        let index = (((WINDOW_SECONDS - age) / WINDOW_SECONDS) * SAMPLES as f32)
            .floor()
            .min((SAMPLES - 1) as f32) as usize;
        columns[index] = columns[index].max(*ms);
    }
    for (bar, mut node, mut colour) in &mut bars {
        let ms = columns[bar.0];
        node.height = Val::Px((ms.min(MAX_MS) / MAX_MS) * HEIGHT);
        colour.0 = if ms > 1000.0 / 60.0 {
            Color::srgb(1.0, 0.42, 0.25)
        } else {
            Color::srgb(0.35, 0.95, 0.45)
        };
    }
    if let Ok(mut text) = stats.single_mut() {
        if values.is_empty() {
            **text = String::new();
            return;
        }
        let (minimum, peak, total) = values.iter().fold(
            (f32::MAX, 0.0_f32, 0.0_f32),
            |(minimum, peak, total), (_, ms)| (minimum.min(*ms), peak.max(*ms), total + ms),
        );
        **text = format!(
            "min {minimum:.1}  avg {:.1}  peak {peak:.1} ms",
            total / values.len() as f32
        );
    }
}
