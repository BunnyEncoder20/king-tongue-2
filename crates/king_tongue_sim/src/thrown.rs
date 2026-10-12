//! The Thrown state: when an Enemy stops being a target and becomes a weapon,
//! and what its collisions do once it has.
//!
//! GLOSSARY: Thrown is "the state of an Enemy that has lost control of its own
//! momentum, having been pulled hard enough by a Tongue or struck hard enough
//! to be weaponized. A Thrown Enemy destroys lesser Enemies it hits and is
//! itself destroyed by terrain." ADR-0011 is the contract and ADR-0015 holds
//! the two questions it left open.
//!
//! The whole module is pure, like [`crate::combat`] beside it. Nothing here
//! reads or writes `World` state: it takes geometry, momentum and mass in and
//! answers what happened, which is what lets every row of the matrix be
//! asserted on directly and keeps the tick's only mutation in one place.
//!
//! The promotion is read from a force, so it is a **pure function of position
//! and velocity** and nothing else, which it has to be: ADR-0007's integrator
//! evaluates that force twice per Sim Tick. The state change it decides is
//! applied once, after the integration step, exactly like an impulse.

use crate::combat::{self, ENEMY_HALF_EXTENTS};
use glam::Vec2;

/// The tension above which a Tongue's Anchor loses control of its own momentum
/// and becomes Thrown, in world units per second squared.
///
/// **An acceleration and not a force, which is how the mass differential
/// enters.** ADR-0011 derives the tension from "the hero's `pull_strength`, the
/// relative velocities, and the mass differential": the first two are already
/// the Tongue's force law ([`crate::tongue_force`] takes `pull_strength` and
/// damps against the relative radial velocity), and dividing by the Anchor's
/// own mass is the third. Comparing a bare force would make a heavy Enemy
/// exactly as easy to Throw as a light one, which is the one thing the mass
/// differential is in that sentence to prevent.
///
/// A starting point for device tuning rather than a final value, like
/// [`crate::combat::KICK_THRESHOLD`]. At the baseline hero's
/// `DEFAULT_PULL_STRENGTH` of 40 a Stretch of 12.5 reaches it, so a Tongue
/// Attached at 13.5 units Throws a mass 1 Enemy and anything closer tracks it.
/// Deliberately out of reach of an ordinary grab: the tracking band is the
/// common case and ADR-0011 gives it rules of its own (environmental immunity),
/// so a threshold a routine Attach cleared would delete that band. What does
/// reach it is loading the Stretch, which ADR-0003 already names as the
/// deliberate manoeuvre, grabbing something light, or holding one Enemy with
/// both slots, since two pulls sum.
///
/// Comfortably under the per-Tongue [`crate::DEFAULT_FORCE_CEILING`] of 1200,
/// so the threshold is reachable without the clamp being involved at all.
pub const THROW_TENSION: f32 = 500.0;

/// A compile-time guard on the paragraph above: the threshold has to be
/// reachable without the per-Tongue clamp being involved.
///
/// A `const` assertion rather than a test, because it is a relation between two
/// constants and nothing a simulation could run would make it more true. Raise
/// either constant past the other and this fails to build, which is the right
/// moment to notice: a threshold above the ceiling would make the Thrown state
/// unreachable for a mass 1 Enemy on one Tongue, and nothing else in the suite
/// would say so.
const _: () = assert!(THROW_TENSION < crate::DEFAULT_FORCE_CEILING);

/// The tension one Tongue force puts on a body of `mass`.
///
/// Force per unit mass, which is the body's own acceleration from the Tongue.
/// See [`THROW_TENSION`] for why the mass is in here.
pub fn tension(tongue_force: Vec2, mass: f32) -> f32 {
    if mass <= 0.0 {
        // A level with a non-positive mass is rejected by `Level::validate`, so
        // this is unreachable from level data. Answered rather than divided by
        // anyway: an infinite tension would Throw a body that cannot exist.
        return 0.0;
    }
    tongue_force.length() / mass
}

/// Whether a tension has taken a body's control of its own momentum away, which
/// is to say whether it promotes an Enemy to Thrown.
///
/// **The single statement of ADR-0011's promotion boundary.** Everything that
/// decides the promotion comes through here: [`exceeds_tension`] below for a
/// force and a mass, and `World::resolve_thrown` for the tension its half-step
/// already recorded. The two signatures differ because the tick has the scalar
/// in hand and a pure caller has the force, and the comparison lived in both
/// places before this function existed; one rule stated twice is one rule that
/// can be retuned in one place and not the other.
///
/// At the threshold exactly it promotes: ADR-0011's bands are written "above
/// the Kick threshold" and "at or above `toughness`", and this follows the
/// second, because a boundary that belonged to neither side would be a band of
/// its own.
pub fn promotes(tension: f32) -> bool {
    tension >= THROW_TENSION
}

