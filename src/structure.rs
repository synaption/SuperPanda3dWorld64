//! Buildings: the things that stand still, take damage, and can be knocked
//! down.
//!
//! Before this module the only things in the game that could be *lost* were
//! actors. A pylon was planted and then permanent, and a warp pipe was scenery
//! that produced a crowd forever with no answer to it -- which made a network
//! something you built once and never thought about again, and a nest something
//! you walked away from because there was nothing else to do with it.
//!
//! One component covers both, and it is deliberately not two:
//!
//!   * A [`Structure`] is a cylinder with hit points, standing on a spot. It
//!     carries the same [`crate::health::Health`] an actor does and the same
//!     [`crate::enemy::Side`] an actor does, which is the whole trick -- the
//!     attention model in [`crate::enemy::alert`] asks what side a thing is on
//!     and how far away it is, and never asks whether it can walk. A pylon is
//!     `Friendly` and a slime nest's pipe is `Hostile`, and from there the
//!     crowd comes for the masts and the squad comes for the pipes with no new
//!     targeting code at all.
//!   * **The recovery window sits on the victim**, exactly as a Mario's does in
//!     [`crate::enemy::maul`]. Six ants on one mast take turns rather than all
//!     landing at once, which is what makes a crowd's damage a rate you can
//!     answer rather than a number that scales with how many of them arrived.
//!
//! **Only the crowd goes through that window**, and the split is worth being
//! precise about. An enemy in [`siege`] has no swing of its own -- it is simply
//! *standing on* the thing, every tick, forever -- so without a window the
//! damage would be "how many bodies fit round it". Every other attacker already
//! throws a discrete blow: the player's sword fires once per swing (the rising
//! edge, in [`demolish`]), a Mario's fist lands at the end of its own swing
//! timer, and a bullet is one round. Those all land in full. A squad's punches
//! should count.
//!
//! What this module owns is the *hitting*: [`siege`] is the crowd wearing a
//! building down, [`demolish`] is the player knocking one over. A Mario's fists
//! are folded into [`crate::enemy::ally_combat`] rather than run here, because a
//! Mario has one swing timer and two things it might be swinging at, and two
//! systems sharing that timer is a Mario that punches twice.

use bevy::prelude::*;

use crate::{
    audio::{Sfx, SoundQueue},
    enemy::{Aggro, Enemy, Side, Threats},
    health::{self, Health},
    player::{Controller, Player, FIXED_DT, PLAYER_RADIUS},
};

/// How often the crowd standing on a building gets to take a piece out of it.
///
/// Shorter than the second an actor gets, and the difference is the point: a
/// building cannot walk out of the way, so a full second of immunity would make
/// a crowd standing on a pylon look like a crowd standing *next to* one. A third
/// of a second is fast enough to read as being torn down and slow enough that a
/// hundred ants do not delete a mast in a tick.
///
/// It paces [`siege`] and nothing else -- see this module's preamble.
pub const RECOVERY: f32 = 0.35;

/// How far past a building's own surface a blow reaches.
///
/// Measured from the surface rather than the centre for the reason every reach
/// in [`crate::enemy`] is: a warp pipe is metres across and a pylon is under one,
/// and a swing written as a distance to the middle would land on one and never
/// on the other.
///
/// **It has to cover [`crate::enemy::CLOSEST_STAND`]**, and that is a rule
/// rather than a coincidence. An enemy walks to its target's surface plus a
/// berth of its own, up to 2.2 m, so that a crowd arrives *around* a thing
/// instead of converging on one spot. At the 1.6 m this started as, the share of
/// the crowd that drew a wide berth stood round a pylon and swung at air: the
/// siege ran until every ant still in contact had been shoved out by
/// [`crate::enemy::spread`], and then stalled with the mast on its last few
/// points, forever. The margin past it is for the weave, which is still nudging
/// a walker sideways as it arrives.
pub const REACH: f32 = 2.6;

/// The rule above, as a compile error rather than a comment.
///
/// [`crate::health`]'s tables are guarded this way for the same reason: what is
/// worth catching is somebody *editing one of these*, and a stalled siege is
/// invisible in every test that does not run one for twenty seconds.
const _: () = assert!(REACH > crate::enemy::CLOSEST_STAND);

/// A building: where it stands, how big it is, and how long it is still being
/// left alone for.
///
/// The size is carried rather than read back off the model for
/// [`crate::pylon::Pylon`]'s reason -- the number the fight is resolved against
/// has to be the number the thing was placed with -- and it is a cylinder
/// because that is what every other body in this game is resolved as.
#[derive(Component)]
pub struct Structure {
    /// How wide it is, which is what a blow has to cover to land.
    pub radius: f32,
    /// How tall it is, which is where its health bar is hung.
    pub height: f32,
    /// How long it still cannot be hurt for. See [`RECOVERY`].
    pub hurt_left: f32,
}

impl Structure {
    pub fn new(radius: f32, height: f32) -> Self {
        Self {
            radius,
            height,
            hurt_left: 0.0,
        }
    }
}

