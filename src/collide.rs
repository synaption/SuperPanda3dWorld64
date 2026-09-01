//! What the collision thinks, drawn over what you can see.
//!
//! Every complaint in this game that starts "I fell through the floor" or "it
//! is standing inside the wall" is a disagreement between two pictures: the
//! one on screen, which is [`crate::level::RenderLevel`] and the models, and
//! the one movement is actually resolved against, which is the triangles and
//! the grid in [`crate::level`]. Nothing draws the second one, so the only way
//! to tell a hole in the collision from a body that walked out of it has been
//! to guess.
//!
//! So this draws the second picture, in three layers, off by default and
//! turned on a layer at a time with `collide_debug` in the console:
//!
//!   1. **The bodies**, as the capsules they are resolved as, each with the
//!      ground query under its feet. A capsule is what `resolve_walls` sees;
//!      the model around it is decoration and is regularly a different size.
//!      Red means the body is inside a wall *right now* -- the resolution ran
//!      and did not get it out -- and a red stub under the feet with no ground
//!      cross on it means there is nothing under it at all, which is the frame
//!      before falling through.
//!   2. **The mesh**, near the camera, coloured by how the grid filed each
//!      triangle: green ground, red wall, amber for the band that is filed as
//!      both. See [`crate::level::FaceKind`] -- that band is where the two
//!      failure modes live, so it is drawn with its normal sticking out.
//!   3. **The grid and the candidates**: the cells a query is answered from,
//!      draped over the floor and lit by how much is filed in each, plus --
//!      for the player and for anything currently being pushed -- the exact
//!      list of wall triangles its own resolution walks, with an arrow on the
//!      ones pushing. A body inside a wall that is *not* in its own candidate
//!      list is a filing bug; one that is in the list with no push is a test
//!      that is not firing. Those are different bugs and this is what tells
//!      them apart.
//!
//! Immediate mode and per frame, like [`crate::path::draw`], which it is meant
//! to be read beside: that one draws where a body has been told to go, this
//! one draws what the world will let it do.

use crate::{
    console::GameTuning,
    enemy::{Crawler, Detail, Enemy},
    gravity::Gravity,
    level::{DebugFace, DebugWall, FaceKind, LevelData},
    player::{Player, PLAYER_HEIGHT, PLAYER_RADIUS},
    squad::Ally,
};
use bevy::prelude::*;

/// How far from the camera a body is drawn, in metres.
///
/// Every body drawn costs a wall resolution of its own, so this is a budget as
/// well as a range -- but it is a generous one: a field of two thousand is
/// nearly all impostors far past this, and what is inside it is the handful
/// you are standing among, which is the handful any of this is about.
const BODY_DRAW_RANGE: f32 = 40.0;

/// How far from the camera the collision mesh is drawn, in metres.
///
/// The castle's triangles are large -- 879 of them for the whole level -- so
/// this reaches further than the navigation grid does in [`crate::path`] and
/// still draws fewer lines than it.
const MESH_DRAW_RANGE: f32 = 25.0;

/// How far either way the grid is drawn, in cells.
const GRID_DRAW_CELLS: i32 = 4;

/// How far above a body's feet the ground query is asked from.
///
/// The same reach [`crate::player`] uses, and for the same reason: feet are
/// never exactly on the floor, so a query asked from them alone finds nothing
/// on the frame the body is a centimetre in the air. Matching it is what makes
/// the overlay's answer the answer movement got.
const GROUND_PROBE: f32 = 0.75;

/// How near the floor a body counts as standing on it, in metres.
const STANDING_BAND: f32 = 0.1;

/// How far down the mark under a body with no ground beneath it reaches.
const VOID_MARK: f32 = 3.0;

/// Everything with a collision body, whatever else it is.
type Bodies<'w, 's> = Query<
    'w,
    's,
    (
        &'static Transform,
        Option<&'static Enemy>,
        Option<&'static Crawler>,
        Option<&'static Detail>,
        Has<Player>,
    ),
    Or<(With<Player>, With<Ally>, With<Enemy>)>,
>;

