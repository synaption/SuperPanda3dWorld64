//! Turning billboarded geometry to face the camera.
//!
//! SM64 draws some things as flat quads it rebuilds every frame to point at
//! the camera: whole objects, like the trees, and single parts of an actor,
//! like a scuttlebug's eyes. glTF has no billboard concept, so all of it arrives
//! as ordinary geometry, drawn from whatever one side it was authored on. A
//! flat quad seen from ninety degrees away is not merely wrong, it is nothing
//! at all -- the tree meshes here measure exactly zero thick.
//!
//! Ported from `sm64py/billboard.py` and the `billboard == "axis"` branch of
//! `sm64py/level.py`, which are also where the reasoning below was worked out
//! the hard way.
//!
//! Two mechanisms, because there are two cases:
//!
//!   * A **whole object** is plain geometry and turns bodily. Only about the
//!     vertical, so a tree does not tip over when the camera looks down at it.
//!   * A **part of an actor** is skinned to a joint, so no transform on the
//!     object can reach it. The exporter makes each such quad a joint of its
//!     own, named `billboard_*`, and those are driven individually.

use bevy::{
    ecs::{schedule::ScheduleConfigs, system::ScheduleSystem},
    prelude::*,
    transform::TransformSystems,
};

/// The name the exporter gives a joint that is a billboard quad.
const JOINT_PREFIX: &str = "billboard_";

/// Scale put back onto a billboard joint.
///
/// These actors are wrapped in a `GEO_SCALE(0x00, 16384)` of 0.25, which the
/// exporter bakes onto the root joint and every joint under it inherits.
/// Billboarded geometry escapes it in the original, because there the matrix
/// is rebuilt at the billboard rather than accumulated into it. 4.0 is exactly
/// 1/0.25 and puts it back.
const JOINT_SCALE: f32 = 4.0;

/// An object drawn as a flat quad that turns bodily to face the camera.
#[derive(Component)]
pub struct BillboardAxis;

/// An actor that may carry billboard joints inside its skeleton, and whose
/// surfaces are therefore drawn from both sides.
#[derive(Component)]
pub struct BillboardActor;

/// One quad inside an actor, claimed from the name its exporter gave it.
#[derive(Component)]
pub struct BillboardJoint;

/// Tags every joint the exporter marked as a billboard, as its scene arrives.
///
/// By name, because that is the only thing that survives the export -- the
/// same reason the animation clips are looked up by name.
pub fn claim(mut commands: Commands, arrivals: Query<(Entity, &Name), Added<Name>>) {
    for (entity, name) in &arrivals {
        if name.as_str().starts_with(JOINT_PREFIX) {
            commands.entity(entity).insert(BillboardJoint);
        }
    }
}

/// Draws billboarded surfaces from both sides.
///
/// A billboard quad is single-sided, and which of its faces was authored
/// toward the viewer is not something this port can see. Aimed the wrong way
/// round it would be invisible from every angle rather than from half of them,
/// which is a worse failure than the one being fixed -- and the original never
/// culls these anyway, since it never sees the back of one.
pub fn two_sided(
    mut materials: ResMut<Assets<StandardMaterial>>,
    hierarchy: Query<&ChildOf>,
    actors: Query<(), With<BillboardActor>>,
    surfaces: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        Added<MeshMaterial3d<StandardMaterial>>,
    >,
) {
    for (entity, handle) in &surfaces {
        let mut ancestor = entity;
        loop {
            if actors.contains(ancestor) {
                if let Some(mut material) = materials.get_mut(&handle.0) {
                    material.double_sided = true;
                    material.cull_mode = None;
                }
                break;
            }
            let Ok(parent) = hierarchy.get(ancestor) else {
                break;
            };
            ancestor = parent.parent();
        }
    }
}

/// The rotation that points a quad's authored face at a target.
///
/// About the vertical only. Pitch would tip the quad away from the camera and
/// roll would spin it about its own normal, and neither changes the
/// silhouette of something flat -- so the one useful degree of freedom is the
/// one taken here.
///
/// The quads face +Z as authored: they come from a build whose forward is
/// Panda3D's -Y, and that is +Z once the axes are converted.
pub fn facing(here: Vec3, target: Vec3) -> Quat {
    let away = target - here;
    Quat::from_rotation_y(away.x.atan2(away.z))
}

