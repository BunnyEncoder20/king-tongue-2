//! Combat: the hero's bounding circle against an Enemy's bounding box, and the
//! three bands one impact resolves into.
//!
//! ADR-0011 is the contract. Everything here is momentum: there is no health,
//! no damage number, and no attack input, so an Enemy's `Toughness` is
//! denominated in the same units as the thing hitting it and no conversion
//! happens at impact.
//!
//! The whole module is pure. Nothing in it reads or writes `World` state: it
//! takes geometry and momentum in and returns what an impact did, which is what
//! lets the bands be asserted on directly and keeps the tick's only mutation in
//! one place. ADR-0007's integrator evaluates acceleration twice per Sim Tick,
//! so a collision response must never be reachable from a force; these are
//! impulses, applied once, after the integration step.

use crate::level::Aabb;
use glam::Vec2;

/// Half the extent of an Enemy's collision box.
///
/// ADR-0011 fixes the shape of the test (circle against box) but not the size.
/// One unit square matches the cell an ASCII sketch spawns an Enemy into and the
/// Placeholder Geometry the Shell draws for one, so a sketched scene means what
/// it looks like.
pub const ENEMY_HALF_EXTENTS: Vec2 = Vec2::splat(0.5);

/// Below this much normal momentum an impact is a Kick rather than a Slam.
///
/// Momentum units, like `toughness`. A starting point for device tuning and not
/// a final value, per the spec: at the baseline hero's mass of 1 this makes
/// anything under two units per second of closing speed a recoverable bump.
pub const KICK_THRESHOLD: f32 = 2.0;

/// The `Toughness` one Kick spends. Fixed and minimal, per ADR-0011: a Kick is
/// a recoverable mistake, not a chip strategy.
pub const KICK_DAMAGE: f32 = 1.0;

/// Fraction of its approach speed a Kick returns to the hero. One is fully
/// elastic, which is what "an elastic repel impulse that restores distance"
/// asks for: the hero leaves at the speed it arrived.
pub const KICK_RESTITUTION: f32 = 1.0;

/// At or above this incoming momentum magnitude an impact drains nothing: the
/// hero keeps its direction and its speed and passes straight through.
///
/// Deliberately above a one-shot of the default Enemy, so the ceiling reads as
/// becoming unstoppable rather than as a cheaper way to kill things.
pub const PASS_THROUGH_MOMENTUM: f32 = 150.0;

/// The hardest a single impact may hit the hero's momentum, as a fraction of
/// the incoming vector. ADR-0011: one unlucky hit must not end a run of Slams.
pub const MAX_DRAIN_FRACTION: f32 = 0.5;

/// Where a swept circle first meets a box, and the surface direction there.
///
/// Only `normal` survives into a shipping build. `t` and `pos` are the geometry
/// the sweep works in and nothing downstream of [`resolve_contact`] reads
/// either: the gate is the normal, the impulse is along the normal, and no band
/// cares where along the motion the meeting happened. They are kept for the
/// tests that assert the sweep found the right point, gated like
/// [`crate::level::Level::from_ascii`] so they cannot reach a build that ships.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    /// Fraction of the motion at which contact happens, in `0.0..=1.0`.
    #[cfg(any(test, feature = "test-util"))]
    pub t: f32,
    /// The circle's centre at contact.
    #[cfg(any(test, feature = "test-util"))]
    pub pos: Vec2,
    /// Unit normal, pointing out of the box toward the circle.
    pub normal: Vec2,
}

/// Which of ADR-0011's three bands an impact fell into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImpactBand {
    /// Below [`KICK_THRESHOLD`]: fixed minimal damage and an elastic repel.
    Kick,
    /// Above the Kick threshold and below the Enemy's remaining `Toughness`:
    /// the normal momentum is spent against the pool and the Enemy survives.
    Slam,
    /// At or above the remaining `Toughness`: destroyed outright.
    Destroy,
}

/// The hero's motion over one Sim Tick, as the impact sees it.
#[derive(Clone, Copy, Debug)]
pub struct HeroPass {
    /// Where the hero was at the start of the tick, before integration.
    pub start: Vec2,
    /// Where it ended, after integration and terrain contact.
    pub end: Vec2,
    pub vel: Vec2,
    pub mass: f32,
    pub radius: f32,
}

/// One Enemy as a target of an impact.
#[derive(Clone, Copy, Debug)]
pub struct EnemyTarget {
    /// Where the Enemy was at the start of the tick, before integration.
    ///
    /// Here because a collision is a question about the two paths that were
    /// travelled and not about where either body ended up. Sweeping the hero's
    /// path against the Enemy's *end* box left the Enemy's own motion unswept
    /// entirely, so a stationary hero was a zero-length sweep, which is a plain
    /// overlap test through a window about two units wide: an Enemy moving more
    /// than roughly that per Sim Tick crossed it untouched. See
    /// [`swept_contact_between`].
    pub start: Vec2,
    /// Where it ended, after integration and terrain contact.
    pub end: Vec2,
    pub vel: Vec2,
    pub mass: f32,
    /// Remaining `Toughness`, in momentum units.
    pub toughness: f32,
    /// Whether an impact has already been spent against the contact currently
    /// in progress.
    ///
    /// This is what makes one pass cost one hit. Without it a hero travelling
    /// slowly enough to spend several ticks inside an Enemy would spend its
    /// momentum against the pool once per tick, and "chip a heavy Enemy over
    /// several passes" would collapse into emptying any pool in a third of a
    /// second.
    ///
    /// **One meaning and not two, which it used to carry.** This is the
    /// allowance alone and never "the bodies are in contact": the two are
    /// different questions, because a tick can be in contact and register
    /// nothing at all. ADR-0011's fourth outcome beside the three bands has a
    /// separating body register nothing, and nothing is also what it spends, so
    /// a separating tick carries this through untouched rather than arming it.
    /// Arming it there armed a flag meaning "already spent" with nothing spent,
    /// and the next tick's genuine Slam was then silently discarded; a hero
    /// hanging on a Movable Anchor reaches that every spring cycle, because
    /// `REST_LENGTH` equals the hero's radius plus an Enemy's half extent.
    ///
    /// Whether the bodies are in contact is read off the sweep, where it is
    /// known exactly, so losing contact is what clears this.
    pub spent: bool,
}

