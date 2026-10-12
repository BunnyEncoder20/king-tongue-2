//! The Core Sim: all game state and rules, with no engine, renderer, or window.
//!
//! Boundary, per ADR-0001: this crate depends on `glam` and nothing else in its
//! default feature set. The `serde` feature adds derives only. Nothing here may
//! reference a Macroquad, miniquad, or platform type, and that is enforced
//! mechanically by `scripts/check-sim-deps.sh` rather than by review.
//!
//! See also ADR-0002 (fixed 60Hz Sim Tick), ADR-0003 (the Tongue is a one-way
//! damped spring), ADR-0007 (Velocity Verlet).

#![forbid(unsafe_code)]

pub mod combat;
pub mod combo;
pub mod despawn;
/// The determinism harness. Behind `test-util`, like `Level::from_ascii`, so it
/// cannot reach a build that ships.
#[cfg(any(test, feature = "test-util"))]
pub mod determinism;
pub mod frame;
pub mod input;
pub mod level;
pub mod movable_anchor;
pub mod thrown;
pub mod tongue;
pub mod world;

pub use combo::Combo;
pub use frame::{Accumulator, Body, BodyKind, Snapshot, TongueLine, MAX_CATCHUP_TICKS};
pub use input::{GestureTracker, TouchPhase, TouchSample};
pub use level::{Aabb, EnemySpawn, Level, LevelError};
pub use tongue::Tongue;
pub use world::World;

use glam::Vec2;

/// Player hitbox radius. All lengths are world units; treat one unit as one metre.
pub const PLAYER_RADIUS: f32 = 0.5;

/// The baseline hero's mass. A parameter and not a character system: ADR-0001's
/// Core Sim carries `mass` per body, and the hero archetype roster is in the fog.
pub const DEFAULT_PLAYER_MASS: f32 = 1.0;

/// The Tongue's rest boundary, tied to player geometry rather than tuned freely.
/// Below this the Tongue is slack and applies nothing. ADR-0003.
pub const REST_LENGTH: f32 = 2.0 * PLAYER_RADIUS;

/// One Sim Tick. Locked by ADR-0002 and load-bearing for ADR-0007's stability
/// bound, which is `pull_strength < 4 * mass / DT^2`.
pub const DT: f32 = 1.0 / 60.0;

/// Moon-scale, deliberately, so touch-driven flight stays tractable. Earth is -9.81.
pub const GRAVITY: Vec2 = Vec2::new(0.0, -1.62);

/// Measured in the #8 prototype. A gameplay cap, not a stability one: uncapped,
/// a 1000 unit attach peaks above 5000 units per second.
pub const DEFAULT_FORCE_CEILING: f32 = 1200.0;

/// Measured in the #8 prototype. `zeta` 0.237 at the baseline hero, inside the
/// usable 0.1 to 0.25 band. Undamped never settles; critically damped kills the snap.
pub const DEFAULT_DAMPING: f32 = 3.0;

/// The baseline hero's Tongue strength: Hooke's `k`, in force per unit of Stretch.
///
/// A per-hero parameter and not a character system, same as `mass`. ADR-0007
/// measured explicit Euler capping out at 3 where the baseline hero needs this,
/// which is the whole reason the integrator is Velocity Verlet. The stability
/// bound is [`max_stable_pull_strength`], roughly 14,400 at mass 1, so this
/// leaves two orders of magnitude of headroom for device tuning.
pub const DEFAULT_PULL_STRENGTH: f32 = 40.0;

/// At most two Tongues, per ADR-0006. A third concurrent touch id is discarded.
pub const MAX_TONGUES: usize = 2;

/// What a Tongue is attached to.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Anchor {
    /// Terrain. Infinite mass, so only the hero is pulled.
    Fixed(Vec2),
    /// An Enemy or other movable body. Always attaches at its exact centre of mass.
    ///
    /// Carries an [`EntityId`] rather than a bare `u32`. The two were already the
    /// same number, and leaving them as different types invited passing a slice
    /// index where an identity was meant, which is exactly the mistake the frame
    /// contract's id matching exists to prevent.
    Movable(EntityId),
}

// A `GestureIntent` enum used to live here, for the Shell to build and the Core
// Sim to consume. It is gone, and deliberately. Under the `World` seam the Shell
// reports raw [`TouchSample`]s and the Gesture tracker decides what they mean,
// so an intent type in between would have had to be built by the Shell out of
// rules that no longer live there: which slot a tap claims, and whether a
// second tap fires or Releases. Those are exactly the decisions the seam exists
// to keep out of an untested crate.

/// Stable identity for anything the Shell has to draw, so adding a body to the
/// Core Sim cannot silently fail to render. The interpolation contract is #7.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EntityId(pub u32);