/// Whether a blow thrown from `from` by a body of `girth` lands on this
/// building.
///
/// Horizontal only, and from surface to surface. The vertical is deliberately
/// not tested: a warp pipe is three metres tall and a pylon eight, and every
/// attacker in this game stands on the ground -- adding a height band would
/// only ever be a way for something standing at the foot of a mast to miss it.
pub fn in_reach(from: Vec3, girth: f32, at: Vec3, structure: &Structure) -> bool {
    let apart = at - from;
    let span = structure.radius + girth + REACH;
    Vec3::new(apart.x, 0.0, apart.z).length_squared() <= span * span
}

/// Lands one of the crowd's blows on a building, honouring its recovery window.
///
/// `None` when the window is still open, so the caller can tell a hit from a
/// blow that arrived during somebody else's turn and stay quiet about it.
/// Despawning is the caller's, because what it wants to do afterwards -- raise a
/// threat, spend a bullet -- differs at every site.
///
/// Discrete blows do not come through here: a sword swing, a Mario's punch and a
/// bullet each spend [`Health::hurt`] directly. See the module preamble.
pub fn hit(structure: &mut Structure, health: &mut Health, amount: i32) -> Option<bool> {
    if structure.hurt_left > 0.0 {
        return None;
    }
    structure.hurt_left = RECOVERY;
    Some(health.hurt(amount))
}

/// Counts every building's recovery window down, once a tick.
///
/// Its own system rather than a line at the top of [`siege`], which iterates
/// *enemies* and never visits a building nothing is currently standing on: a
/// window opened by the last ant of a broken siege would otherwise stay open
/// forever, and the next one to arrive would find the mast immune.
pub fn recover(mut buildings: Query<&mut Structure>) {
    for mut structure in &mut buildings {
        structure.hurt_left = (structure.hurt_left - FIXED_DT).max(0.0);
    }
}

/// The crowd wearing a building down.
///
/// [`crate::enemy::maul`]'s shape exactly, pointed at buildings instead of
/// Marios: the loop is over *enemies* and does nothing at all for one whose
/// [`Aggro::target`] is not a building, so the thousands chasing the player cost
/// an archetype check each and no distance work whatever.
///
/// An enemy that knocks a mast over raises a threat where it fell, which is what
/// brings the squad -- and the player, who is on the same ledger -- to the hole
/// in the network rather than leaving it to be found later.
pub fn siege(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut threats: ResMut<Threats>,
    mut buildings: Query<(&Transform, &mut Structure, &mut Health), Without<Enemy>>,
    enemies: Query<(Entity, &Enemy, &Transform, &Aggro), Without<Structure>>,
) {
    for (enemy, body, transform, aggro) in &enemies {
        let Some(target) = aggro.target else {
            continue;
        };
        let Ok((at, mut structure, mut health)) = buildings.get_mut(target) else {
            continue;
        };
        let girth = body.kind.body().0;
        if !in_reach(transform.translation, girth, at.translation, &structure) {
            continue;
        }
        let Some(felled) = hit(&mut structure, &mut health, body.kind.damage()) else {
            continue;
        };
        if felled {
            commands.entity(target).despawn();
            sounds.push_at(Sfx::Defeat, at.translation);
            threats.kill(enemy, at.translation);
        } else {
            sounds.push_at(Sfx::Hurt, at.translation);
        }
    }
}

/// The player knocking one over.
///
/// Only buildings on the other side: swinging a sword about in your own base
/// should not cost you the network. That check is the one thing this does that
/// [`siege`] does not need, because an enemy never targets its own side in the
/// first place.
///
/// **One swing is one blow, and the `Local` is what makes that true.**
/// `Controller::attack_left` is a *window*, not an edge -- it is set to a little
/// over half a second and counted down -- so a system that acted on it being
/// positive would land a blow every tick it was open. Against a creature that
/// was invisible, because [`crate::health::PLAYER_DAMAGE`] one-shots everything
/// the game places; against a warp pipe it is the difference between six swings
/// and one. So the rising edge is taken here, the same shape
/// [`crate::input::InputState`] latches a press with and for the same reason.
pub fn demolish(
    mut commands: Commands,
    mut sounds: ResMut<SoundQueue>,
    mut threats: ResMut<Threats>,
    mut swinging: Local<bool>,
    player: Query<(Entity, &Transform, &Controller, &Side), With<Player>>,
    mut buildings: Query<(Entity, &Transform, &Side, &Structure, &mut Health), Without<Player>>,
) {
    let Ok((luna, transform, controller, mine)) = player.single() else {
        // A world with no player has no swing in progress either, or the first
        // frame after he respawns would count as one.
        *swinging = false;
        return;
    };
    let fresh = controller.attack_left > 0.0 && !*swinging;
    *swinging = controller.attack_left > 0.0;
    if !fresh {
        return;
    }
    let here = transform.translation;
    for (entity, at, side, structure, mut health) in &mut buildings {
        if side == mine {
            continue;
        }
        if !in_reach(here, PLAYER_RADIUS, at.translation, structure) {
            continue;
        }
        // Straight against the pool, no window: a swing is a discrete blow, and
        // one that arrived while a crowd happened to be mid-turn would be a
        // sword that silently does nothing in exactly the fight it is for.
        let felled = health.hurt(health::PLAYER_DAMAGE);
        if felled {
            commands.entity(entity).despawn();
            sounds.push_at(Sfx::Defeat, at.translation);
        } else {
            sounds.push_at(Sfx::Hurt, at.translation);
        }
        // On the same ledger as everything else he does: knocking a nest's pipe
        // down in the middle of its own brood is exactly the moment the brood
        // ought to decide he is the problem.
        threats.kill(luna, at.translation);
        // One building a tick. A sword is not an area attack, and a pipe
        // standing against a mast should not take the same swing twice.
        return;
    }
}