/// Aims everything billboarded at the camera, once per rendered frame.
#[allow(clippy::type_complexity)]
pub fn aim(
    // **The eye the player is actually looking from, which is not always where
    // the camera entity is.** A gate the boom goes through flies the camera to
    // the far end of the pair for the frame it is drawn on -- see
    // [`crate::portal::carry_camera`] -- and a card turned to face *that* is a
    // card turned across the map. `FollowCamera::eye` is where the view
    // logically is, beside the player, which is the right answer for the
    // overwhelming majority of what is on the screen at that moment.
    //
    // It is `Option` because the impostor baker runs this same chain with a
    // camera of its own and no follow rig on it; there the entity's own
    // transform is the only eye there is.
    camera: Query<
        (&GlobalTransform, Option<&crate::camera::FollowCamera>),
        (
            With<Camera3d>,
            Without<BillboardAxis>,
            Without<crate::portal::PortalView>,
        ),
    >,
    globals: Query<&GlobalTransform>,
    mut objects: Query<(&mut Transform, &GlobalTransform), With<BillboardAxis>>,
    mut joints: Query<
        (&mut Transform, &GlobalTransform, &ChildOf),
        (With<BillboardJoint>, Without<BillboardAxis>),
    >,
) {
    let Ok((view, follow)) = camera.single() else {
        return;
    };
    let eye = follow.map_or_else(|| view.translation(), |follow| follow.eye.translation);
    for (mut transform, global) in &mut objects {
        transform.rotation = facing(global.translation(), eye);
    }
    for (mut transform, global, parent) in &mut joints {
        let Ok(above) = globals.get(parent.parent()) else {
            continue;
        };
        // The joint chain above this quad leaves it with a rotation of its
        // own -- on the scuttlebug, about a quarter turn of roll -- and a local
        // value is applied on top of that. So a heading set here comes out as
        // net *pitch*, and no value of it could ever have worked. Since
        // `net = parent * local`, the local wanted is `parent^-1 * world`.
        let (_, above_rotation, _) = above.to_scale_rotation_translation();
        transform.rotation = above_rotation.inverse() * facing(global.translation(), eye);
        transform.scale = Vec3::splat(JOINT_SCALE);
    }
}