/// What one tick of hero-against-Enemy resolution did.
///
/// Returned as the post-state rather than as a set of deltas so that the caller
/// does the same assignment in every band and cannot forget one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resolution {
    pub hero_vel: Vec2,
    pub enemy_vel: Vec2,
    /// The Enemy's remaining `Toughness`. Zero or below means destroyed.
    pub toughness: f32,
    /// The one-hit-per-contact allowance, to be carried into the next tick. See
    /// [`EnemyTarget::spent`] for why it is only ever that.
    pub spent: bool,
    /// Which band resolved, or `None` if nothing registered: no contact, the
    /// bodies separating, or a contact already spent on an earlier tick.
    pub band: Option<ImpactBand>,
}

impl Resolution {
    /// Nothing happened: the caller writes this back and the tick is unchanged
    /// apart from the contact flag.
    fn inert(hero: &HeroPass, enemy: &EnemyTarget, spent: bool) -> Self {
        Resolution {
            hero_vel: hero.vel,
            enemy_vel: enemy.vel,
            toughness: enemy.toughness,
            spent,
            band: None,
        }
    }

    /// Whether this resolution emptied the `Toughness` pool. GLOSSARY: reaching
    /// zero destroys the Enemy.
    ///
    /// Test scaffolding, gated like [`crate::level::Level::from_ascii`] so it
    /// cannot reach a build that ships. `World::resolve_impacts` writes the
    /// pool back and lets its own single `retain(|b| b.toughness > 0.0)` decide,
    /// which is how destruction stays one rule; a second reading of the same
    /// condition on the shipping surface is something that rule could come to
    /// disagree with.
    #[cfg(any(test, feature = "test-util"))]
    pub fn destroyed(&self) -> bool {
        self.toughness <= 0.0
    }
}

/// Resolves one tick of the hero's pass against one Enemy.
pub fn resolve_contact(hero: &HeroPass, enemy: &EnemyTarget) -> Resolution {
    let Some(contact) = swept_contact_between(
        hero.start,
        hero.end,
        hero.radius,
        enemy.start,
        enemy.end,
        ENEMY_HALF_EXTENTS,
    ) else {
        // Out of contact clears the allowance, which is what lets the next pass
        // register: contact ended, so the next one is a new contact.
        return Resolution::inert(hero, enemy, false);
    };

    // Closing speed along the normal, relative and signed. ADR-0011 used to
    // write the gate as `|m * v . n|` with `v` the hero's velocity, and its
    // "The gate is signed relative closing" section now records this form
    // instead: taking it relative to the Enemy is the same number whenever the
    // Enemy is at rest, and the honest one once an earlier impact has set it
    // moving. The sign is kept rather than taken as an absolute value so that
    // bodies already separating do not register: a hero leaving an Enemy it has
    // just Kicked is not hitting it again. That section also holds the measured
    // consequence, which is that a fleeing Enemy can land more than one band
    // below what the absolute form would have scored.
    let closing = (enemy.vel - hero.vel).dot(contact.normal);
    if closing <= 0.0 {
        // ADR-0011's fourth outcome: a separating body registers nothing. It
        // spends nothing either, so the allowance is carried through rather
        // than armed. See [`EnemyTarget::spent`].
        return Resolution::inert(hero, enemy, enemy.spent);
    }
    let normal_momentum = hero.mass * closing;

    if normal_momentum < KICK_THRESHOLD {
        // The elastic repel is a contact response and not an impact, so it is
        // applied on every tick of contact. That is what lets a hero come to
        // rest against an Enemy rather than sinking through it. Only the
        // `Toughness` it spends is held to once per contact.
        let separated = kick(hero, enemy, contact.normal);
        return Resolution {
            hero_vel: separated.0,
            enemy_vel: separated.1,
            toughness: if enemy.spent {
                enemy.toughness
            } else {
                enemy.toughness - KICK_DAMAGE
            },
            spent: true,
            band: Some(ImpactBand::Kick),
        };
    }

    if enemy.spent {
        // A Slam already spent on this contact. The hero carries straight on
        // through: a Slam does not bounce.
        return Resolution::inert(hero, enemy, true);
    }

    // The pool takes the whole normal momentum, or whatever is left of it.
    let spent = normal_momentum.min(enemy.toughness);
    let drain = momentum_drain(hero.mass * hero.vel, spent);
    Resolution {
        hero_vel: hero.vel - drain / hero.mass,
        // Whatever the hero loses, the Enemy gains, so a Slam below the
        // pass-through ceiling conserves momentum exactly. Above the ceiling it
        // deliberately does not: that is the hero becoming unstoppable.
        enemy_vel: enemy.vel + drain / enemy.mass,
        toughness: enemy.toughness - spent,
        spent: true,
        band: Some(if normal_momentum >= enemy.toughness {
            ImpactBand::Destroy
        } else {
            ImpactBand::Slam
        }),
    }
}

/// The momentum an impact takes off the hero, given what it spent.
///
/// Proportional to the `Toughness` actually consumed, so a trivial Enemy does
/// not cost what a heavy one does; capped at [`MAX_DRAIN_FRACTION`] of the
/// incoming vector; and zero at or above [`PASS_THROUGH_MOMENTUM`].
///
/// Scaling the incoming vector rather than subtracting along the normal is
/// deliberate. ADR-0011 caps the drain against "the incoming momentum vector"
/// and says that above the ceiling direction and speed are both preserved, so
/// the drain only ever shortens the vector and never turns it.
pub fn momentum_drain(momentum: Vec2, toughness_spent: f32) -> Vec2 {
    let incoming = momentum.length();
    if incoming >= PASS_THROUGH_MOMENTUM || incoming < 1e-6 {
        return Vec2::ZERO;
    }
    let fraction = (toughness_spent / incoming).clamp(0.0, MAX_DRAIN_FRACTION);
    momentum * fraction
}