/// Whether a body under `tongue_force` has lost control of its own momentum.
///
/// The force-and-mass form of [`promotes`], which holds the rule.
pub fn exceeds_tension(tongue_force: Vec2, mass: f32) -> bool {
    promotes(tension(tongue_force, mass))
}

/// The circle a Thrown Enemy sweeps with.
///
/// Its own box's inscribed circle. The projectile is a box like every other
/// Enemy, but the integrator already sweeps every body against terrain as a
/// circle of [`crate::PLAYER_RADIUS`], so treating it as a circle here is the
/// approximation this simulation has been making for every body since ticket
/// #11 rather than a new one. A box-against-box sweep would be a second
/// geometry to keep in step with the first for an accuracy Placeholder Geometry
/// cannot show.
const THROWN_RADIUS: f32 = ENEMY_HALF_EXTENTS.x;

/// Which side of a weight class a Thrown Enemy's target is on.
///
/// Mass and not `Toughness`: ADR-0011 writes the two rows as "a lesser Enemy"
/// and "an equal weight class", and a weight class is a weight. `Toughness` is
/// resistance to the hero's momentum and is already spoken for by the three
/// bands in [`crate::combat`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeightClass {
    /// Lighter than the projectile by more than [`WEIGHT_CLASS_BAND`].
    Lesser,
    /// Within the band, or heavier. ADR-0015 records why heavier folds in here
    /// rather than becoming a fourth row.
    Equal,
}

/// How far below a Thrown Enemy's own mass a target can be and still count as
/// the same weight class, as a fraction.
///
/// A band and not an equality test, because mass is a float that arrives from
/// level data: two Enemies authored as the same weight would otherwise fall
/// into different rows of the matrix on a last-bit difference, and which row
/// they landed in decides whether the player keeps a weapon or loses one.
///
/// A tenth is wide enough to absorb that and narrow enough that a weight class
/// still means something: at a tenth, mass 0.9 against mass 1.0 is an equal
/// fight and mass 0.8 is not.
pub const WEIGHT_CLASS_BAND: f32 = 0.1;

/// Which row of ADR-0011's matrix a target's weight puts it in.
pub fn weight_class(projectile_mass: f32, target_mass: f32) -> WeightClass {
    if target_mass < projectile_mass * (1.0 - WEIGHT_CLASS_BAND) {
        WeightClass::Lesser
    } else {
        WeightClass::Equal
    }
}

/// The fraction of its speed a Thrown Enemy loses per unit of target mass it
/// ploughs through, relative to its own mass.
///
/// ADR-0011 asks for "minor proportional velocity damping" and leaves the
/// number open. Proportional to the **target's weight relative to the
/// projectile's**, because that is the only thing in the collision it could be
/// proportional to that is not arbitrary: ploughing through something a tenth
/// of your weight should cost a tenth of what ploughing through something at
/// the edge of your own weight class does.
///
/// A fifth is the ceiling that follows, since a target at or above the band is
/// mutual destruction and never reaches this row at all: the most a surviving
/// projectile can lose is just under `0.2 * 0.9`, so it keeps at least four
/// fifths of its speed. That is "minor" in the sense the ADR means, which is
/// that ploughing is a reward and not a cost.
pub const PLOUGH_DRAG: f32 = 0.2;

/// What one Thrown Enemy striking one other Enemy did. One row of ADR-0011's
/// inverted matrix each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Plough {
    /// A lesser Enemy: the target is destroyed and the projectile flies on,
    /// carrying the velocity in here.
    ///
    /// Its link and chain state are untouched, which is the ADR's wording and
    /// is why this carries a velocity and nothing else: there is nothing to say
    /// about the Tongue, so saying nothing is the implementation.
    Through { vel: Vec2 },
    /// An equal weight class: both bodies are vaporized. The player's Anchor
    /// slot goes with the projectile under ADR-0014, which generalised exactly
    /// this row.
    Mutual,
}

