#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod animation;
mod camera;
mod enemy;
mod level;
mod player;

use bevy::{
    prelude::*,
    window::{CursorGrabMode, PrimaryWindow},
};
use camera::FollowCamera;
use enemy::Enemy;
use player::{Controller, Player, PlayerVisual, PreviousPose, RenderPose};
use std::path::PathBuf;

#[derive(Resource, Default)]
struct GameState {
    active: ActiveCharacter,
    aiming: bool,
    debug: bool,
}

#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
enum ActiveCharacter {
    #[default]
    Hero,
    Mario,
}

#[derive(Component)]
struct Hud;

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::rgb(0.32, 0.60, 0.86)))
        .insert_resource(GameState {
            active: ActiveCharacter::Hero,
            aiming: false,
            debug: true,
        })
        .insert_resource(Time::<Fixed>::from_hz(30.0))
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Super Bevy World 64".into(),
                        resolution: (1280., 720.).into(),
                        present_mode: bevy::window::PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: asset_path().to_string_lossy().into_owned(),
                    ..default()
                }),
        )
        .add_systems(Startup, setup)
        .add_systems(FixedUpdate, player::movement)
        .add_systems(
            Update,
            (
                player::sync_visual,
                camera::update,
                animation::claim_players,
                animation::update,
                enemy::update,
                controls,
                update_hud,
            )
                .chain(),
        )
        .run();
}

fn asset_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.join("assets")))
        .filter(|path| path.join("hero/hero.glb").is_file())
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets"))
}

fn setup(
    mut commands: Commands,
    assets: Res<AssetServer>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    let (collision, render) = level::load();
    commands.insert_resource(animation::CharacterAnimations::load(&assets));
    commands.insert_resource(collision);
    commands.spawn(SceneBundle {
        scene: assets.load("bevy/castle.glb#Scene0"),
        ..default()
    });
    for position in render.trees {
        commands.spawn(SceneBundle {
            scene: assets.load("actors/tree.glb#Scene0"),
            transform: Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
            ..default()
        });
    }
    let spawn = Transform::from_xyz(-13.28, 3.0, 46.64);
    commands.insert_resource(RenderPose {
        translation: spawn.translation,
        rotation: spawn.rotation,
    });
    commands.spawn((
        Player,
        PreviousPose::new(&spawn),
        Controller::default(),
        SpatialBundle::from_transform(spawn),
    ));
    commands.spawn((
        PlayerVisual,
        ActiveCharacter::Hero,
        SceneBundle {
            scene: assets.load("hero/hero.glb#Scene0"),
            transform: Transform::from_scale(Vec3::splat(0.81)),
            ..default()
        },
    ));
    commands.spawn((
        PlayerVisual,
        ActiveCharacter::Mario,
        SceneBundle {
            scene: assets.load("mario/mario.glb#Scene0"),
            visibility: Visibility::Hidden,
            transform: Transform::from_scale(Vec3::splat(0.00667)),
            ..default()
        },
    ));

    let spawns = [
        ("actors/goomba.glb#Scene0", Vec3::new(-3., 3., 26.)),
        ("actors/goomba.glb#Scene0", Vec3::new(-24., 3., 29.)),
        ("actors/goomba.glb#Scene0", Vec3::new(9., 3., 34.)),
        ("actors/scuttlebug.glb#Scene0", Vec3::new(-29., 3., 21.)),
        ("actors/scuttlebug.glb#Scene0", Vec3::new(4., 3., 19.)),
    ];
    for (i, (model, position)) in spawns.into_iter().enumerate() {
        let animation = assets.load(format!("{}#Animation0", model.trim_end_matches("#Scene0")));
        commands.spawn((
            Enemy {
                origin: position,
                phase: i as f32,
                animation,
            },
            SceneBundle {
                scene: assets.load(model),
                transform: Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
                ..default()
            },
        ));
    }
    for position in [
        Vec3::new(-9.15, 2.6, 46.3),
        Vec3::new(-55.1, 5.4, -39.2),
        Vec3::new(46.8, 5.4, -68.1),
    ] {
        commands.spawn(SceneBundle {
            scene: assets.load("actors/warp_pipe.glb#Scene0"),
            transform: Transform::from_translation(position).with_scale(Vec3::splat(0.01)),
            ..default()
        });
    }
    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            illuminance: 18_000.0,
            shadows_enabled: true,
            ..default()
        },
        transform: Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, -0.5, 0.0)),
        ..default()
    });
    commands.insert_resource(AmbientLight {
        color: Color::rgb(0.75, 0.82, 1.0),
        brightness: 0.65,
    });
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_xyz(-13.0, 10.0, 56.0)
                .looking_at(Vec3::new(-13.0, 4.0, 46.0), Vec3::Y),
            projection: PerspectiveProjection {
                fov: 60_f32.to_radians(),
                near: 0.05,
                far: 1000.0,
                ..default()
            }
            .into(),
            ..default()
        },
        FollowCamera {
            yaw: 0.0,
            pitch: -0.2,
            distance: 9.5,
        },
    ));
    commands.spawn((
        Hud,
        TextBundle::from_section(
            "",
            TextStyle {
                font_size: 18.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            position_type: PositionType::Absolute,
            left: Val::Px(16.0),
            top: Val::Px(12.0),
            ..default()
        }),
    ));
    commands.spawn(
        TextBundle::from_section(
            "+",
            TextStyle {
                font_size: 28.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            position_type: PositionType::Absolute,
            left: Val::Percent(49.5),
            top: Val::Percent(47.0),
            ..default()
        }),
    );
    if let Ok(mut window) = windows.get_single_mut() {
        window.cursor.grab_mode = CursorGrabMode::Locked;
        window.cursor.visible = false;
    }
}

fn controls(
    keys: Res<Input<KeyCode>>,
    mut state: ResMut<GameState>,
    mut visuals: Query<(&ActiveCharacter, &mut Visibility), With<PlayerVisual>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if keys.just_pressed(KeyCode::F2) {
        state.active = if state.active == ActiveCharacter::Hero {
            ActiveCharacter::Mario
        } else {
            ActiveCharacter::Hero
        };
        for (kind, mut visibility) in &mut visuals {
            *visibility = if *kind == state.active {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
    if keys.just_pressed(KeyCode::F1) {
        state.debug = !state.debug;
    }
    if keys.just_pressed(KeyCode::Escape) {
        if let Ok(mut window) = windows.get_single_mut() {
            let locked = window.cursor.grab_mode != CursorGrabMode::None;
            window.cursor.grab_mode = if locked {
                CursorGrabMode::None
            } else {
                CursorGrabMode::Locked
            };
            window.cursor.visible = locked;
        }
    }
}

fn update_hud(
    state: Res<GameState>,
    player: Query<&Controller, With<Player>>,
    mut text: Query<&mut Text, With<Hud>>,
) {
    let ctrl = player.single();
    text.single_mut().sections[0].value = if state.debug {
        format!("Super Bevy World 64\n{:?}  ·  {:?}\nWASD move · mouse look · Space jump · V jetpack/skate\nShift attack · F/right mouse aim · F2 switch · Esc cursor", state.active, ctrl.motion)
    } else {
        String::new()
    };
}
