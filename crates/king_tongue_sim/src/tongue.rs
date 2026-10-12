//! The Tongue: what it is Attached to, where a tap puts it, and how it pulls.
//!
//! ADR-0003 makes the Tongue a one-way damped spring rather than a rope, and
//! ADR-0012 makes targeting a pure ray cast with the aim never bent. The force
//! law itself is [`crate::tongue_force`], measured in the ticket #8 prototype
//! against the analytic solution to within 1e-6; this module decides what to
//! feed it and never restates it.

use crate::level::Aabb;
use crate::{Anchor, DEFAULT_DAMPING, DEFAULT_FORCE_CEILING, MAX_TONGUES};
use glam::Vec2;

/// One Attached Tongue.
///
/// There is no in-flight state and no length limit. ADR-0012: retraction is
/// instant, range is unlimited, and a fired Tongue always connects, so there is
/// no whiff to model. The slot a Tongue occupies is its index in the hero's
/// slot array rather than a field, so the two can never disagree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tongue {
    pub anchor: Anchor,
}

/// The hero's Tongue slots. At most two, per ADR-0006.
pub type Tongues = [Option<Tongue>; MAX_TONGUES];

/// One Tongue's pull on a body: where it drags toward, how fast that point is
/// itself moving, and how hard.
///
/// `strength` travels with the point rather than being read off the body being
/// pulled, because Hooke's `k` is a property of the Tongue and so of the hero
/// who fired it. A Tongue on a Movable Anchor pulls the Enemy by mass ratio, so
/// the Enemy feels the hero's `k` and has none of its own.
///
/// `vel` is the Anchor's own velocity, zero for a Fixed Anchor and the Enemy's
/// velocity for a Movable one. It exists so the damping term can be taken
/// against the **relative** radial velocity, which ADR-0011 names directly and
/// Newton's third law requires: damping against each body's absolute velocity
/// would give the two ends of one Tongue forces of different magnitude, and the
/// pair would gain or lose momentum out of nothing. For a Fixed Anchor the two
/// readings are the same number, because terrain does not move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pull {
    pub point: Vec2,
    pub vel: Vec2,
    pub strength: f32,
}

/// Every Tongue pulling on one body this Sim Tick.
///
/// A fixed-capacity array and not a `Vec`, because this is built once per body
/// per Sim Tick and the frame contract refuses per-tick allocation elsewhere for
/// the same reason. `Copy`, so it can be handed to a force evaluation without
/// borrowing the `World` that produced it, which is part of what keeps that
/// evaluation unable to touch anything.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pulls {
    pulls: [Option<Pull>; MAX_TONGUES],
    count: usize,
}

impl Pulls {
    /// Adds a pull toward a point that does not move. Terrain, in other words.
    pub fn push(&mut self, point: Vec2, strength: f32) {
        self.push_moving(point, Vec2::ZERO, strength);
    }

    /// Adds a pull toward a point that is itself a moving body.
    pub fn push_moving(&mut self, point: Vec2, anchor_vel: Vec2, strength: f32) {
        if self.count < MAX_TONGUES {
            self.pulls[self.count] = Some(Pull {
                point,
                vel: anchor_vel,
                strength,
            });
            self.count += 1;
        }
    }

    /// The total force these Tongues apply to a body at `pos` moving at `vel`.
    ///
    /// Two Tongues sum as vectors, which ADR-0003 settled: aligned pulls double,
    /// opposed pulls cancel, and no configuration is infeasible. There is no
    /// constraint solver and no iteration count here, and the exact ratios are
    /// ticket #14's to assert.
    pub fn force(&self, pos: Vec2, vel: Vec2) -> Vec2 {
        self.pulls
            .iter()
            .flatten()
            .map(|p| pull(pos, vel - p.vel, p.point, p.strength))
            .sum()
    }
}

/// The force one Attached Tongue applies to a body at `pos` closing on its
/// Anchor at `relative_vel`.
///
/// A thin wrapper that supplies the damping and ceiling the #8 prototype
/// measured, so no caller invents its own. Pure, which ADR-0007 makes a hard
/// requirement rather than a preference: Velocity Verlet evaluates acceleration
/// twice per Sim Tick, so anything with a side effect here would be applied
/// twice and the simulation would be quietly wrong.
///
/// `relative_vel` is the body's velocity **minus the Anchor's**. For a Fixed
/// Anchor that is the body's own velocity, which is why every measurement taken
/// against terrain still means what it did. For a Movable Anchor it is what
/// makes the two ends of one Tongue feel forces that are exact negations of
/// each other.
pub fn pull(pos: Vec2, relative_vel: Vec2, anchor_point: Vec2, pull_strength: f32) -> Vec2 {
    crate::tongue_force(
        pos,
        relative_vel,
        anchor_point,
        pull_strength,
        DEFAULT_DAMPING,
        DEFAULT_FORCE_CEILING,
    )
}