/// How far clear of the top of a building its health bar floats.
///
/// The same gap `health::BAR_CLEARANCE` gives an actor, written here rather than
/// borrowed from there because the two are answering about different things and
/// keeping one number in step with the other across two modules is the kind of
/// coupling this codebase spends its comments refusing.
const BAR_CLEARANCE: f32 = 0.2;

/// Where a building's health bar hangs, in world space.
///
/// Its own function because [`crate::health::draw_unit_bars`] wants it and so
/// does anything else that ever points at one, and because the answer for a
/// building is not the answer for an actor: an actor's bar goes over its head at
/// a height nobody authored, and a building's goes over the top of a thing whose
/// height was measured off its own file.
pub fn head(at: Vec3, structure: &Structure) -> Vec3 {
    at + Vec3::Y * (structure.height + BAR_CLEARANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mast() -> Structure {
        Structure::new(1.0, 8.0)
    }

    #[test]
    fn a_blow_reaches_as_far_as_a_chaser_is_allowed_to_stand_off() {
        // The same rule the `const` block states, from the other end: an ant
        // parked at the widest berth `Quirk::stand_off` hands out is an ant that
        // can still land on what it walked to.
        let pipe = Structure::new(2.5, 3.0);
        let girth = 2.5;
        let parked = pipe.radius + girth + crate::enemy::CLOSEST_STAND;
        assert!(in_reach(
            Vec3::new(parked, 0.0, 0.0),
            girth,
            Vec3::ZERO,
            &pipe
        ));
    }

    #[test]
    fn a_blow_reaches_a_buildings_surface_rather_than_its_middle() {
        let structure = mast();
        let at = Vec3::ZERO;
        // Standing against it, and standing a stride past the edge of it.
        assert!(in_reach(Vec3::new(2.0, 0.0, 0.0), 0.4, at, &structure));
        // And well outside it, which the same reach on a wider building would
        // still cover -- that is the point of measuring from the surface.
        assert!(!in_reach(Vec3::new(8.0, 0.0, 0.0), 0.4, at, &structure));
        let pipe = Structure::new(5.0, 3.0);
        assert!(in_reach(Vec3::new(8.0, 0.0, 0.0), 0.4, at, &pipe));
    }

    #[test]
    fn height_is_not_part_of_reaching_one() {
        // Something standing at the foot of a mast is hitting the mast, whatever
        // the mast's own height says about where its middle is.
        let structure = mast();
        assert!(in_reach(
            Vec3::new(1.5, 6.0, 0.0),
            0.4,
            Vec3::ZERO,
            &structure
        ));
    }

    #[test]
    fn the_recovery_window_makes_a_besieging_crowd_take_turns() {
        let mut structure = mast();
        let mut health = Health::new(health::PYLON_HEALTH);
        assert_eq!(hit(&mut structure, &mut health, 10), Some(false));
        assert_eq!(health.current, health::PYLON_HEALTH - 10);
        // The second ant of the crowd, on the same tick: refused, and nothing
        // is spent. Twenty of them standing round a mast do a rate rather than
        // a number that grows with how many arrived.
        assert_eq!(hit(&mut structure, &mut health, 10), None);
        assert_eq!(health.current, health::PYLON_HEALTH - 10);
        // Wound down, and it takes the next one.
        structure.hurt_left = 0.0;
        assert_eq!(hit(&mut structure, &mut health, 10), Some(false));
        assert_eq!(health.current, health::PYLON_HEALTH - 20);
    }

    #[test]
    fn a_building_that_runs_out_reports_that_it_has() {
        let mut structure = mast();
        let mut health = Health::new(6);
        assert_eq!(hit(&mut structure, &mut health, 4), Some(false));
        structure.hurt_left = 0.0;
        assert_eq!(hit(&mut structure, &mut health, 4), Some(true));
        assert!(health.dead());
    }

    #[test]
    fn a_mast_falls_to_a_crowd_in_seconds_rather_than_instantly_or_never() {
        // The sentence [`crate::health::PYLON_HEALTH`] is written to make true:
        // a pylon being stood on by ants is a thing you have time to answer, and
        // the answer is flying back across the map -- so the window is tens of
        // seconds rather than a handful.
        let blows = (health::PYLON_HEALTH + health::ANT_DAMAGE - 1) / health::ANT_DAMAGE;
        let seconds = blows as f32 * RECOVERY;
        assert!(seconds > 8.0 && seconds < 30.0, "{seconds} seconds");
    }
}