/// The elastic repel of a Kick: a restitution impulse along the normal that
/// conserves momentum between the two bodies.
fn kick(hero: &HeroPass, enemy: &EnemyTarget, normal: Vec2) -> (Vec2, Vec2) {
    let closing = (enemy.vel - hero.vel).dot(normal);
    let inverse_mass = 1.0 / hero.mass + 1.0 / enemy.mass;
    let impulse = (1.0 + KICK_RESTITUTION) * closing / inverse_mass;
    (
        hero.vel + normal * (impulse / hero.mass),
        enemy.vel - normal * (impulse / enemy.mass),
    )
}

/// Whether two moving bodies, a circle and a box, met over one Sim Tick.
///
/// **Relative, and that is the whole point of it.** Sweeping one body's path
/// against the other's end-of-tick box leaves the second body's own motion
/// unswept, so a body that happens to be standing still is a zero-length sweep:
/// a plain overlap test through a window two radii plus two half extents wide,
/// which anything crossing more than that per Sim Tick passes through untouched.
/// Measured before this existed: an Enemy at 186 units per second went from 7.70
/// to 10.80 straight past a hero parked at 9.50 and nothing registered at all.
/// That is reachable without test scaffolding, because tension is force over
/// mass, so a Tongue can keep accelerating a heavy Enemy that can never reach
/// `THROW_TENSION` indefinitely.
///
/// Holding the box fixed at where it began and sweeping the *relative*
/// displacement is the same question asked in the box's own frame, and it is
/// symmetric: either body moving closes the tunnel, and which one is "the mover"
/// stops mattering. Only the relative path can be swept, because a sweep answers
/// "did these paths meet" and two independent motions have one relative path
/// between them.
///
/// The normal that comes back is a direction, so it means the same thing in
/// either frame and needs no converting. The contact position does not, and is
/// in the box's frame; nothing downstream of here reads it, and it is gated to
/// the tests that assert the sweep found the right point.
///
/// Shared by [`resolve_contact`] and [`crate::thrown::plough`] rather than
/// written twice, for the reason [`Aabb::nearest_face`] was extracted: two
/// copies of one piece of geometry are two things to keep in step, and the copy
/// that is not looked at is the one that rots.
pub fn swept_contact_between(
    mover_start: Vec2,
    mover_end: Vec2,
    radius: f32,
    target_start: Vec2,
    target_end: Vec2,
    target_half_extents: Vec2,
) -> Option<Contact> {
    let relative_motion = (mover_end - mover_start) - (target_end - target_start);
    swept_contact(
        mover_start,
        relative_motion,
        radius,
        Aabb::new(target_start, target_half_extents),
    )
}

/// Where a circle of `radius` sweeping from `start` along `motion` first meets
/// `target`, with the exact surface normal there.
///
/// Swept rather than overlap-tested for the reason terrain is: at the speeds the
/// pass-through ceiling describes, the hero crosses an Enemy's whole box inside
/// one Sim Tick, and an overlap test would see nothing to resolve and let it
/// sail through untouched.
///
/// The normal is read off exact circle-against-box geometry rather than from the
/// box grown by the radius, and that difference is the point of this function
/// existing beside the terrain sweep in `world`. Growing a box treats its
/// corners as square, so a horizontal pass that just clips an Enemy's top corner
/// reports a vertical face's *side* normal and the hero's whole momentum counts
/// as normal momentum. Terrain can afford that approximation; combat cannot,
/// because the normal is the gate, and a graze reading as a clean hit is exactly
/// the failure ADR-0011 chose the normal component to avoid.
pub fn swept_contact(start: Vec2, motion: Vec2, radius: f32, target: Aabb) -> Option<Contact> {
    // Already touching when the tick began. Depenetration is nobody's job here
    // (an Enemy is not terrain), but the contact still has to be reported.
    if overlaps(start, radius, target) {
        return Some(Contact {
            #[cfg(any(test, feature = "test-util"))]
            t: 0.0,
            #[cfg(any(test, feature = "test-util"))]
            pos: start,
            normal: surface_normal(start, target),
        });
    }

    let mut first: Option<f32> = None;
    let mut consider = |t: f32| {
        if (0.0..=1.0).contains(&t) && first.is_none_or(|best| t < best) {
            first = Some(t);
        }
    };

    // The Minkowski boundary of a box and a circle is a rounded rectangle: four
    // faces pushed out by the radius, joined by quarter circles at the corners.
    // Faces first, each clipped to the extent of the face it belongs to, so a
    // crossing in the corner region is left to the arcs.
    let (min, max) = (target.min(), target.max());
    for axis in 0..2 {
        let other = 1 - axis;
        if motion[axis].abs() < 1e-12 {
            continue;
        }
        for plane in [min[axis] - radius, max[axis] + radius] {
            let t = (plane - start[axis]) / motion[axis];
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let crossing = start[other] + motion[other] * t;
            if crossing >= min[other] && crossing <= max[other] {
                consider(t);
            }
        }
    }

    // The corner arcs, as whole circles. A root outside the arc's quadrant lies
    // inside a face's slab, where the face candidate above already found an
    // earlier crossing, so taking the smallest root over everything is still the
    // first meeting with the rounded rectangle.
    for corner in [min, max, Vec2::new(min.x, max.y), Vec2::new(max.x, min.y)] {
        if let Some(t) = ray_circle_entry(start, motion, corner, radius) {
            consider(t);
        }
    }

    first.map(|t| {
        let pos = start + motion * t;
        Contact {
            #[cfg(any(test, feature = "test-util"))]
            t,
            #[cfg(any(test, feature = "test-util"))]
            pos,
            normal: surface_normal(pos, target),
        }
    })
}

/// Whether a circle overlaps a box.
fn overlaps(pos: Vec2, radius: f32, target: Aabb) -> bool {
    (pos - pos.clamp(target.min(), target.max())).length_squared() <= radius * radius
}

/// The outward direction from a box to a point, as a unit vector.
///
/// Exact: in a face region it is the face normal, and in a corner region it is
/// the diagonal from the corner, which is what a round body actually touches a
/// square one with.
fn surface_normal(pos: Vec2, target: Aabb) -> Vec2 {
    let away = pos - pos.clamp(target.min(), target.max());
    if away.length_squared() > 1e-18 {
        return away.normalize();
    }
    // The centre is inside the box, so the geometry offers no outward direction.
    // Leave through the nearest face: the shortest way out, and the only choice
    // that does not teleport a body across the level. Shared with `World`'s
    // depenetration, which needs the same face for the same reason and also
    // reads the distance; see [`Aabb::nearest_face`].
    target.nearest_face(pos).1
}