/// Returns the one-way damped spring force pulling `pos` toward `anchor`.
///
/// Three zones, per ADR-0003:
/// 1. `len <= REST_LENGTH` gives zero force. A web never pushes.
/// 2. `len > REST_LENGTH` gives Hooke's law on the Stretch past the boundary.
/// 3. Approaching the boundary needs no special case: Hooke's law already tapers
///    force continuously to zero there, so deceleration is smooth for free.
pub fn tongue_force(
    pos: Vec2,
    vel: Vec2,
    anchor: Vec2,
    pull_strength: f32,
    damping: f32,
    force_ceiling: f32,
) -> Vec2 {
    let to_anchor = anchor - pos;
    let len = to_anchor.length();
    if len <= REST_LENGTH || len < 1e-6 {
        return Vec2::ZERO;
    }
    let dir = to_anchor / len;
    let stretch = len - REST_LENGTH;
    let radial_vel = vel.dot(dir);
    let mag = (pull_strength * stretch - damping * radial_vel)
        .max(0.0)
        .min(force_ceiling);
    dir * mag
}

/// The stability ceiling from ADR-0007. Exceeding this at the locked `DT` makes
/// even a symplectic integrator diverge, and reopens ADR-0002 rather than ADR-0007.
pub fn max_stable_pull_strength(mass: f32) -> f32 {
    4.0 * mass / (DT * DT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slack_tongue_applies_nothing() {
        let f = tongue_force(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(0.0, REST_LENGTH * 0.5),
            40.0,
            DEFAULT_DAMPING,
            DEFAULT_FORCE_CEILING,
        );
        assert_eq!(
            f,
            Vec2::ZERO,
            "a Tongue inside the rest boundary must not pull"
        );
    }

    #[test]
    fn tongue_never_pushes() {
        // Racing toward the anchor hard enough that damping would otherwise
        // invert the force and shove the hero away.
        let f = tongue_force(
            Vec2::ZERO,
            Vec2::new(0.0, 500.0),
            Vec2::new(0.0, 1.2),
            40.0,
            DEFAULT_DAMPING,
            DEFAULT_FORCE_CEILING,
        );
        assert!(f.y >= 0.0, "spring must be one-way, got {f:?}");
    }

    #[test]
    fn force_respects_the_ceiling() {
        let f = tongue_force(
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::new(0.0, 1000.0),
            40.0,
            DEFAULT_DAMPING,
            DEFAULT_FORCE_CEILING,
        );
        assert!(
            f.length() <= DEFAULT_FORCE_CEILING + 1e-3,
            "expected cap at {DEFAULT_FORCE_CEILING}, got {}",
            f.length()
        );
    }

    #[test]
    fn hang_equilibrium_matches_theory() {
        // ADR-0003: equilibrium hang length is REST_LENGTH + m*g/k.
        let (mass, k) = (1.0f32, 40.0f32);
        let expected = REST_LENGTH + mass * GRAVITY.length() / k;
        let anchor = Vec2::new(0.0, 10.0);
        let mut pos = Vec2::ZERO;
        let mut vel = Vec2::ZERO;
        for _ in 0..20_000 {
            let a = |p: Vec2, v: Vec2| {
                (GRAVITY * mass
                    + tongue_force(p, v, anchor, k, DEFAULT_DAMPING, DEFAULT_FORCE_CEILING))
                    / mass
            };
            // Velocity Verlet, per ADR-0007.
            let a0 = a(pos, vel);
            pos += vel * DT + 0.5 * a0 * DT * DT;
            let a1 = a(pos, vel);
            vel += 0.5 * (a0 + a1) * DT;
        }
        let measured = (anchor - pos).length();
        assert!(
            (measured - expected).abs() < 1e-3,
            "expected {expected}, measured {measured}"
        );
    }

    #[test]
    fn stability_bound_leaves_headroom_over_the_baseline_hero() {
        // Baseline pull_strength is 40. ADR-0007 measured Velocity Verlet at
        // roughly 93 percent of this bound in practice.
        assert!(max_stable_pull_strength(1.0) > 40.0 * 100.0);
    }

    /// The largest Stretch a free oscillator reaches over `ticks`, integrated
    /// with ADR-0007's Velocity Verlet from a Stretch of one.
    ///
    /// No gravity and no damping, and neither is an omission. The bound
    /// `omega * dt < 2` is a statement about an undamped harmonic oscillator,
    /// so damping would be measuring a different system and reporting this
    /// one's number; the force ceiling is lifted for the same reason, because
    /// a clamped force is not Hooke's law and the clamp alone would keep an
    /// unstable configuration looking tame.
    ///
    /// The small initial velocity is load-bearing, and a failure put it there.
    /// Released from rest exactly along an axis, this one-way spring has exact
    /// resonances: at the bound, and again at exactly twice it, the position
    /// step lands on the mirror image of where it started and the discrete map
    /// is a perfect two-tick cycle, so the amplitude comes back as 1.0000 and
    /// a runaway configuration reads as stable. A nudge off that symmetry is
    /// what makes the measurement about the integrator rather than about an
    /// arithmetic coincidence.
    /// The Stretch [`peak_stretch`] starts from, and the velocity it starts
    /// with. Named so the analytic amplitude can be written from them.
    const START_STRETCH: f32 = 1.0;
    const START_VEL: Vec2 = Vec2::new(-0.37, 0.11);

    fn peak_stretch(pull_strength: f32, mass: f32, ticks: u32) -> f32 {
        let anchor = Vec2::ZERO;
        let mut pos = Vec2::new(REST_LENGTH + START_STRETCH, 0.0);
        let mut vel = START_VEL;
        let mut peak = 0.0f32;
        let a =
            |p: Vec2, v: Vec2| tongue_force(p, v, anchor, pull_strength, 0.0, f32::INFINITY) / mass;
        for _ in 0..ticks {
            let a0 = a(pos, vel);
            pos += vel * DT + 0.5 * a0 * DT * DT;
            let a1 = a(pos, vel);
            vel += 0.5 * (a0 + a1) * DT;
            peak = peak.max(pos.length() - REST_LENGTH);
            if !peak.is_finite() {
                return peak;
            }
        }
        peak
    }

    #[test]
    fn the_stability_bound_is_a_real_cliff_and_not_a_decorative_constant() {
        // What makes every claim about staying far below this bound mean
        // anything. ADR-0007 says a `pull_strength` approaching `4 * mass /
        // DT^2` reopens ADR-0002 rather than ADR-0007, and ticket #20's sweep
        // asserts that nothing in the game comes near it. A sweep against a
        // number nobody had shown to matter would be a sweep that cannot fail,
        // so both sides of the cliff are measured here.
        const TICKS: u32 = 10_000;
        let bound = max_stable_pull_strength(1.0);

        // Far inside it, the oscillator holds the amplitude energy
        // conservation says it should, over nearly three minutes of Sim Ticks.
        //
        // The prediction is analytic rather than "about one". A one-way spring
        // with no damping is conservative, the slack zone included, because
        // force depends on position alone and is zero inside the boundary, so
        //
        //     0.5*k*s_max^2 = 0.5*k*s0^2 + 0.5*m*v0^2
        //
        // and the turning point is `sqrt(s0^2 + m*v0^2/k)`. At a ten-thousandth
        // of the bound that is 1.051 and not 1.0, which is where this test
        // first failed: the starting velocity is a tenth of the spring's energy
        // at that stiffness and all of it is real amplitude. One percent is
        // what the integrator is allowed on top, and the residual is the
        // transverse motion the radial formula leaves out.
        //
        // The baseline hero's own ratio is 0.0028 and two Tongues on one body
        // is 0.0056, so the whole game lives at the left end of this range.
        for fraction in [0.0001, 0.001, 0.01, 0.05] {
            let k = bound * fraction;
            let peak = peak_stretch(k, 1.0, TICKS);
            let predicted = (START_STRETCH * START_STRETCH + START_VEL.length_squared() / k).sqrt();
            assert!(
                (peak - predicted).abs() / predicted < 0.01,
                "at {fraction} of the bound the amplitude reached {peak}, against \
                 the conserved-energy prediction {predicted}"
            );
        }

        // At and above it the same integrator runs away, and not by a little:
        // 33 at nine tenths of the bound, 9,260 at the bound itself, and
        // non-finite past it. No tolerance absorbs that, which is the point.
        // Nine tenths already diverging is ADR-0007's own measurement, which
        // puts Velocity Verlet at roughly 93 percent of the theoretical bound
        // in practice rather than at all of it.
        for fraction in [0.9, 1.0, 1.1, 1.5, 10.0] {
            let peak = peak_stretch(bound * fraction, 1.0, TICKS);
            // Non-finite counts as running away, and is checked separately
            // rather than left to a comparison: a NaN loses every float
            // comparison it is in, so `peak > 10.0` alone would read the worst
            // possible outcome as the test passing.
            assert!(
                !peak.is_finite() || peak > 10.0,
                "at {fraction} of the bound the amplitude should have run away, \
                 reached only {peak}"
            );
        }

        // In between, the amplitude wanders instead of being conserved, and
        // that is worth writing down because the bound alone does not suggest
        // it. Measured peaks from a Stretch of one: 1.23 at an eighth of the
        // bound, 1.85 at three tenths, 6.4 at seven tenths. None of that is
        // divergence and all of it is a Swing that does not come back to where
        // the player left it, so the usable region is a long way below 14,400
        // and the sweep over the scripted runs is strict rather than merely
        // shy of the cliff.
        assert!(peak_stretch(bound * 0.3, 1.0, TICKS) > 1.5);

        // And the dynamics depend on `pull_strength / mass` and nothing else,
        // which is why the bound rises with mass exactly as ADR-0007 records,
        // and why the sweep over the scripted runs divides by the *smallest*
        // mass in the scene. Asserted as an identity rather than as a
        // tolerance: sixteen times the spring on sixteen times the mass is the
        // same motion to the bit.
        for fraction in [0.001, 0.05, 0.3, 1.5] {
            assert_eq!(
                peak_stretch(bound * fraction, 1.0, TICKS).to_bits(),
                peak_stretch(bound * fraction * 16.0, 16.0, TICKS).to_bits(),
                "at {fraction} of the bound, mass must scale the bound exactly"
            );
        }
    }
}