/// Aiming runs after the animation player has posed the skeleton and before
/// the transforms are propagated.
///
/// Both halves are load-bearing. Before the animation player, every joint
/// written here is overwritten by the clip a moment later and nothing turns at
/// all; after propagation, the turn is a frame late and, worse, the quads are
/// drawn from transforms that never saw it.
pub fn systems() -> ScheduleConfigs<ScheduleSystem> {
    (claim, two_sided, aim)
        .chain()
        .after(bevy::animation::animate_targets)
        .before(TransformSystems::Propagate)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quad facing +Z turns to put that face on the camera, whichever side
    /// of it the camera is on.
    #[test]
    fn a_quad_turns_its_face_to_the_camera() {
        let here = Vec3::new(4.0, 1.0, -7.0);
        for camera in [
            Vec3::new(4.0, 1.0, 0.0),
            Vec3::new(-20.0, 3.0, -7.0),
            Vec3::new(30.0, 0.0, -30.0),
            Vec3::new(4.0, 40.0, -12.0),
        ] {
            let face = facing(here, camera) * Vec3::Z;
            let wanted = (camera - here) * Vec3::new(1.0, 0.0, 1.0);
            let cosine = face.normalize().dot(wanted.normalize());
            assert!(
                cosine > 0.999,
                "the quad faces {face:?} with the camera at {camera:?}"
            );
        }
    }

    /// It turns about the vertical only: a camera looking down from overhead
    /// must not tip a tree onto its back.
    #[test]
    fn a_billboard_never_tips_over() {
        let up = facing(Vec3::ZERO, Vec3::new(0.3, 50.0, 0.3)) * Vec3::Y;
        assert!(
            (up - Vec3::Y).length() < 1e-5,
            "the billboard's up ended at {up:?}"
        );
    }

    /// The joint case: whatever rotation the skeleton leaves the quad with is
    /// cancelled, so the quad ends up facing the camera in world space rather
    /// than relative to a bone that is itself rotated a quarter turn.
    #[test]
    fn a_joints_parent_rotation_is_cancelled() {
        // A quarter turn of roll, which is what a billboard quad hangs off.
        let parent = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let here = Vec3::new(1.0, 2.0, 3.0);
        let camera = Vec3::new(9.0, 2.5, -4.0);
        let local = parent.inverse() * facing(here, camera);
        let net = parent * local;
        let face = net * Vec3::Z;
        let wanted = (camera - here) * Vec3::new(1.0, 0.0, 1.0);
        assert!(
            face.normalize().dot(wanted.normalize()) > 0.999,
            "the quad ended up facing {face:?} rather than the camera"
        );
    }

    /// Claiming is by name, so the name is worth pinning: if the exporter ever
    /// stops marking these, nothing turns and nothing says why.
    ///
    /// No actor the game loads carries one any more. The goomba and the
    /// scuttlebug did -- both were decomp exports, and their faces and eyes were
    /// the flat quads SM64 rebuilds every frame -- and the slime and the ant
    /// that replaced them are authored art with real skinned meshes and nothing
    /// flat anywhere on them. So the name is pinned against the decomp's own
    /// export, kept unedited under `assets/packs/reference`, and the loaded
    /// actors are checked for the opposite: one arriving with a `billboard_*`
    /// joint means this module is back in the business of driving joints, which
    /// is worth hearing from a test rather than from a face pointing the wrong
    /// way.
    #[test]
    fn the_actors_still_carry_the_joints_this_claims() {
        let nodes = node_names("packs/reference/actors/scuttlebug.glb");
        assert!(
            nodes.iter().any(|name| name.starts_with(JOINT_PREFIX)),
            "the decomp scuttlebug has no {JOINT_PREFIX}* joint for this to drive"
        );
        // The tree is the other case: a whole object, with no joint of its own
        // to claim, turned bodily instead.
        let tree = node_names("actors/tree.glb");
        assert!(
            !tree.iter().any(|name| name.starts_with(JOINT_PREFIX)),
            "the tree grew a billboard joint, so it wants driving the other way"
        );
        // And the two enemies are a third case: actors with no billboarded
        // part at all. They still carry `BillboardActor`, which is what makes
        // their surfaces double-sided, but there is nothing here for `aim` to
        // turn.
        for actor in ["slime", "ant"] {
            let nodes = node_names(&format!("actors/{actor}.glb"));
            assert!(
                !nodes.iter().any(|name| name.starts_with(JOINT_PREFIX)),
                "the {actor} grew a billboard joint, so something has to drive it"
            );
        }
    }

    /// Where `JOINT_SCALE` comes from. If the exporter ever stops baking the
    /// 0.25 onto the skeleton, putting 4.0 back would make these quads four
    /// times too big rather than the right size.
    ///
    /// Measured on the decomp export rather than on a runtime actor, for the
    /// same reason as above: nothing the game loads carries a billboard joint
    /// now. It is also the only copy that still carries the 0.25 at all -- the
    /// runtime scuttlebug was rebuilt from a .blend and came back with no scale
    /// on its skeleton, so this had been failing against it since before the
    /// bug was replaced.
    #[test]
    fn the_scale_put_back_is_the_one_baked_in() {
        let actor = "packs/reference/actors/scuttlebug";
        let baked = root_joint_scale(&format!("{actor}.glb"))
            .unwrap_or_else(|| panic!("{actor} has no scale on its skeleton"));
        assert!(
            (baked * JOINT_SCALE - 1.0).abs() < 1e-4,
            "{actor} bakes {baked} and this puts back {JOINT_SCALE}"
        );
    }

    /// A tree is a flat card in the XY plane, so its face is along Z -- which
    /// is the whole assumption `facing` turns it on, and the reason it
    /// disappears entirely when it is not turned.
    #[test]
    fn a_tree_is_a_flat_card_facing_along_z() {
        let extent = mesh_extent("actors/tree.glb");
        assert_eq!(extent[2], 0.0, "the tree is {extent:?}, not flat along Z");
        assert!(
            extent[0] > 0.0 && extent[1] > 0.0,
            "the tree has no face at all: {extent:?}"
        );
    }

    /// Aiming turns the objects in the world, not just the maths above.
    #[test]
    fn aiming_turns_a_field_of_billboards_toward_the_camera() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        let eye = Vec3::new(12.0, 4.0, -9.0);
        world.spawn((
            Camera3d::default(),
            Transform::from_translation(eye),
            GlobalTransform::from_translation(eye),
        ));
        let places = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(-8.0, 1.0, 4.0),
            Vec3::new(20.0, 0.0, -30.0),
        ];
        let trees: Vec<_> = places
            .iter()
            .map(|place| {
                world
                    .spawn((
                        BillboardAxis,
                        Transform::from_translation(*place),
                        GlobalTransform::from_translation(*place),
                    ))
                    .id()
            })
            .collect();
        world.run_system_once(aim).expect("aim could not run");
        for (tree, place) in trees.iter().zip(places) {
            let face = world.get::<Transform>(*tree).unwrap().rotation * Vec3::Z;
            let wanted = ((eye - place) * Vec3::new(1.0, 0.0, 1.0)).normalize();
            assert!(
                face.dot(wanted) > 0.999,
                "the tree at {place:?} faces {face:?} rather than {wanted:?}"
            );
        }
    }

    // -- reading the glTF directly, so none of this needs a renderer ---------

    fn gltf(path: &str) -> serde_json::Value {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let bytes = std::fs::read(root.join(path)).expect("missing glb");
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&bytes[20..20 + length]).expect("bad glb json")
    }

    fn node_names(path: &str) -> Vec<String> {
        gltf(path)["nodes"]
            .as_array()
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|node| node["name"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The uniform scale the exporter bakes onto the skeleton, if it does.
    fn root_joint_scale(path: &str) -> Option<f32> {
        gltf(path)["nodes"]
            .as_array()?
            .iter()
            .find_map(|node| Some(node["scale"].as_array()?[0].as_f64()? as f32))
    }

    /// How big the first mesh primitive is along each axis, from the accessor
    /// bounds the exporter writes.
    fn mesh_extent(path: &str) -> [f32; 3] {
        let file = gltf(path);
        let accessor = file["meshes"][0]["primitives"][0]["attributes"]["POSITION"]
            .as_u64()
            .expect("no positions") as usize;
        let bounds = |key: &str| -> Vec<f32> {
            file["accessors"][accessor][key]
                .as_array()
                .expect("the exporter wrote no bounds")
                .iter()
                .map(|value| value.as_f64().unwrap() as f32)
                .collect()
        };
        let (low, high) = (bounds("min"), bounds("max"));
        [high[0] - low[0], high[1] - low[1], high[2] - low[2]]
    }
}