/// The fraction of `motion` at which a ray first enters a circle, if it does.
fn ray_circle_entry(start: Vec2, motion: Vec2, centre: Vec2, radius: f32) -> Option<f32> {
    let a = motion.length_squared();
    if a < 1e-18 {
        return None;
    }
    let offset = start - centre;
    let b = 2.0 * motion.dot(offset);
    let c = offset.length_squared() - radius * radius;
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return None;
    }
    // The nearer root: the entry. The far root is where it would leave.
    Some((-b - discriminant.sqrt()) / (2.0 * a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_PLAYER_MASS, PLAYER_RADIUS};

    /// An Enemy at the origin, which every geometry case below sweeps against.
    fn target() -> Aabb {
        Aabb::new(Vec2::ZERO, ENEMY_HALF_EXTENTS)
    }

    fn pass(start: Vec2, end: Vec2, vel: Vec2) -> HeroPass {
        HeroPass {
            start,
            end,
            vel,
            mass: DEFAULT_PLAYER_MASS,
            radius: PLAYER_RADIUS,
        }
    }

    fn enemy(toughness: f32) -> EnemyTarget {
        EnemyTarget {
            start: Vec2::ZERO,
            end: Vec2::ZERO,
            vel: Vec2::ZERO,
            mass: 1.0,
            toughness,
            spent: false,
        }
    }

    #[test]
    fn a_head_on_pass_reports_the_face_normal() {
        let hit = swept_contact(
            Vec2::new(-3.0, 0.0),
            Vec2::new(6.0, 0.0),
            PLAYER_RADIUS,
            target(),
        )
        .expect("a pass through the Enemy's centre must register");
        // Contact at exactly a radius short of the box's near face.
        assert!((hit.pos.x - -1.0).abs() < 1e-5, "{hit:?}");
        assert!((hit.normal - Vec2::NEG_X).length() < 1e-5, "{hit:?}");
    }

    #[test]
    fn a_corner_graze_reports_a_diagonal_normal_rather_than_a_face() {
        // This is the geometry the whole grazing rule rests on, and the reason
        // this module does not reuse the terrain sweep. Growing the box by the
        // hero's radius makes its corners square, so a horizontal pass that
        // clips an Enemy's top corner looks like a hit on the box's *side* and
        // reports a normal of (-1, 0): the hero's entire momentum then counts
        // as normal momentum, and a graze destroys an Enemy outright.
        //
        // Exact circle-against-box geometry says the hero touches the corner
        // itself. Expected value from the geometry and not from the code: the
        // circle centre is 0.48 above the corner, so it meets it at a
        // horizontal offset of sqrt(0.5^2 - 0.48^2) = 0.14, and the normal is
        // that offset over the radius.
        let dy = 0.98;
        let hit = swept_contact(
            Vec2::new(-3.0, dy),
            Vec2::new(6.0, 0.0),
            PLAYER_RADIUS,
            target(),
        )
        .expect("clipping the corner must still register as contact");
        let expected = Vec2::new(-0.14, 0.48) / PLAYER_RADIUS;
        assert!(
            (hit.normal - expected).length() < 1e-4,
            "expected {expected:?}, got {hit:?}"
        );
        assert!(
            hit.normal.y > hit.normal.x.abs(),
            "a graze across the top must read as mostly vertical, got {hit:?}"
        );
    }

    #[test]
    fn a_pass_clear_of_the_enemy_reports_nothing() {
        // Just outside tangency: box half extent plus hero radius is 1.0.
        assert!(swept_contact(
            Vec2::new(-3.0, 1.001),
            Vec2::new(6.0, 0.0),
            PLAYER_RADIUS,
            target()
        )
        .is_none());
    }

    #[test]
    fn a_tick_long_enough_to_cross_the_whole_enemy_still_registers() {
        // The pass-through ceiling is 150 momentum units, which at the baseline
        // hero's mass is 2.5 units of travel per Sim Tick against a box one unit
        // across. An overlap test sees nothing to resolve at that speed and the
        // Enemy survives being hit by the fastest thing in the game.
        let motion = Vec2::new(PASS_THROUGH_MOMENTUM / DEFAULT_PLAYER_MASS * crate::DT, 0.0);
        let hit = swept_contact(Vec2::new(-1.2, 0.0), motion, PLAYER_RADIUS, target())
            .expect("a tick that steps clean over an Enemy must still register");
        assert!((hit.normal - Vec2::NEG_X).length() < 1e-5, "{hit:?}");
    }

    #[test]
    fn a_hero_already_overlapping_is_in_contact_with_a_finite_normal() {
        // Hostile: a hero at the Enemy's exact centre has no outward direction
        // to read off the geometry, so a naive normalize yields NaN and every
        // number downstream of it is poisoned.
        let hit = swept_contact(Vec2::ZERO, Vec2::ZERO, PLAYER_RADIUS, target())
            .expect("overlapping is contact");
        assert_eq!(hit.t, 0.0);
        assert!(
            (hit.normal.length() - 1.0).abs() < 1e-6,
            "normal must stay a unit vector, got {hit:?}"
        );
    }

    #[test]
    fn a_slow_drift_is_a_kick_that_spends_minimal_toughness_and_pushes_both_apart() {
        let hero = pass(
            Vec2::new(-1.2, 0.0),
            Vec2::new(-0.9, 0.0),
            Vec2::new(1.0, 0.0),
        );
        let out = resolve_contact(&hero, &enemy(100.0));
        assert_eq!(out.band, Some(ImpactBand::Kick));
        assert_eq!(out.toughness, 100.0 - KICK_DAMAGE);
        assert!(!out.destroyed());
        // What "pushes us apart" means with a mass each: the closing speed
        // comes back out as a separating speed. At equal masses, fully elastic
        // and head on, the pair swaps velocities, so the hero stops dead and
        // the Enemy leaves with what the hero brought. Momentum is conserved
        // exactly, which a repel that only moved the hero would not be.
        assert!(out.enemy_vel.x > 0.0, "the Enemy must be pushed: {out:?}");
        assert!(
            out.enemy_vel.x - out.hero_vel.x > 0.0,
            "the bodies must end up separating: {out:?}"
        );
        assert!(
            (out.hero_vel.x + out.enemy_vel.x - 1.0).abs() < 1e-5,
            "a Kick must conserve momentum: {out:?}"
        );
    }

    #[test]
    fn a_kick_against_a_heavy_enemy_sends_the_hero_back_the_way_it_came() {
        let hero = pass(
            Vec2::new(-1.2, 0.0),
            Vec2::new(-0.9, 0.0),
            Vec2::new(1.0, 0.0),
        );
        let mut heavy = enemy(100.0);
        heavy.mass = 1_000.0;
        let out = resolve_contact(&hero, &heavy);
        assert_eq!(out.band, Some(ImpactBand::Kick));
        // An effectively immovable Enemy reflects the hero: a drift into a
        // heavy body is recoverable, which is the whole point of the band.
        assert!(
            (out.hero_vel.x - -1.0).abs() < 0.01,
            "expected a near full reflection, got {out:?}"
        );
    }

    #[test]
    fn a_hit_above_the_kick_threshold_and_below_toughness_spends_momentum_against_the_pool() {
        let hero = pass(
            Vec2::new(-2.0, 0.0),
            Vec2::new(-0.5, 0.0),
            Vec2::new(50.0, 0.0),
        );
        let out = resolve_contact(&hero, &enemy(100.0));
        assert_eq!(out.band, Some(ImpactBand::Slam));
        assert!(!out.destroyed(), "100 Toughness survives a 50 hit: {out:?}");
        assert_eq!(out.toughness, 50.0, "the pool takes the normal momentum");
    }

    #[test]
    fn a_hit_at_or_above_toughness_destroys_the_enemy_outright() {
        let hero = pass(
            Vec2::new(-3.0, 0.0),
            Vec2::new(-0.5, 0.0),
            Vec2::new(100.0, 0.0),
        );
        let out = resolve_contact(&hero, &enemy(100.0));
        assert_eq!(out.band, Some(ImpactBand::Destroy));
        assert!(out.destroyed());
    }

    #[test]
    fn the_drain_is_proportional_to_the_toughness_actually_consumed() {
        // The reason a flat percentage was rejected: destroying a trivial Enemy
        // must not cost what destroying a heavy one does.
        let momentum = Vec2::new(60.0, 0.0);
        let cheap = momentum_drain(momentum, 5.0).length();
        let heavy = momentum_drain(momentum, 20.0).length();
        assert!((cheap - 5.0).abs() < 1e-4, "got {cheap}");
        assert!((heavy - 20.0).abs() < 1e-4, "got {heavy}");
    }

    #[test]
    fn one_impact_never_drains_more_than_half_the_incoming_momentum() {
        let momentum = Vec2::new(30.0, 40.0); // length 50
        let drain = momentum_drain(momentum, 999.0);
        assert!(
            (drain.length() - 25.0).abs() < 1e-4,
            "expected the cap at half of 50, got {}",
            drain.length()
        );
        assert!(
            (drain.normalize() - momentum.normalize()).length() < 1e-6,
            "the drain only shortens the vector, it never turns it: {drain:?}"
        );
    }

    #[test]
    fn at_or_above_the_pass_through_ceiling_the_drain_is_zero() {
        let momentum = Vec2::new(PASS_THROUGH_MOMENTUM, 0.0);
        assert_eq!(momentum_drain(momentum, 100.0), Vec2::ZERO);
        assert_eq!(
            momentum_drain(momentum * 2.0, f32::MAX),
            Vec2::ZERO,
            "nothing is tough enough to slow a hero past the ceiling"
        );
    }

    #[test]
    fn a_grazing_pass_does_not_register_as_a_clean_hit() {
        // The named regression this whole gate exists for. Total momentum here
        // is 100, exactly the Enemy's Toughness, so a gate on magnitude would
        // destroy it outright from a pass that barely clips its corner. The
        // normal component is 28, which chips it instead.
        let hero = pass(
            Vec2::new(-3.0, 0.98),
            Vec2::new(3.0, 0.98),
            Vec2::new(100.0, 0.0),
        );
        let out = resolve_contact(&hero, &enemy(100.0));
        assert_eq!(out.band, Some(ImpactBand::Slam), "{out:?}");
        assert!(!out.destroyed(), "a graze must not one-shot: {out:?}");
        assert!(
            (out.toughness - 72.0).abs() < 0.5,
            "the pool should take only the normal component, got {out:?}"
        );
    }

    #[test]
    fn an_impact_already_spent_does_not_register_again_while_still_in_contact() {
        // One pass costs one hit. A hero crossing an Enemy takes several Sim
        // Ticks to leave its box, and spending the pool once per tick would
        // empty any Toughness in a third of a second.
        let hero = pass(
            Vec2::new(-0.2, 0.0),
            Vec2::new(0.2, 0.0),
            Vec2::new(50.0, 0.0),
        );
        let mut target = enemy(100.0);
        target.spent = true;
        let out = resolve_contact(&hero, &target);
        assert_eq!(out.band, None);
        assert_eq!(out.toughness, 100.0);
        assert_eq!(out.hero_vel, hero.vel);
        assert!(
            out.spent,
            "the allowance must stay spent while contact lasts"
        );
    }

    #[test]
    fn a_separating_tick_leaves_the_next_real_slam_able_to_register() {
        // The flag is the one-hit-per-contact allowance and nothing else, so a
        // tick that registered nothing must not consume it. A hero hanging on a
        // Movable Anchor sits right at the contact boundary, because
        // `REST_LENGTH` equals the hero's radius plus an Enemy's half extent, so
        // the spring's outward half of a cycle reaches this every time; the
        // inward half then has to still be able to land.
        //
        // Reached through a separating tick rather than by setting the flag by
        // hand, because the value a separating tick leaves behind is the thing
        // under test.
        let outward = pass(
            Vec2::new(-0.2, 0.0),
            Vec2::new(-0.3, 0.0),
            Vec2::new(-10.0, 0.0),
        );
        let first = resolve_contact(&outward, &enemy(100.0));
        assert_eq!(first.band, None, "moving away registers nothing: {first:?}");
        assert_eq!(first.toughness, 100.0, "nothing was spent: {first:?}");

        // The very next tick, the same overlap with the hero driven in at 50
        // units per second: a Slam, carried forward exactly as
        // `World::resolve_impacts` carries it.
        let inward = pass(
            Vec2::new(-0.2, 0.0),
            Vec2::new(0.2, 0.0),
            Vec2::new(50.0, 0.0),
        );
        let mut target = enemy(100.0);
        target.spent = first.spent;
        let second = resolve_contact(&inward, &target);
        assert_eq!(second.band, Some(ImpactBand::Slam), "{second:?}");
        assert_eq!(
            second.toughness, 50.0,
            "the pool must take the normal momentum: {second:?}"
        );
        assert!(
            second.hero_vel.x < inward.vel.x,
            "a Slam that registered must cost the hero momentum: {second:?}"
        );
    }

    #[test]
    fn the_sweep_is_relative_so_either_body_moving_closes_the_tunnel() {
        // One geometry, read from both sides, and the answer must not change:
        // sweeping one body's path against the other's end box leaves the
        // second body's motion unswept, so whichever body happens to be
        // standing still becomes a zero-length sweep and a window two radii
        // plus two half extents wide opens up in it.
        //
        // Three units of travel per tick is well past that window, which is
        // about two units across here. The numbers are the geometry's and not
        // the code's: the circle meets the box's near face at a radius short of
        // it, so the relative offset at contact is -1.0 out of a path from -1.5
        // to 1.5, which is a sixth of the way along.
        const EXPECTED_T: f32 = 1.0 / 6.0;
        let moving_circle = swept_contact_between(
            Vec2::new(-1.5, 0.0),
            Vec2::new(1.5, 0.0),
            PLAYER_RADIUS,
            Vec2::ZERO,
            Vec2::ZERO,
            ENEMY_HALF_EXTENTS,
        )
        .expect("a circle crossing a still box must register");
        let moving_box = swept_contact_between(
            Vec2::ZERO,
            Vec2::ZERO,
            PLAYER_RADIUS,
            Vec2::new(1.5, 0.0),
            Vec2::new(-1.5, 0.0),
            ENEMY_HALF_EXTENTS,
        )
        .expect("a box crossing a still circle must register too");
        for hit in [moving_circle, moving_box] {
            assert!((hit.t - EXPECTED_T).abs() < 1e-5, "{hit:?}");
            assert!((hit.normal - Vec2::NEG_X).length() < 1e-5, "{hit:?}");
        }
    }

    #[test]
    fn a_fast_enemy_cannot_cross_a_stationary_hero_untouched() {
        // The measured case: 186 units per second is 3.1 units of travel in one
        // Sim Tick, and the hero is parked, so before the sweep went relative
        // this read `band: None` with the pool untouched at 100.
        //
        // Reachable without scaffolding: tension is force over mass, so an
        // Enemy of mass 4.8 or more can never reach `THROW_TENSION` even at
        // twice the per-Tongue force ceiling, and a Tongue can therefore keep
        // accelerating it through the hero indefinitely.
        let hero = pass(Vec2::new(9.5, 0.0), Vec2::new(9.5, 0.0), Vec2::ZERO);
        let mut target = enemy(100.0);
        target.start = Vec2::new(7.7, 0.0);
        target.end = Vec2::new(10.8, 0.0);
        target.vel = Vec2::new(186.0, 0.0);
        let out = resolve_contact(&hero, &target);
        assert_eq!(
            out.band,
            Some(ImpactBand::Destroy),
            "186 of normal momentum against a pool of 100: {out:?}"
        );
        assert!(out.destroyed(), "{out:?}");
    }

    #[test]
    fn a_hero_leaving_an_enemy_does_not_register_a_second_impact() {
        // Separating is not hitting. Taking the gate as an absolute magnitude
        // would read a hero flying away from a Kick as hitting it again.
        let hero = pass(
            Vec2::new(-0.9, 0.0),
            Vec2::new(-1.1, 0.0),
            Vec2::new(-50.0, 0.0),
        );
        let out = resolve_contact(&hero, &enemy(100.0));
        assert_eq!(out.band, None, "{out:?}");
        assert_eq!(out.toughness, 100.0);
    }

    #[test]
    fn an_enemy_already_moving_away_reads_a_lower_band_than_the_absolute_gate_would() {
        // ADR-0011 writes the gate as `|m * v . n|` with `v` the hero's
        // velocity; this module reads the **relative** closing instead, and the
        // ADR records why that supersedes the absolute form. What the ADR
        // cannot argue in prose is how far the two diverge, so it is asserted
        // here: the band an impact lands in depends on how fast the target is
        // already leaving, and it is not capped at one band of difference.
        //
        // One geometry throughout, so nothing below differs for any reason but
        // the Enemy's own velocity. The normal is `NEG_X` for a head-on pass,
        // and at the hero's mass of 1 the absolute gate would read 10 against
        // every row: above this Enemy's Toughness of 4, so the ADR's band
        // table calls all four of these Destroy.
        const TOUGHNESS: f32 = 4.0;
        let hero = pass(
            Vec2::new(-2.0, 0.0),
            Vec2::new(-0.5, 0.0),
            Vec2::new(10.0, 0.0),
        );
        assert_eq!(hero.mass, 1.0, "the arithmetic below assumes a unit mass");
        assert!(
            hero.vel.x >= TOUGHNESS,
            "the absolute gate must read Destroy for every row, or nothing diverges"
        );

        // Expected bands worked out from the relative closing by hand, not from
        // the code: closing is `hero.vel.x - fleeing`, and the thresholds are
        // KICK_THRESHOLD (2) and TOUGHNESS (4).
        let rows = [
            (0.0f32, Some(ImpactBand::Destroy)), // closing 10: the ADR agrees
            (7.0, Some(ImpactBand::Slam)),       // closing 3: one band lower
            (8.5, Some(ImpactBand::Kick)),       // closing 1.5: two bands lower
            (11.0, None),                        // closing -1: no impact at all
        ];
        for (fleeing, expected) in rows {
            let mut target = enemy(TOUGHNESS);
            target.vel = Vec2::new(fleeing, 0.0);
            let out = resolve_contact(&hero, &target);
            assert_eq!(
                out.band, expected,
                "an Enemy leaving at {fleeing} against a hero at {} read {:?}",
                hero.vel.x, out.band
            );
        }
        // The guard that keeps the rows above meaningful: the thresholds really
        // are the numbers the comments assume, so a retune cannot leave this
        // test quietly asserting the wrong boundaries.
        assert_eq!(KICK_THRESHOLD, 2.0, "retune the rows above with the gate");
    }

    #[test]
    fn a_stationary_hero_inside_an_enemy_resolves_to_nothing_rather_than_a_nan() {
        let hero = pass(Vec2::ZERO, Vec2::ZERO, Vec2::ZERO);
        let out = resolve_contact(&hero, &enemy(100.0));
        assert_eq!(out.band, None);
        assert!(
            out.hero_vel.is_finite() && out.enemy_vel.is_finite(),
            "{out:?}"
        );
        assert!(out.toughness.is_finite());
    }

    /// The same three bands, asserted through the Core Sim's one seam.
    ///
    /// These are the tests that matter: they construct a `Level`, advance Sim
    /// Ticks, and read `Snapshot`s, so nothing here depends on how combat is
    /// arranged internally. The hero's momentum is assigned rather than earned
    /// by Swinging, which is what lets combat land in parallel with the Tongue.
    mod through_the_world_seam {
        use super::*;
        use crate::frame::Body;
        use crate::level::{EnemySpawn, Level};
        use crate::world::World;
        use crate::{EntityId, DT};

        /// An empty arena: no terrain at all, so a pass is a pass and nothing
        /// lands on anything. Both bodies fall at the same rate under gravity,
        /// so their relative geometry is untouched over a short flight.
        fn arena(enemy_spawns: Vec<EnemySpawn>) -> Level {
            let level = Level {
                world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(500.0)),
                static_terrain: vec![],
                player_spawn: Vec2::new(-20.0, 0.0),
                enemy_spawns,
            };
            level.validate().expect("fixture must be a valid level");
            level
        }

        fn spawn(pos: Vec2, toughness: f32, mass: f32) -> EnemySpawn {
            EnemySpawn {
                pos,
                toughness,
                mass,
            }
        }

        /// Every Enemy still in the simulation, in snapshot order.
        fn enemies(world: &World) -> Vec<Body> {
            world
                .snapshot()
                .bodies
                .iter()
                .copied()
                .filter(|b| b.id != world.hero_id())
                .collect()
        }

        /// A body's velocity, read through the seam rather than out of the
        /// `World`: the two retained snapshots are a tick apart, so the
        /// difference over `DT` is the velocity that carried it.
        ///
        /// Exact along x for every case here, because nothing in an empty arena
        /// accelerates a body horizontally. Along y it runs half a tick of
        /// gravity fast, which is why nothing below asserts on y.
        fn velocity_x(world: &World, id: EntityId) -> f32 {
            let now = world.snapshot().body(id).expect("body must exist").pos;
            let before = world
                .previous_snapshot()
                .body(id)
                .expect("body must exist a tick ago")
                .pos;
            (now.x - before.x) / DT
        }

        fn launch(world: &mut World, from: Vec2, vel: Vec2) {
            let hero = world.hero_id();
            world.place(hero, from, vel);
        }

        #[test]
        fn enemies_spawn_with_the_toughness_the_level_declared() {
            // One hero speed, two Enemies, two outcomes: the frail one is
            // destroyed and the heavy one is not. If `toughness` were a
            // constant rather than per-Enemy level data, both would share a
            // fate whichever way the constant fell.
            let frail = Vec2::new(0.0, 0.0);
            let tough = Vec2::new(8.0, 0.0);
            let level = arena(vec![spawn(frail, 20.0, 1.0), spawn(tough, 300.0, 1.0)]);
            let mut world = World::from_level(&level);
            launch(&mut world, Vec2::new(-3.0, 0.0), Vec2::new(60.0, 0.0));

            for _ in 0..30 {
                world.step();
            }

            let left = enemies(&world);
            assert_eq!(left.len(), 1, "exactly one Enemy should have survived");
            assert!(
                left[0].pos.x > 4.0,
                "the survivor must be the tough one, found {left:?}"
            );
        }

        #[test]
        fn a_slow_drift_kicks_the_hero_back_and_the_enemy_survives() {
            // Story 20: drifting into something is a recoverable mistake and
            // not a death. One unit per second of closing speed at the baseline
            // hero's mass is under the Kick threshold.
            let level = arena(vec![spawn(Vec2::ZERO, 100.0, 50.0)]);
            let mut world = World::from_level(&level);
            launch(&mut world, Vec2::new(-1.2, 0.0), Vec2::new(1.0, 0.0));

            // A drift this slow takes a dozen Sim Ticks to close the last fifth
            // of a unit, so the window has to outlast the approach as well as
            // the impact. Gravity acts on both bodies equally in an arena with
            // no terrain, so it cancels out of the closing speed entirely.
            for _ in 0..40 {
                world.step();
            }

            assert_eq!(enemies(&world).len(), 1, "a Kick must not destroy");
            assert!(
                velocity_x(&world, world.hero_id()) < 0.0,
                "the hero must be repelled back the way it came"
            );
        }

        #[test]
        fn a_slam_below_toughness_chips_the_enemy_over_several_passes() {
            // Story 24: a weak hero is never locked out of a heavy Enemy. At 40
            // momentum against 100 Toughness the pool takes 40 a pass, so the
            // third pass is the one that lands at or above what is left.
            let level = arena(vec![spawn(Vec2::ZERO, 100.0, 1.0)]);
            let mut world = World::from_level(&level);
            let target = enemies(&world)[0].id;

            for pass in 1..=3 {
                // Both bodies are reset so each pass is the same pass: the
                // Enemy carries away the momentum the hero loses, so left
                // alone it would drift out of reach.
                world.place(target, Vec2::ZERO, Vec2::ZERO);
                launch(&mut world, Vec2::new(-3.0, 0.0), Vec2::new(40.0, 0.0));
                for _ in 0..12 {
                    world.step();
                }
                if pass < 3 {
                    assert_eq!(
                        enemies(&world).len(),
                        1,
                        "pass {pass} of 3 must only chip the pool"
                    );
                }
            }

            assert!(
                enemies(&world).is_empty(),
                "three passes of 40 must empty a pool of 100"
            );
        }

        #[test]
        fn a_slam_at_or_above_toughness_destroys_the_enemy_in_one_pass() {
            // Story 23: building speed is rewarded with a one-shot.
            let level = arena(vec![spawn(Vec2::ZERO, 100.0, 1.0)]);
            let mut world = World::from_level(&level);
            launch(&mut world, Vec2::new(-3.0, 0.0), Vec2::new(100.0, 0.0));

            for _ in 0..6 {
                world.step();
            }
            assert!(enemies(&world).is_empty(), "100 against 100 is a one-shot");
        }

        #[test]
        fn one_impact_never_drains_more_than_half_the_heros_momentum() {
            // Story 26: one unlucky hit cannot end a run of Slams. Spending 100
            // Toughness out of 100 momentum would leave the hero stationary
            // without the cap.
            let level = arena(vec![spawn(Vec2::ZERO, 100.0, 1.0)]);
            let mut world = World::from_level(&level);
            launch(&mut world, Vec2::new(-3.0, 0.0), Vec2::new(100.0, 0.0));

            for _ in 0..6 {
                world.step();
            }
            let speed = velocity_x(&world, world.hero_id());
            assert!(
                (speed - 50.0).abs() < 1e-2,
                "expected half of 100 to survive the impact, measured {speed}"
            );
        }

        #[test]
        fn above_the_momentum_ceiling_the_hero_passes_straight_through() {
            // Story 27: reaching top speed is becoming unstoppable. The Enemy
            // still dies; the hero does not notice.
            let level = arena(vec![spawn(Vec2::ZERO, 100.0, 1.0)]);
            let mut world = World::from_level(&level);
            let speed = PASS_THROUGH_MOMENTUM / crate::DEFAULT_PLAYER_MASS + 50.0;
            launch(&mut world, Vec2::new(-3.0, 0.0), Vec2::new(speed, 0.0));

            for _ in 0..6 {
                world.step();
            }
            assert!(enemies(&world).is_empty(), "the Enemy should be gone");
            let measured = velocity_x(&world, world.hero_id());
            assert!(
                (measured - speed).abs() < 1e-2,
                "expected {speed} to be preserved exactly, measured {measured}"
            );
        }

        #[test]
        fn a_fast_enemy_cannot_tunnel_through_a_parked_hero() {
            // The same defect as `a_fast_enemy_cannot_cross_a_stationary_hero_untouched`,
            // asserted through the seam instead: the hero is parked, which used
            // to make its sweep zero length and open a window about two units
            // wide, and the Enemy crosses 3.1 units in this one Sim Tick.
            //
            // The Enemy is driven directly rather than Tongued because the
            // sweep sees the momentum and not the route to it. The route does
            // exist: tension is force over mass, so an Enemy of mass 4.8 or
            // more can never reach `THROW_TENSION` even at twice the per-Tongue
            // force ceiling, and a Tongue can keep accelerating it forever.
            let level = arena(vec![spawn(Vec2::new(-3.0, 0.0), 100.0, 1.0)]);
            let mut world = World::from_level(&level);
            let target = enemies(&world)[0].id;
            world.place(world.hero_id(), Vec2::ZERO, Vec2::ZERO);
            world.place(target, Vec2::new(-3.0, 0.0), Vec2::new(186.0, 0.0));

            world.step();

            assert!(
                enemies(&world).is_empty(),
                "186 of normal momentum against a pool of 100 must destroy, found {:?}",
                enemies(&world)
            );
        }

        #[test]
        fn resting_against_an_enemy_does_not_grind_its_toughness_away() {
            // A long-running scene rather than an impact: the hero drops onto
            // an Enemy standing on the floor and then stays there. Spending
            // Toughness per Sim Tick of contact rather than per impact would
            // destroy anything the hero merely stood on, and nothing in the
            // band rules would look wrong while it happened.
            let floor = Aabb::new(Vec2::new(0.0, -0.5), Vec2::new(10.0, 0.5));
            let level = Level {
                world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(100.0)),
                static_terrain: vec![floor],
                player_spawn: Vec2::new(0.0, 4.0),
                enemy_spawns: vec![spawn(Vec2::new(0.0, PLAYER_RADIUS), 100.0, 1.0)],
            };
            level.validate().expect("fixture must be a valid level");

            let mut world = World::from_level(&level);
            for _ in 0..900 {
                world.step();
            }
            assert_eq!(
                enemies(&world).len(),
                1,
                "fifteen seconds of contact must not destroy an Enemy"
            );
        }

        #[test]
        fn a_grazing_pass_does_not_register_as_a_clean_hit() {
            // The reason the gate is the normal component and not the total
            // magnitude. This hero carries exactly the Enemy's Toughness and
            // clips the top of its box: on magnitude it is a one-shot, on the
            // normal component it is a chip. Without this the targeting arc's
            // tuning would fight combat's.
            let level = arena(vec![spawn(Vec2::ZERO, 100.0, 1.0)]);
            let mut world = World::from_level(&level);
            let grazing_y = ENEMY_HALF_EXTENTS.y + PLAYER_RADIUS - 0.02;
            launch(
                &mut world,
                Vec2::new(-3.0, grazing_y),
                Vec2::new(100.0, 0.0),
            );

            for _ in 0..6 {
                world.step();
            }
            assert_eq!(
                enemies(&world).len(),
                1,
                "a graze carrying a one-shot's momentum must not destroy"
            );
            let kept = velocity_x(&world, world.hero_id());
            assert!(
                kept > 60.0,
                "a graze must cost only the normal component, measured {kept}"
            );
        }
    }
}