/// A Thrown Enemy's motion over one Sim Tick, as the matrix sees it.
#[derive(Clone, Copy, Debug)]
pub struct Projectile {
    /// Where it was at the start of the tick, before integration.
    pub start: Vec2,
    /// Where it ended, after integration and terrain contact.
    pub end: Vec2,
    pub vel: Vec2,
    pub mass: f32,
}

/// Another Enemy in a Thrown Enemy's path.
#[derive(Clone, Copy, Debug)]
pub struct Victim {
    /// Where it was at the start of the tick, before integration.
    ///
    /// Here for the reason [`crate::combat::EnemyTarget::start`] is: a
    /// collision is a question about both paths. Sweeping the projectile's path
    /// against the victim's end box left the victim's own motion unswept, so a
    /// fast victim crossed a slow or parked weapon untouched.
    pub start: Vec2,
    /// Where it ended, after integration and terrain contact.
    pub end: Vec2,
    pub vel: Vec2,
    pub mass: f32,
}

/// Resolves one tick of a Thrown Enemy's pass against one other Enemy.
///
/// `None` means nothing happened: the paths did not meet, or they met while
/// already separating.
///
/// **There is no momentum gate here, and its absence is deliberate.** The three
/// bands in [`crate::combat`] gate the hero's impacts on normal momentum
/// because the hero is always moving and most of that motion is not an attack.
/// A Thrown Enemy had to clear [`THROW_TENSION`] to exist at all, so the state
/// *is* the gate, and a second threshold on top of it would be a fourth band
/// ADR-0011 does not have.
pub fn plough(projectile: &Projectile, victim: &Victim) -> Option<Plough> {
    // Relative, so either body's motion closes the tunnel. The shared sweep
    // holds why; this row of the matrix had the identical shape the hero's
    // impacts did, and a fast victim could dodge a weapon by standing in front
    // of a slow one.
    let contact = combat::swept_contact_between(
        projectile.start,
        projectile.end,
        THROWN_RADIUS,
        victim.start,
        victim.end,
        ENEMY_HALF_EXTENTS,
    )?;

    // Already separating. The sign is kept rather than taken as an absolute
    // value for the same reason [`crate::combat::resolve_contact`] keeps it: a
    // projectile leaving a body it has just passed is not striking it again,
    // and the mutual row would otherwise destroy a weapon for flying away from
    // something.
    let closing = (victim.vel - projectile.vel).dot(contact.normal);
    if closing <= 0.0 {
        return None;
    }

    match weight_class(projectile.mass, victim.mass) {
        WeightClass::Lesser => Some(Plough::Through {
            // Scaling the velocity rather than subtracting along the normal, so
            // the drag only ever shortens the vector and never turns it. Same
            // choice as [`crate::combat::momentum_drain`] and for the same
            // reason: a weapon that changed direction on every body it passed
            // through could not be aimed.
            vel: projectile.vel * (1.0 - PLOUGH_DRAG * victim.mass / projectile.mass),
        }),
        WeightClass::Equal => Some(Plough::Mutual),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_PULL_STRENGTH, REST_LENGTH};

    /// A projectile crossing the origin from the left, fast enough that one
    /// tick's motion carries it clean through a box sitting there.
    fn projectile(mass: f32) -> Projectile {
        Projectile {
            start: Vec2::new(-2.0, 0.0),
            end: Vec2::new(2.0, 0.0),
            vel: Vec2::new(240.0, 0.0),
            mass,
        }
    }

    /// A victim at the origin, in the path of [`projectile`].
    fn victim(mass: f32) -> Victim {
        Victim {
            start: Vec2::ZERO,
            end: Vec2::ZERO,
            vel: Vec2::ZERO,
            mass,
        }
    }

    #[test]
    fn the_tension_is_a_force_over_a_mass_so_a_heavier_enemy_resists() {
        // ADR-0011 derives the tension from the hero's `pull_strength`, the
        // relative velocities, and the mass differential. The first two are the
        // Tongue's own force law; this is the third, and without it a heavy
        // Enemy would be exactly as easy to Throw as a light one.
        let force = Vec2::new(0.0, THROW_TENSION);
        assert!(
            exceeds_tension(force, 1.0),
            "a force equal to the threshold must Throw a unit mass"
        );
        assert!(
            !exceeds_tension(force, 2.0),
            "twice the mass must halve the tension and stay below the threshold"
        );
        // The direction is irrelevant: a yank sideways is as much a yank as one
        // along an axis, so the magnitude is what counts.
        let diagonal = Vec2::splat(THROW_TENSION / 2.0f32.sqrt());
        assert!(
            (tension(diagonal, 1.0) - THROW_TENSION).abs() < 1e-3,
            "a diagonal force of the same magnitude read {}",
            tension(diagonal, 1.0)
        );
    }

    #[test]
    fn the_threshold_is_reachable_by_a_loaded_stretch_and_not_by_an_ordinary_attach() {
        // The one gameplay claim the constant makes, and it is checkable rather
        // than rhetorical. ADR-0011 gives the tracking band rules of its own,
        // so a threshold an ordinary Attach cleared would delete that band, and
        // one nothing could reach would delete the Thrown state instead.
        //
        // Both ends are pure Hooke's law at rest, where the damping term is
        // identically zero, so these are exact rather than measured.
        let tension_at = |length: f32, mass: f32| {
            tension(
                Vec2::new(DEFAULT_PULL_STRENGTH * (length - REST_LENGTH), 0.0),
                mass,
            )
        };
        assert!(
            tension_at(8.0, 1.0) < THROW_TENSION,
            "an eight unit Attach on a baseline Enemy must merely track it, got {}",
            tension_at(8.0, 1.0)
        );
        assert!(
            tension_at(20.0, 1.0) >= THROW_TENSION,
            "a twenty unit loaded Stretch must Throw a baseline Enemy, got {}",
            tension_at(20.0, 1.0)
        );
        // And a light Enemy goes at a touch, which is the mass differential
        // being the mechanic rather than a correction.
        assert!(
            tension_at(4.0, 0.2) >= THROW_TENSION,
            "a quarter-weight Enemy must Throw at close range, got {}",
            tension_at(4.0, 0.2)
        );
    }

    #[test]
    fn a_non_positive_mass_reads_no_tension_rather_than_an_infinite_one() {
        // Hostile input. `Level::validate` rejects a non-positive mass, so this
        // is unreachable from level data, but a division that answered infinity
        // would Throw a body that cannot exist, and the answer would arrive as
        // `true` rather than as a panic.
        assert_eq!(tension(Vec2::new(1.0, 0.0), 0.0), 0.0);
        assert!(!exceeds_tension(Vec2::new(1e30, 0.0), 0.0));
    }

    #[test]
    fn the_promotion_boundary_includes_the_threshold_exactly() {
        // The one rule `World::resolve_thrown` and `exceeds_tension` both read.
        // ADR-0011's bands are written "at or above `toughness`", so the
        // threshold itself promotes: a boundary belonging to neither side would
        // be a band of its own.
        //
        // Literals rather than a recomputation of the comparison: the
        // neighbours of the threshold are named outright so the test cannot
        // agree with the code by construction.
        assert!(promotes(500.0), "the threshold itself must promote");
        assert!(promotes(500.001), "above the threshold must promote");
        assert!(
            !promotes(499.999),
            "just below the threshold must stay a target"
        );
        assert!(!promotes(0.0), "a slack Tongue must not promote");
        // And the constant really is the number those literals assume, so this
        // test cannot quietly stop testing the boundary if it is retuned.
        assert_eq!(
            THROW_TENSION, 500.0,
            "retune the literals above with the threshold"
        );
        // The force-and-mass entry point reads the same boundary, measured
        // independently: a force of 500 on a unit mass is a tension of 500.
        assert!(exceeds_tension(Vec2::new(0.0, 500.0), 1.0));
        assert!(!exceeds_tension(Vec2::new(0.0, 499.0), 1.0));
    }

    #[test]
    fn the_weight_class_band_separates_lesser_from_equal_and_folds_heavier_in() {
        assert_eq!(weight_class(1.0, 0.5), WeightClass::Lesser);
        // Identical masses are the row that matters, because `from_ascii` and
        // the shipped scene both author Enemies at one mass: two of them must
        // be an equal fight and not a coin toss on a last bit.
        assert_eq!(weight_class(1.0, 1.0), WeightClass::Equal);
        assert_eq!(
            weight_class(1.0, 1.0 - WEIGHT_CLASS_BAND * 0.5),
            WeightClass::Equal
        );
        assert_eq!(
            weight_class(1.0, 1.0 - WEIGHT_CLASS_BAND * 2.0),
            WeightClass::Lesser
        );
        // Heavier folds into the equal row rather than becoming a fourth one.
        // ADR-0015 records why: the matrix has three rows, and the risk of
        // using a weapon must not fall as the target gets heavier.
        assert_eq!(weight_class(1.0, 50.0), WeightClass::Equal);
        // Scale-free, so the band is a weight class and not an absolute mass.
        assert_eq!(weight_class(10.0, 5.0), WeightClass::Lesser);
        assert_eq!(weight_class(0.1, 0.05), WeightClass::Lesser);
    }

    #[test]
    fn a_pass_that_meets_nothing_resolves_nothing() {
        let mut aside = victim(0.1);
        aside.start = Vec2::new(0.0, 10.0);
        aside.end = aside.start;
        assert_eq!(plough(&projectile(1.0), &aside), None);
    }

    #[test]
    fn a_target_already_separating_is_not_struck() {
        // Hostile input, and the same guard `combat::resolve_contact` keeps:
        // the closing speed is signed rather than taken as an absolute value. A
        // projectile leaving a body it has just passed is not striking it
        // again, and without the sign the mutual row would vaporize a weapon
        // for flying away from something.
        let mut fleeing = victim(1.0);
        // Running away faster than the projectile is chasing.
        fleeing.vel = Vec2::new(500.0, 0.0);
        assert_eq!(plough(&projectile(1.0), &fleeing), None);

        // Mutation check: the same geometry with the target standing still is
        // struck, so the test above is not passing because the paths missed.
        assert_eq!(plough(&projectile(1.0), &victim(1.0)), Some(Plough::Mutual));
    }

    #[test]
    fn a_fast_victim_cannot_cross_a_stationary_projectile_untouched() {
        // The same defect the hero's impacts had, in the same shape: only the
        // projectile's path was swept, against the victim's end position, so a
        // victim crossing 3.1 units in one Sim Tick read as nothing at all. The
        // geometry it is asserted over lives in
        // `combat::tests::the_sweep_is_relative_so_either_body_moving_closes_the_tunnel`;
        // this is the matrix row reading it.
        let parked = Projectile {
            start: Vec2::new(9.5, 0.0),
            end: Vec2::new(9.5, 0.0),
            vel: Vec2::ZERO,
            mass: 1.0,
        };
        let crossing = Victim {
            start: Vec2::new(7.7, 0.0),
            end: Vec2::new(10.8, 0.0),
            vel: Vec2::new(186.0, 0.0),
            mass: 1.0,
        };
        assert_eq!(plough(&parked, &crossing), Some(Plough::Mutual));
    }

    #[test]
    fn ploughing_a_lesser_enemy_shortens_the_projectiles_velocity_without_turning_it() {
        let shot = projectile(1.0);
        let Some(Plough::Through { vel }) = plough(&shot, &victim(0.25)) else {
            panic!("a quarter-weight target must be the lesser row");
        };
        // Direction preserved exactly. Subtracting along the contact normal
        // instead would turn the weapon, and a weapon that changed direction on
        // every body it passed through could not be aimed.
        assert_eq!(
            vel.normalize().to_array(),
            shot.vel.normalize().to_array(),
            "the drag turned the projectile"
        );
        let expected = 1.0 - PLOUGH_DRAG * 0.25;
        assert!(
            (vel.length() / shot.vel.length() - expected).abs() < 1e-6,
            "kept {} of its speed against the analytic {expected}",
            vel.length() / shot.vel.length()
        );
        // Proportional to the target's weight: half the target, half the cost.
        let Some(Plough::Through { vel: lighter }) = plough(&shot, &victim(0.125)) else {
            panic!("an eighth-weight target must be the lesser row");
        };
        let lost = |v: Vec2| 1.0 - v.length() / shot.vel.length();
        assert!(
            (lost(vel) / lost(lighter) - 2.0).abs() < 1e-3,
            "halving the target's mass changed the cost by {}x and not 2x",
            lost(vel) / lost(lighter)
        );
    }

    #[test]
    fn the_whole_lesser_row_leaves_a_weapon_worth_using() {
        // The ADR calls the damping "minor", and the sharpest way to say what
        // that means is over the entire row: the heaviest target that can reach
        // it at all must still leave the projectile most of its speed.
        let shot = projectile(1.0);
        let worst = victim(1.0 - WEIGHT_CLASS_BAND - 1e-4);
        let Some(Plough::Through { vel }) = plough(&shot, &worst) else {
            panic!("the heaviest lesser target must still be the lesser row");
        };
        assert!(
            vel.length() / shot.vel.length() > 0.8,
            "the worst case left only {} of the speed",
            vel.length() / shot.vel.length()
        );
    }
}