/// Where a tap from `origin` along `direction` Attaches, or `None` if the ray
/// leaves the level without meeting anything.
///
/// A literal ray cast, which ADR-0012 records as a decision rather than an
/// implementation detail: the aim is never bent, adjusted, or snapped toward a
/// better surface, because every surface is anchorable and bending the aim away
/// from the one the player pointed at is always the worse answer. A reader
/// looking at thumb-sized touch targets will want to add a tolerance cone back;
/// the ADR explains why the earlier 30 degree cone was removed.
///
/// The ray is a point and not a swept circle, so the Anchor lands exactly on the
/// surface rather than a radius short of it. Range is unlimited: distance costs
/// time, never reach.
pub fn cast(origin: Vec2, direction: Vec2, terrain: &[Aabb]) -> Option<Vec2> {
    let dir = direction.normalize_or_zero();
    if dir == Vec2::ZERO {
        // A tap on the hero itself has no direction to fire along. Discarded
        // rather than normalized, which would be a NaN fired into the level.
        return None;
    }

    let mut nearest = f32::INFINITY;
    for aabb in terrain {
        if let Some(t) = hit(origin, dir, *aabb) {
            if t < nearest {
                nearest = t;
            }
        }
    }

    nearest.is_finite().then(|| origin + dir * nearest)
}

/// How far along `dir` the ray first meets `aabb`, if it does.
///
/// Slab test. Unlike the swept-circle version in `world`, the box is not grown:
/// the Tongue has no thickness, so there is no Minkowski sum to take and no
/// corner approximation to apologise for.
fn hit(origin: Vec2, dir: Vec2, aabb: Aabb) -> Option<f32> {
    let (min, max) = (aabb.min(), aabb.max());
    let mut t_enter = f32::NEG_INFINITY;
    let mut t_exit = f32::INFINITY;

    for axis in 0..2 {
        let d = dir[axis];
        let o = origin[axis];
        if d.abs() < 1e-12 {
            // Parallel to this pair of faces: either between them for the whole
            // ray or never meeting the box at all.
            if o < min[axis] || o > max[axis] {
                return None;
            }
            continue;
        }
        let mut near = (min[axis] - o) / d;
        let mut far = (max[axis] - o) / d;
        if near > far {
            core::mem::swap(&mut near, &mut far);
        }
        t_enter = t_enter.max(near);
        t_exit = t_exit.min(far);
    }

    if t_enter > t_exit || t_exit < 0.0 {
        return None;
    }
    // A negative entry time means the origin is already inside the box, which a
    // hero spawned in terrain can be. Attaching at its own position is inert,
    // because a Tongue shorter than its rest boundary is slack, and that is a
    // better answer than skipping this box and Attaching to something across
    // the level in the same direction.
    Some(t_enter.max(0.0))
}

