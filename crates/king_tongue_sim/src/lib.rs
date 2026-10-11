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
/// The determinism harness. Behind `test-util`, like `Level::from_ascii`, so it
/// cannot reach a build that ships.
#[cfg(any(test, feature = "test-util"))]
pub mod determinism;
pub mod frame;
pub mod level;
pub mod world;

pub use frame::{Accumulator, Body, BodyKind, Snapshot, TongueLine, MAX_CATCHUP_TICKS};
pub use level::{Aabb, EnemySpawn, Level, LevelError};
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

/// A Gesture's effect, as the Shell reports it. The Shell produces intents; it
/// never mutates simulation state. ADR-0006: a tap fires, a second tap Releases.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GestureIntent {
    /// Fire a Tongue along this unit vector from the hero.
    Fire { slot: u8, direction: Vec2 },
    /// Release the Tongue in this slot.
    Release { slot: u8 },
}

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
}