pub fn draw(
    tuning: Res<GameTuning>,
    level: Res<LevelData>,
    gravity: Res<Gravity>,
    mut gizmos: Gizmos,
    bodies: Bodies,
    camera: Query<&Transform, With<Camera3d>>,
    mut faces: Local<Vec<DebugFace>>,
    mut walls: Local<Vec<DebugWall>>,
) {
    let layer = tuning.collide_debug.round() as u32;
    if layer == 0 {
        return;
    }
    let Ok(eye) = camera.single() else {
        return;
    };
    let eye = eye.translation;
    for (at, enemy, crawler, detail, driving) in &bodies {
        let position = at.translation;
        if position.distance(eye) > BODY_DRAW_RANGE {
            continue;
        }
        // Whatever the thing is actually resolved as. An enemy's cylinder
        // comes off its kind, and everything else in this game is a Mario.
        let (radius, height) = enemy.map_or((PLAYER_RADIUS, PLAYER_HEIGHT), |it| it.kind.body());
        // A crawler is held within the surface it is stuck to rather than
        // against gravity, so it is drawn leaning the way it is resolved.
        let up = crawler.map_or_else(|| gravity.up(position), |bug| bug.up);
        let escape = level.resolve_walls(position, up, radius, height) - position;
        let stuck = escape.length() > 1e-4;
        // The cheap tier is not held out of walls by the level at all -- the
        // flow field is what stands between it and the geometry -- so one of
        // those inside a wall is a different bug with a different owner, and
        // is drawn as its own colour rather than as the near tier's red.
        let crowd = detail == Some(&Detail::Crowd);
        let colour = match (stuck, crowd) {
            (false, false) => Color::srgba(0.3, 0.9, 1.0, 0.7),
            (false, true) => Color::srgba(0.3, 0.9, 1.0, 0.35),
            (true, false) => Color::srgb(1.0, 0.2, 0.2),
            (true, true) => Color::srgb(1.0, 0.5, 0.1),
        };
        // The capsule as `resolve_walls` builds it: a spine from a radius
        // above the feet to a radius below the head, swept by the radius.
        let spine = (height - radius * 2.0).max(0.0);
        let turn = Quat::from_rotation_arc(Vec3::Y, up);
        gizmos.primitive_3d(
            &Capsule3d::new(radius, spine),
            Isometry3d::new(position + up * (height * 0.5), turn),
            colour,
        );
        if stuck {
            // Which way out, and how far. Short arrows are the common case --
            // a body a centimetre inside a wall is still a body inside a wall
            // -- so the capsule's colour is the alarm and this is the detail.
            gizmos.arrow(
                position + up * radius,
                position + up * radius + escape,
                colour,
            );
        }
        match level.ground_below(position + up * GROUND_PROBE, up) {
            Some((ground, normal)) => {
                let separation = (position - ground).dot(up);
                let footing = match separation {
                    // Under the surface it is standing on. Not fatal on its
                    // own -- feet ease down onto the floor rather than being
                    // put on it -- but it is what the frame before a body
                    // drops through one looks like.
                    _ if separation < -STANDING_BAND => Color::srgb(1.0, 0.3, 0.3),
                    _ if separation <= STANDING_BAND => Color::srgb(0.3, 1.0, 0.4),
                    // In the air, which is only interesting if it should not
                    // be.
                    _ => Color::srgba(1.0, 0.8, 0.2, 0.6),
                };
                gizmos.line(position, ground, footing);
                // Lying on the surface that was found rather than flat, so a
                // body standing on a slope shows which facet answered.
                gizmos.circle(
                    Isometry3d::new(ground, Quat::from_rotation_arc(Vec3::Z, normal)),
                    radius,
                    footing,
                );
            }
            // The one to look for. Nothing is under this body: it is over a
            // hole in the collision, or outside the grid altogether.
            None => {
                let void = Color::srgb(1.0, 0.1, 0.1);
                gizmos.line(position, position - up * VOID_MARK, void);
                gizmos.cross(position - up * VOID_MARK, radius * 2.0, void);
            }
        }
        if layer < 3 {
            continue;
        }
        // The candidate list itself, for the bodies it is a question about:
        // the one you are driving, and anything a wall is pushing on.
        if !stuck && !driving {
            continue;
        }
        level.wall_contacts(position, up, radius, height, &mut walls);
        for wall in walls.iter() {
            let pushing = wall.push != Vec3::ZERO;
            let colour = match pushing {
                true => Color::srgb(1.0, 0.2, 0.6),
                false => Color::srgba(0.7, 0.3, 0.9, 0.35),
            };
            outline(&mut gizmos, wall.face.corners, colour);
            if pushing {
                gizmos.arrow(wall.nearest, wall.nearest + wall.push, colour);
            }
        }
    }
    if layer < 2 {
        return;
    }
    level.faces_near(eye, MESH_DRAW_RANGE, &mut faces);
    for face in faces.iter() {
        let colour = match face.kind {
            FaceKind::Ground => Color::srgba(0.3, 0.9, 0.4, 0.4),
            FaceKind::Ledge => Color::srgba(1.0, 0.7, 0.2, 0.7),
            FaceKind::Wall => Color::srgba(1.0, 0.3, 0.3, 0.5),
        };
        outline(&mut gizmos, face.corners, colour);
        if face.kind == FaceKind::Ledge {
            // The band that is filed as floor *and* as wall, with its facing
            // shown: which way one of these leans is what decides whether a
            // body climbs it, is stopped by it, or ends up inside it.
            let middle = (face.corners[0] + face.corners[1] + face.corners[2]) / 3.0;
            gizmos.arrow(middle, middle + face.normal, colour);
        }
    }
    if layer < 3 {
        return;
    }
    // The grid the answers come out of, draped over the floor under it. A cell
    // with nothing filed in it is a cell that can hold nothing up, and a run
    // of them under a floor you can see is the collision hole itself.
    let Some(here) = level.cell_footprint(eye) else {
        // A planet files by face cell rather than by a square of `(x, z)`, and
        // a square drawn over one would be a picture of something that is not
        // there.
        return;
    };
    for down in -GRID_DRAW_CELLS..=GRID_DRAW_CELLS {
        for across in -GRID_DRAW_CELLS..=GRID_DRAW_CELLS {
            let at = eye + Vec3::new(across as f32 * here.size.x, 0.0, down as f32 * here.size.y);
            let Some(cell) = level.cell_footprint(at) else {
                continue;
            };
            let colour = match (cell.floors, cell.walls) {
                // Empty. Whatever is drawn over this square, nothing here
                // stops a body.
                (0, 0) => Color::srgba(1.0, 0.2, 0.2, 0.8),
                (0, _) => Color::srgba(1.0, 0.6, 0.2, 0.6),
                (_, 0) => Color::srgba(0.4, 0.7, 1.0, 0.3),
                _ => Color::srgba(0.5, 0.9, 1.0, 0.5),
            };
            let corners = [
                Vec2::new(cell.min.x, cell.min.y),
                Vec2::new(cell.min.x + cell.size.x, cell.min.y),
                Vec2::new(cell.min.x + cell.size.x, cell.min.y + cell.size.y),
                Vec2::new(cell.min.x, cell.min.y + cell.size.y),
            ];
            // Each corner put on the floor under it, so the grid follows the
            // stairs and the banks instead of cutting through them. A corner
            // with no floor under it is dropped to the cell's own level rather
            // than skipped: a gap in the outline is the hole, and it should be
            // visible as one.
            let lifted = corners.map(|corner| {
                let post = Vec3::new(corner.x, eye.y, corner.y);
                level
                    .ground_below(post, Vec3::Y)
                    .map_or(post - Vec3::Y * VOID_MARK, |(ground, _)| ground)
            });
            gizmos.linestrip(lifted.into_iter().chain(std::iter::once(lifted[0])), colour);
        }
    }
}

/// One triangle, as its three edges.
fn outline(gizmos: &mut Gizmos, corners: [Vec3; 3], colour: Color) {
    gizmos.linestrip([corners[0], corners[1], corners[2], corners[0]], colour);
}