// An `anchor_id` function used to live here, mapping an `Anchor` to an
// `Option<EntityId>` for the frame contract to compare. It is gone, and
// deliberately: it threw away exactly the information interpolation needed,
// collapsing every Fixed Anchor to `None` so that two different terrain points
// compared equal. A [`crate::frame::TongueLine`] now carries the whole
// [`Anchor`], which already compares correctly in both cases.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::REST_LENGTH;

    /// A one-unit tile at integer cell coordinates, matching how
    /// `Level::from_ascii` lays terrain out.
    fn tile(x: f32, y: f32) -> Aabb {
        Aabb::new(Vec2::new(x + 0.5, y + 0.5), Vec2::splat(0.5))
    }

    #[test]
    fn the_cast_lands_on_the_surface_it_points_at() {
        let terrain = [tile(0.0, 5.0)];
        let hit = cast(Vec2::new(0.5, 0.0), Vec2::Y, &terrain).expect("the ray must meet the tile");
        assert_eq!(hit, Vec2::new(0.5, 5.0), "the Anchor sits on the face");
    }

    #[test]
    fn the_nearest_surface_wins() {
        let terrain = [tile(0.0, 9.0), tile(0.0, 3.0), tile(0.0, 20.0)];
        let hit = cast(Vec2::new(0.5, 0.0), Vec2::Y, &terrain).expect("three tiles are in the way");
        assert_eq!(hit.y, 3.0, "Attached past a nearer surface: {hit:?}");
    }

    #[test]
    fn a_ray_into_open_sky_meets_nothing() {
        let terrain = [tile(0.0, 5.0)];
        assert_eq!(cast(Vec2::new(0.5, 0.0), -Vec2::Y, &terrain), None);
    }

    #[test]
    fn a_tap_with_no_direction_is_discarded_rather_than_normalized() {
        // Hostile on purpose. A tap landing exactly on the hero gives a zero
        // vector, and normalizing that yields NaN, which would be fired into
        // the level as an Anchor and poison every later tick.
        let terrain = [tile(0.0, 5.0)];
        assert_eq!(cast(Vec2::new(0.5, 0.0), Vec2::ZERO, &terrain), None);
        let hit = cast(
            Vec2::new(0.5, 0.0),
            Vec2::new(f32::MIN_POSITIVE, 0.0),
            &terrain,
        );
        assert!(
            hit.is_none_or(|p| p.is_finite()),
            "a denormal aim must not produce a non-finite Anchor: {hit:?}"
        );
    }

    #[test]
    fn an_origin_inside_terrain_attaches_to_itself_rather_than_across_the_level() {
        // A hero authored inside a block. The near slab time is negative, and
        // treating that as a miss would skip this box and Attach to the next
        // one along, which is a Tongue fired through a wall.
        let terrain = [tile(0.0, 0.0), tile(0.0, 40.0)];
        let origin = Vec2::new(0.5, 0.5);
        let hit = cast(origin, Vec2::Y, &terrain).expect("the ray starts inside a tile");
        assert_eq!(hit, origin);
        assert!(
            pull(origin, Vec2::ZERO, hit, 40.0) == Vec2::ZERO,
            "a zero-length Tongue must be slack"
        );
    }

    #[test]
    fn the_aim_is_never_bent_toward_a_nearer_surface() {
        // ADR-0012. A big slab sits just off the tapped direction, far nearer
        // than the surface actually pointed at. Any tolerance cone would prefer
        // it; a ray cast must not.
        let terrain = [
            Aabb::new(Vec2::new(6.0, 1.0), Vec2::new(4.0, 0.5)),
            tile(0.0, 30.0),
        ];
        let hit = cast(Vec2::new(0.5, 0.0), Vec2::Y, &terrain).expect("straight up meets the tile");
        assert_eq!(hit, Vec2::new(0.5, 30.0), "the aim was bent: {hit:?}");
    }

    #[test]
    fn a_body_never_feels_more_than_two_tongues() {
        // ADR-0006 caps the hero at two. A third pull reaching the force sum
        // would mean the slot cap had been bypassed somewhere upstream, and the
        // hero would be pulled harder than any Gesture could ask for.
        let mut pulls = Pulls::default();
        for _ in 0..5 {
            pulls.push(Vec2::new(0.0, 10.0), 40.0);
        }
        let one = pull(Vec2::ZERO, Vec2::ZERO, Vec2::new(0.0, 10.0), 40.0);
        let total = pulls.force(Vec2::ZERO, Vec2::ZERO);
        assert!(
            (total.length() - one.length() * MAX_TONGUES as f32).abs() < 1e-3,
            "expected at most {MAX_TONGUES} pulls, got {}x",
            total.length() / one.length()
        );
    }

    #[test]
    fn two_pulls_sum_to_the_prototypes_exact_force_figures() {
        // The ticket #14 numbers, and they are exact rather than approximate.
        // Each was measured in the #8 prototype at **zero velocity**, which is
        // what makes them safe to pin: the damping term is `damping *
        // radial_vel`, so at rest it is identically zero and nothing is left but
        // vector algebra over Hooke's law. At `k = 40` with the Anchor 10 units
        // away the Stretch is `10 - REST_LENGTH = 9`, so one Tongue pulls
        // `40 * 9 = 360` and the rest follows from geometry:
        //
        //     one Tongue                  |F| = 360.000   1.000x
        //     two on the same Anchor      |F| = 720.000   2.000x
        //     two exactly opposed         |F| =   0.000   cancels
        //     two perpendicular           |F| = 509.117   1.414x
        //
        // The contrast worth knowing is with the same prototype's slingshot
        // return speeds, which ticket #13 found are *not* reproducible at the
        // shipped `DEFAULT_DAMPING` (ADR-0003). Force summation at rest does not
        // depend on the damping constant at all, which is exactly why these
        // survive and those did not.
        const K: f32 = 40.0;
        const UP: Vec2 = Vec2::new(0.0, 10.0);
        const DOWN: Vec2 = Vec2::new(0.0, -10.0);
        const RIGHT: Vec2 = Vec2::new(10.0, 0.0);
        // Three decimals, which is the precision the prototype's figures are
        // quoted to.
        const TOLERANCE: f32 = 1e-3;

        let total = |anchors: &[Vec2]| {
            let mut pulls = Pulls::default();
            for anchor in anchors {
                pulls.push(*anchor, K);
            }
            pulls.force(Vec2::ZERO, Vec2::ZERO)
        };

        let one = total(&[UP]);
        assert!(
            (one.length() - 360.0).abs() < TOLERANCE,
            "one Tongue 10 units out at k {K} must pull 360.000, got {}",
            one.length()
        );

        let aligned = total(&[UP, UP]);
        assert!(
            (aligned.length() - 720.0).abs() < TOLERANCE,
            "two Tongues on one Anchor must pull 720.000, got {}",
            aligned.length()
        );
        assert!(
            (aligned - one * 2.0).length() < TOLERANCE,
            "aligned pulls must double in direction too: {aligned:?} against {one:?}"
        );

        let opposed = total(&[UP, DOWN]);
        assert!(
            opposed.length() < TOLERANCE,
            "two exactly opposed Tongues must cancel to 0.000, got {opposed:?}"
        );

        let perpendicular = total(&[UP, RIGHT]);
        let expected = 360.0 * core::f32::consts::SQRT_2;
        assert!(
            (perpendicular.length() - expected).abs() < TOLERANCE,
            "two perpendicular Tongues must pull {expected:.3} (509.117), got {}",
            perpendicular.length()
        );
        // And along the diagonal between them, which the magnitude alone does
        // not pin down: a sum that dropped one component would still be 1.414x
        // if it scaled the other.
        assert!(
            (perpendicular.normalize() - Vec2::new(1.0, 1.0).normalize()).length() < TOLERANCE,
            "the resultant must bisect the two Anchors, got {perpendicular:?}"
        );
    }

    #[test]
    fn damping_is_taken_against_the_anchor_and_never_against_the_world() {
        // Two bodies drifting together at the same speed are not closing on
        // each other at all, so the Tongue is being carried rather than
        // stretched and the damping term must be identically zero. A Tongue
        // that damped against the body's absolute velocity would brake a hero
        // and an Enemy flying in formation, which is a force out of nowhere.
        const ANCHOR: Vec2 = Vec2::new(0.0, 10.0);
        const DRIFT: Vec2 = Vec2::new(0.0, 25.0);

        let mut carried = Pulls::default();
        carried.push_moving(ANCHOR, DRIFT, 40.0);
        let mut terrain = Pulls::default();
        terrain.push(ANCHOR, 40.0);

        let still = terrain.force(Vec2::ZERO, Vec2::ZERO);
        assert_eq!(
            carried.force(Vec2::ZERO, DRIFT),
            still,
            "a Tongue whose Anchor moves with the body must damp nothing"
        );
        // Vacuity guard: the same body velocity against *terrain* does damp, so
        // the equality above is a property of the Anchor's velocity and not of
        // a damping term that is zero for every input.
        assert_ne!(
            terrain.force(Vec2::ZERO, DRIFT),
            still,
            "the fixture's drift is too small to damp, so nothing above was tested"
        );
    }

    #[test]
    fn the_two_ends_of_one_tongue_feel_exactly_opposite_forces() {
        // Newton's third law at the level of the force law itself, which is
        // what makes the mass-ratio pull in ADR-0003 well defined. Asserted as
        // an exact sum rather than a tolerance: both ends are computed from one
        // Stretch, one direction and one relative radial velocity, so the only
        // difference between them is a sign and float arithmetic has no room to
        // introduce another.
        let (hero_pos, hero_vel) = (Vec2::new(1.0, 2.0), Vec2::new(-3.0, 7.0));
        let (enemy_pos, enemy_vel) = (Vec2::new(9.0, -4.0), Vec2::new(2.0, 1.0));
        let on_hero = pull(hero_pos, hero_vel - enemy_vel, enemy_pos, 40.0);
        let on_enemy = pull(enemy_pos, enemy_vel - hero_vel, hero_pos, 40.0);

        assert_eq!(
            on_hero + on_enemy,
            Vec2::ZERO,
            "the pair gains {:?} out of nothing",
            on_hero + on_enemy
        );
        // Vacuity guard: a Tongue that was slack, or whose force had been
        // clamped to zero, would sum to zero trivially.
        assert!(
            on_hero.length() > 1.0,
            "the fixture's Tongue pulls almost nothing: {on_hero:?}"
        );
    }

    #[test]
    fn pull_uses_the_prototypes_measured_defaults() {
        // The wrapper must not invent its own damping or ceiling. Checked
        // against the free function with the constants passed explicitly, so a
        // drift between the two shows up here rather than as a feel complaint.
        let (pos, vel, anchor) = (Vec2::ZERO, Vec2::new(1.0, 2.0), Vec2::new(0.0, 8.0));
        assert_eq!(
            pull(pos, vel, anchor, 40.0),
            crate::tongue_force(
                pos,
                vel,
                anchor,
                40.0,
                DEFAULT_DAMPING,
                DEFAULT_FORCE_CEILING
            )
        );
        assert!(
            pull(pos, vel, Vec2::new(0.0, REST_LENGTH * 0.5), 40.0) == Vec2::ZERO,
            "a slack Tongue pulls nothing"
        );
    }
}
