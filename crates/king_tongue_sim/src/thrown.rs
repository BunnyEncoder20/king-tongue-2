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
use crate::level::Aabb;
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

/// Whether a body under `tongue_force` has lost control of its own momentum.
///
/// At the threshold exactly it has: ADR-0011's bands are written "above the
/// Kick threshold" and "at or above `toughness`", and this follows the second,
/// because a boundary that belonged to neither side would be a band of its own.
pub fn exceeds_tension(tongue_force: Vec2, mass: f32) -> bool {
    tension(tongue_force, mass) >= THROW_TENSION
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
    pub pos: Vec2,
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
    let box_ = Aabb::new(victim.pos, ENEMY_HALF_EXTENTS);
    let motion = projectile.end - projectile.start;
    let contact = combat::swept_contact(projectile.start, motion, THROWN_RADIUS, box_)?;

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
