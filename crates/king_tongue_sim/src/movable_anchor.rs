//! Movable Anchors: targeting an Enemy, and what becomes of a slot when the
//! Enemy it held leaves the simulation.
//!
//! GLOSSARY: a Movable Anchor is an Anchor that is itself a body with mass, so
//! Attaching exchanges momentum and both bodies are pulled toward each other.
//! The force law is unchanged ([`crate::tongue_force`]); what this module owns
//! is the two decisions either side of it. Which body a tap picks, per
//! ADR-0012's Enemy Assist Arc, and whether a slot survives its Anchor, per
//! ADR-0014.
//!
//! Everything here is pure. The mass-ratio pull is a force and not an impulse,
//! so ADR-0007's integrator evaluates it twice per Sim Tick, and nothing a
//! force can reach may mutate anything.

use crate::level::Aabb;
use crate::tongue::Tongues;
use crate::{Anchor, EntityId};
use glam::Vec2;

/// Half-angle of the Enemy Assist Arc, in degrees.
///
/// ADR-0012 keeps one narrow arc scoped to Enemies alone, after removing a 30
/// degree tolerance cone that applied to terrain. Narrow is the whole point, so
/// this is deliberately well inside that: an Enemy within 10 degrees of the
/// tapped vector is preferred over the terrain behind it, and anything wider is
/// the cone the ADR rejected coming back by the side door.
///
/// It expresses a preference and nothing else. The ray's direction is never
/// changed, so the Fixed Anchor a tap outside the arc lands on is bit-identical
/// to the one a pure [`crate::tongue::cast`] would have found.
pub const ENEMY_ASSIST_ARC_DEGREES: f32 = 10.0;

/// One Enemy as a tap sees it: an identity and a box.
///
/// Carries the Enemy's centre separately from its box because the two answer
/// different questions. The box decides whether the ray struck it; the centre
/// is where the Tongue Attaches, and ADR-0012 allows no other attachment point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub id: EntityId,
    /// The Enemy's centre of mass.
    pub centre: Vec2,
    pub half_extents: Vec2,
}

/// What a tap from `origin` along `direction` Attaches to, Enemies included.
///
/// Two rules, in this order:
///
/// 1. An Enemy the ray strikes, or one whose centre lies inside the Enemy
///    Assist Arc, is preferred over terrain **behind** it. Nearest wins.
/// 2. Otherwise the answer is exactly [`crate::tongue::cast`]'s: the terrain
///    the ray meets, or nothing.
///
/// An Enemy further along the ray than the terrain hit is not a candidate, so a
/// wall genuinely blocks: assist picks between things the tap could have hit,
/// it does not reach through geometry.
///
/// The arc never alters the direction, per ADR-0012. A preferred Enemy Attaches
/// at its exact centre of mass, which may be off the tapped line; that is the
/// preference being expressed, not the aim being bent, and it is required
/// rather than convenient. A surface attachment would put the spring force off
/// the line between the two centres and so imply a torque, which bodies with no
/// angular dynamics cannot represent.
pub fn cast_anchor<I>(origin: Vec2, direction: Vec2, terrain: &[Aabb], enemies: I) -> Option<Anchor>
where
    I: IntoIterator<Item = Target>,
{
    let dir = direction.normalize_or_zero();
    if dir == Vec2::ZERO {
        // A tap on the hero itself. Discarded rather than normalized, which
        // would fire a NaN into the level. Same rule as `tongue::cast`.
        return None;
    }

    let terrain_hit = crate::tongue::cast(origin, dir, terrain);
    let terrain_distance = terrain_hit.map_or(f32::INFINITY, |p| (p - origin).length());

    let cos_arc = ENEMY_ASSIST_ARC_DEGREES.to_radians().cos();
    let mut best: Option<(f32, EntityId)> = None;
    for enemy in enemies {
        let to_enemy = enemy.centre - origin;
        let along = to_enemy.dot(dir);
        if along <= 0.0 {
            // Behind the tap. The ray is one-directional and an Enemy at the
            // player's back is not a target.
            continue;
        }
        let struck =
            crate::tongue::cast(origin, dir, &[Aabb::new(enemy.centre, enemy.half_extents)])
                .is_some();
        // An Enemy the ray literally strikes is a candidate whatever the arc
        // says. A close, wide Enemy subtends more than the arc, so judging it
        // on its centre alone would let a tap pass straight through the body it
        // was pointed at and Attach to the wall beyond.
        let in_arc = struck || to_enemy.normalize_or_zero().dot(dir) >= cos_arc;
        if !in_arc || along > terrain_distance {
            continue;
        }
        if best.is_none_or(|(d, _)| along < d) {
            best = Some((along, enemy.id));
        }
    }

    match best {
        Some((_, id)) => Some(Anchor::Movable(id)),
        None => terrain_hit.map(Anchor::Fixed),
    }
}

/// Frees every slot whose Movable Anchor has left the simulation, and reports
/// how many it freed.
///
/// ADR-0014: destruction forces a Release. The hero keeps whatever momentum it
/// had, because a Release is only ever the clearing of a slot, and the slot is
/// free on the same tick the Anchor stops existing rather than waiting for a
/// tap the player cannot know is needed.
///
/// `alive` is asked by identity and never by slice position. A body that
/// despawned must answer false, which is exactly what a lookup over the live
/// body list does.
pub fn release_dead_anchors(tongues: &mut Tongues, alive: impl Fn(EntityId) -> bool) -> usize {
    let mut freed = 0;
    for slot in tongues.iter_mut() {
        let dead = matches!(slot.map(|t| t.anchor), Some(Anchor::Movable(id)) if !alive(id));
        if dead {
            *slot = None;
            freed += 1;
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::ENEMY_HALF_EXTENTS;
    use crate::level::{EnemySpawn, Level};
    use crate::world::World;
    use crate::{TouchPhase, TouchSample, DT, REST_LENGTH};

    /// An Enemy of the shipped collision size at `centre`.
    fn target(id: u32, centre: Vec2) -> Target {
        Target {
            id: EntityId(id),
            centre,
            half_extents: ENEMY_HALF_EXTENTS,
        }
    }

    /// Hero at the origin, one Enemy exactly 10 units to the right at the same
    /// height, and a wall face 10 units further along the same line.
    ///
    /// Ten units is the distance the #8 prototype's force figures were measured
    /// at, so the Stretch is `10 - REST_LENGTH = 9` and one Tongue pulls
    /// `DEFAULT_PULL_STRENGTH * 9 = 360`. The hero sits at the origin so a
    /// single tick's displacement is exact rather than a difference of two
    /// large coordinates, and the Tongue is **horizontal** so that gravity
    /// contributes nothing at all to the axis being measured.
    ///
    /// `toughness` is far above anything the hero can spend here, so an impact
    /// can never remove the Enemy and change the body count under a test that
    /// was measuring a force.
    ///
    /// The wall is tall rather than a single tile so that a tap aimed off the
    /// horizontal still meets it, which is what lets a test compare the Enemy
    /// against the terrain genuinely behind it.
    fn duel(enemy_mass: f32) -> Level {
        let level = Level {
            world_bounds: Aabb::new(Vec2::ZERO, Vec2::splat(500.0)),
            static_terrain: vec![Aabb::new(Vec2::new(20.5, 0.0), Vec2::new(0.5, 20.0))],
            player_spawn: Vec2::ZERO,
            enemy_spawns: vec![EnemySpawn {
                pos: Vec2::new(10.0, 0.0),
                toughness: 100_000.0,
                mass: enemy_mass,
            }],
        };
        level.validate().expect("fixture must be a valid level");
        level
    }

    /// The one Enemy in a world, by identity rather than by slice position.
    fn enemy_of(world: &World) -> EntityId {
        let hero = world.hero_id();
        world
            .snapshot()
            .bodies
            .iter()
            .find(|b| b.id != hero)
            .map(|b| b.id)
            .expect("the fixture spawns one Enemy")
    }

    /// Taps once at a world point, as one finger going down.
    fn tap(world: &mut World, id: u64, at: Vec2) {
        world.apply_touches(&[TouchSample {
            id,
            phase: TouchPhase::Started,
            pos: at,
        }]);
    }

    /// Lifts a finger. The end of a Gesture and not a Release: ADR-0006 makes
    /// attachment outlast the finger, so this gives up the slot's *ownership*
    /// and leaves whatever Tongue is in it Attached. A later tap cannot reach
    /// that slot at all until this has happened.
    fn lift(world: &mut World, id: u64) {
        world.apply_touches(&[TouchSample {
            id,
            phase: TouchPhase::Ended,
            pos: Vec2::ZERO,
        }]);
    }

    #[test]
    fn a_tongue_attaches_to_an_enemy_at_its_centre_of_mass_and_not_where_the_ray_struck() {
        // ADR-0012 allows no other attachment point: a surface attachment puts
        // the spring force off the line between the two centres and so implies
        // a torque, which bodies with no angular dynamics cannot represent.
        let mut world = World::from_level(&duel(1.0));
        let enemy = enemy_of(&world);

        // Deliberately off centre. The ray enters the Enemy's left face well
        // below its middle, so an implementation that Attached where the ray
        // struck would land near (9.5, -0.4) and not on the centre at all.
        tap(&mut world, 1, Vec2::new(9.5, -0.4));
        world.step();

        let line = world
            .snapshot()
            .tongues
            .iter()
            .find(|t| t.slot == 0)
            .copied()
            .expect("the tap must Attach something");
        assert_eq!(
            line.anchor,
            Anchor::Movable(enemy),
            "a tap that meets an Enemy must Attach to it by identity"
        );
        // The Enemy has moved by one tick of the pull, so the Anchor is read
        // off the body rather than off the spawn point.
        let centre = world
            .snapshot()
            .body(enemy)
            .expect("the Enemy is still alive")
            .pos;
        assert_eq!(line.to, centre, "the Anchor must be the centre of mass");

        // Vacuity guard. Without the Enemy the very same tap Attaches to the
        // wall behind it, so this test would pass for free if the Anchor were
        // being read off terrain.
        let terrain_only =
            crate::tongue::cast(Vec2::ZERO, Vec2::new(9.5, -0.4), &duel(1.0).static_terrain)
                .expect("the same ray meets the wall behind the Enemy");
        assert!(
            (terrain_only - centre).length() > 1.0,
            "the fixture does not distinguish the Enemy from the terrain behind it"
        );
    }

    #[test]
    fn an_enemy_inside_the_assist_arc_is_preferred_over_the_terrain_behind_it() {
        // ADR-0012's narrow arc, exercised where the ray misses the Enemy's box
        // outright: the preference is what picks it, not a literal hit.
        let wall = [Aabb::new(Vec2::new(20.5, 0.0), Vec2::splat(0.5))];
        let aim = Vec2::new(10.0, 0.0);
        let nudged = target(7, Vec2::new(10.0, 1.0));
        assert!(
            crate::tongue::cast(
                Vec2::ZERO,
                aim,
                &[Aabb::new(nudged.centre, nudged.half_extents)]
            )
            .is_none(),
            "the fixture must place the Enemy off the ray, or the arc is not what is being tested"
        );
        assert_eq!(
            cast_anchor(Vec2::ZERO, aim, &wall, [nudged]),
            Some(Anchor::Movable(EntityId(7))),
            "an Enemy 5.7 degrees off the tapped vector is inside a 10 degree arc"
        );

        // And the arc really is narrow: the same Enemy further off the line is
        // not preferred, and the answer falls back to the terrain untouched.
        let outside = target(7, Vec2::new(10.0, 4.0));
        assert_eq!(
            cast_anchor(Vec2::ZERO, aim, &wall, [outside]),
            Some(Anchor::Fixed(Vec2::new(20.0, 0.0))),
            "an Enemy 21.8 degrees off the tapped vector is outside the arc"
        );
    }

    #[test]
    fn the_arc_never_bends_the_aim() {
        // The AC's wording, and it is checkable rather than rhetorical: with an
        // Enemy outside the arc the Fixed Anchor must be *bit-identical* to the
        // one a pure terrain cast finds. A cone that nudged the ray toward the
        // Enemy would differ in the last few bits and nothing else would catch
        // it.
        let wall = [Aabb::new(Vec2::new(20.5, 3.0), Vec2::new(0.5, 8.0))];
        let aim = Vec2::new(10.0, 2.0);
        let pure = crate::tongue::cast(Vec2::ZERO, aim, &wall).expect("the ray meets the wall");
        let outside = target(7, Vec2::new(4.0, 8.0));

        let with_enemy = cast_anchor(Vec2::ZERO, aim, &wall, [outside]);
        assert_eq!(
            with_enemy,
            Some(Anchor::Fixed(pure)),
            "an Enemy outside the arc changed the Fixed Anchor"
        );
        match with_enemy {
            Some(Anchor::Fixed(p)) => assert!(
                p.x.to_bits() == pure.x.to_bits() && p.y.to_bits() == pure.y.to_bits(),
                "the Anchor differs from the pure cast in its low bits: {p:?} against {pure:?}"
            ),
            other => panic!("expected a Fixed Anchor, got {other:?}"),
        }

        // Vacuity guard: the same Enemy brought inside the arc does change the
        // answer, so the assertion above is not true for every input.
        assert_eq!(
            cast_anchor(Vec2::ZERO, aim, &wall, [target(7, Vec2::new(4.0, 0.9))]),
            Some(Anchor::Movable(EntityId(7))),
            "the arc preferred nothing at all, so the test above proves nothing"
        );
    }

    #[test]
    fn a_wall_in_front_of_an_enemy_blocks_the_tap() {
        // Assist picks between things the tap could have hit. An Enemy behind
        // the terrain the ray meets is not one of them, or the arc would be
        // reaching through geometry rather than expressing a preference.
        let wall = [Aabb::new(Vec2::new(5.5, 0.0), Vec2::splat(0.5))];
        assert_eq!(
            cast_anchor(
                Vec2::ZERO,
                Vec2::X,
                &wall,
                [target(7, Vec2::new(10.0, 0.0))]
            ),
            Some(Anchor::Fixed(Vec2::new(5.0, 0.0))),
            "an Enemy behind a wall must not be preferred over it"
        );
    }

    #[test]
    fn an_enemy_behind_the_hero_is_never_a_target() {
        // Hostile input: the dot product that measures the arc is positive for
        // a direction and its reverse unless the sign is checked, so an Enemy
        // directly behind the tap would otherwise be a perfect match.
        let wall = [Aabb::new(Vec2::new(20.5, 0.0), Vec2::splat(0.5))];
        assert_eq!(
            cast_anchor(
                Vec2::ZERO,
                Vec2::X,
                &wall,
                [target(7, Vec2::new(-10.0, 0.0))]
            ),
            Some(Anchor::Fixed(Vec2::new(20.0, 0.0))),
            "an Enemy at the hero's back was treated as inside the arc"
        );
    }

    #[test]
    fn the_nearest_enemy_in_the_arc_wins() {
        let wall = [Aabb::new(Vec2::new(20.5, 0.0), Vec2::splat(0.5))];
        assert_eq!(
            cast_anchor(
                Vec2::ZERO,
                Vec2::X,
                &wall,
                [
                    target(9, Vec2::new(15.0, 0.4)),
                    target(7, Vec2::new(6.0, 0.4)),
                    target(8, Vec2::new(11.0, 0.4)),
                ]
            ),
            Some(Anchor::Movable(EntityId(7))),
            "Attached past a nearer Enemy"
        );
    }

    #[test]
    fn a_tap_with_no_direction_is_discarded_rather_than_normalized() {
        // A tap landing exactly on the hero. Normalizing a zero vector yields
        // NaN, and an Enemy sitting at the hero's own position would otherwise
        // be compared against it.
        let wall = [Aabb::new(Vec2::new(20.5, 0.0), Vec2::splat(0.5))];
        assert_eq!(
            cast_anchor(Vec2::ZERO, Vec2::ZERO, &wall, [target(7, Vec2::ZERO)]),
            None
        );
    }

    #[test]
    fn a_slot_whose_movable_anchor_has_gone_is_freed() {
        // ADR-0014. Asserted on the pure function here and through the `World`
        // seam below, because the rule is one line and the wiring that calls it
        // at the right point in the tick is the part that can be wrong.
        let mut tongues: Tongues = [
            Some(crate::Tongue {
                anchor: Anchor::Movable(EntityId(3)),
            }),
            Some(crate::Tongue {
                anchor: Anchor::Fixed(Vec2::new(1.0, 2.0)),
            }),
        ];
        let freed = release_dead_anchors(&mut tongues, |id| id != EntityId(3));
        assert_eq!(freed, 1);
        assert_eq!(tongues[0], None, "the dead Anchor's slot must be free");
        assert_eq!(
            tongues[1],
            Some(crate::Tongue {
                anchor: Anchor::Fixed(Vec2::new(1.0, 2.0))
            }),
            "a Fixed Anchor has no body to lose and must be untouched"
        );

        // Mutation check on the other half of the claim: with every body alive
        // nothing is freed at all, so the sweep is driven by the predicate and
        // not by the Anchor merely being Movable.
        let mut live: Tongues = [
            Some(crate::Tongue {
                anchor: Anchor::Movable(EntityId(3)),
            }),
            None,
        ];
        let before = live;
        assert_eq!(release_dead_anchors(&mut live, |_| true), 0);
        assert_eq!(live, before, "a live Anchor's slot was freed");
    }

    /// The rules that matter asserted where a player meets them: through the
    /// one seam, on a `Snapshot`, never on `World` internals and never on the
    /// order a slice happens to list things in.
    mod through_the_world_seam {
        use super::*;

        #[test]
        fn the_arc_prefers_an_enemy_over_the_terrain_behind_it_and_bends_nothing_otherwise() {
            // The AC's two halves in one fixture, so that the second is the
            // mutation of the first rather than a separate scene that might
            // differ for some other reason. The only thing that changes between
            // the two runs is the Enemy's spawn height.
            let scene = |enemy_y: f32| {
                let mut level = duel(1.0);
                level.enemy_spawns[0].pos = Vec2::new(10.0, enemy_y);
                let mut world = World::from_level(&level);
                let enemy = enemy_of(&world);
                // Along the x axis, straight at the wall. The aim is identical
                // in both runs, which is the whole point.
                tap(&mut world, 1, Vec2::new(10.0, 0.0));
                world.step();
                let anchor = world
                    .snapshot()
                    .tongues
                    .iter()
                    .find(|t| t.slot == 0)
                    .map(|t| t.anchor);
                (anchor, enemy)
            };

            // One unit off a ten unit aim is 5.7 degrees, inside the arc. The
            // ray misses the Enemy's box outright, so preference is the only
            // thing that can pick it.
            let (inside, enemy) = scene(1.0);
            assert_eq!(
                inside,
                Some(Anchor::Movable(enemy)),
                "an Enemy inside the arc must be preferred over the wall behind it"
            );

            // Four units off the same aim is 21.8 degrees, outside it. The
            // Anchor must then be the wall, at exactly the point a pure terrain
            // cast finds: the arc expresses a preference and never moves the
            // ray.
            let (outside, _) = scene(4.0);
            let pure = crate::tongue::cast(Vec2::ZERO, Vec2::X, &duel(1.0).static_terrain)
                .expect("the ray meets the wall");
            assert_eq!(
                outside,
                Some(Anchor::Fixed(pure)),
                "an Enemy outside the arc changed where the hero Attached"
            );
        }

        /// The Tongue force at rest on a Tongue `length` long, which is pure
        /// Hooke's law: the damping term is `damping * radial_vel` and is
        /// identically zero at zero velocity. ADR-0003 records this as the
        /// reason the #8 prototype's force figures reproduce exactly while its
        /// return speeds do not, and it is why every assertion below measures
        /// the **first** tick out of a standstill.
        fn force_at_rest(world: &World, length: f32) -> f32 {
            world.hero_pull_strength() * (length - REST_LENGTH)
        }

        /// One tick of displacement from rest under acceleration `a`, which is
        /// Velocity Verlet's position step with `vel` zero: `0.5 * a * DT^2`.
        fn first_tick_displacement(a: f32) -> f32 {
            0.5 * a * DT * DT
        }

        #[test]
        fn a_tongue_on_an_enemy_pulls_both_bodies_by_their_mass_ratio() {
            // ADR-0003: a Tongue on a Movable Anchor pulls both bodies by their
            // relative weights, so grabbing something heavy moves the hero and
            // grabbing something light moves the Enemy. The Tongue is
            // horizontal and the measurement is on x alone, so gravity
            // contributes exactly nothing rather than nearly nothing.
            for enemy_mass in [0.25f32, 10.0] {
                let mut world = World::from_level(&duel(enemy_mass));
                let hero = world.hero_id();
                let enemy = enemy_of(&world);
                let f = force_at_rest(&world, 10.0);
                assert!(
                    (f - 360.0).abs() < 1e-3,
                    "the fixture must reproduce the prototype's 360.000, got {f}"
                );

                tap(&mut world, 1, Vec2::new(10.0, 0.0));
                world.step();

                let hero_dx = world.snapshot().body(hero).expect("hero").pos.x;
                let enemy_dx = world.snapshot().body(enemy).expect("enemy").pos.x - 10.0;
                let expected_hero = first_tick_displacement(f / crate::DEFAULT_PLAYER_MASS);
                let expected_enemy = -first_tick_displacement(f / enemy_mass);
                // Tolerance is relative to the larger of the two, so the light
                // Enemy's big swing is not measured to a bound sized for the
                // heavy one's small nudge.
                let tol = 1e-4 * expected_hero.abs().max(expected_enemy.abs());
                assert!(
                    (hero_dx - expected_hero).abs() <= tol,
                    "hero moved {hero_dx} against the analytic {expected_hero} \
                     at Enemy mass {enemy_mass}"
                );
                assert!(
                    (enemy_dx - expected_enemy).abs() <= tol,
                    "Enemy moved {enemy_dx} against the analytic {expected_enemy} \
                     at Enemy mass {enemy_mass}"
                );
                // The ratio is the mechanic, and it is exact: `a = F/m` with
                // the same `F`, so the displacements are inversely as the
                // masses whatever the force happens to be.
                assert!(
                    (hero_dx / -enemy_dx - enemy_mass / crate::DEFAULT_PLAYER_MASS).abs() < 1e-3,
                    "displacement ratio {} is not the inverse mass ratio {}",
                    hero_dx / -enemy_dx,
                    enemy_mass / crate::DEFAULT_PLAYER_MASS
                );
            }
        }

        #[test]
        fn both_slots_on_one_enemy_pull_it_twice_as_hard() {
            // ADR-0003's force ceiling is per Tongue rather than per hero, and
            // the reaction is drawn from that same budget, so an Enemy held by
            // both slots feels up to `MAX_TONGUES * DEFAULT_FORCE_CEILING` just
            // as the hero does. Worth pinning rather than inferring: the
            // tempting fix of summing the reactions and then clamping would
            // mean a Tongue that weakened because another one existed, which is
            // precisely the property ADR-0003 exists to provide.
            let mut one = World::from_level(&duel(1.0));
            tap(&mut one, 1, Vec2::new(10.0, 0.0));
            one.step();
            let single = one.snapshot().body(enemy_of(&one)).expect("enemy").pos.x - 10.0;

            let mut two = World::from_level(&duel(1.0));
            // Two overlapping fingers in one frame, aimed at the same Enemy.
            // ADR-0006 makes that the only way to hold two Tongues at once.
            two.apply_touches(&[
                TouchSample {
                    id: 1,
                    phase: TouchPhase::Started,
                    pos: Vec2::new(10.0, 0.0),
                },
                TouchSample {
                    id: 2,
                    phase: TouchPhase::Started,
                    pos: Vec2::new(10.0, 0.0),
                },
            ]);
            two.step();
            assert_eq!(
                two.snapshot().tongues.len(),
                2,
                "the fixture must hold two Tongues on one Enemy: {:?}",
                two.snapshot().tongues
            );
            let doubled = two.snapshot().body(enemy_of(&two)).expect("enemy").pos.x - 10.0;

            assert!(single < 0.0, "one Tongue must pull the Enemy in: {single}");
            assert!(
                (doubled / single - 2.0).abs() < 1e-3,
                "two Tongues on one Enemy pulled it {}x and not 2x",
                doubled / single
            );
        }

        /// How many ticks the `duel` fixture stays a clean two-body problem.
        ///
        /// At `DEFAULT_PULL_STRENGTH` with a 9 unit Stretch the hero
        /// accelerates at 360 units per second squared, so the 9 unit gap
        /// closes in roughly 11 ticks and the two bodies then collide, which
        /// brings `combat` and its impulses into a measurement that was about a
        /// force. Ten ticks is the whole window before that, and the guards
        /// below assert the window really is clean rather than assuming it.
        const CLEAN_TICKS: u32 = 10;

        #[test]
        fn the_enemy_feels_an_equal_and_opposite_force() {
            // Newton's third law, measured as momentum rather than as force:
            // the pair's momentum changes only by the impulse gravity gave it,
            // because the two Tongue forces cancel.
            //
            // They cancel *exactly*, which is a property of how the tick is
            // arranged rather than a happy accident. Both evaluations of
            // ADR-0007's integrator see the same configuration for both bodies,
            // so the two forces are computed from one Stretch, one direction
            // and one relative radial velocity, and differ only in sign. The
            // bound here is float noise in the velocity update and nothing
            // else.
            let enemy_mass = 3.0f32;
            let mut world = World::from_level(&duel(enemy_mass));
            let hero = world.hero_id();
            let enemy = enemy_of(&world);
            tap(&mut world, 1, Vec2::new(10.0, 0.0));

            let total_mass = crate::DEFAULT_PLAYER_MASS + enemy_mass;
            for tick in 1..=CLEAN_TICKS {
                world.step();
                let hv = world.body_velocity(hero).expect("hero");
                let ev = world.body_velocity(enemy).expect("enemy");
                let momentum = crate::DEFAULT_PLAYER_MASS * hv + enemy_mass * ev;
                let gravity = total_mass * crate::GRAVITY * DT * tick as f32;
                assert!(
                    (momentum - gravity).length() < 1e-3,
                    "tick {tick}: the pair carries {momentum:?} where gravity alone \
                     gives {gravity:?}, so the reaction force is not equal and opposite"
                );
                // The x axis carries no gravity at all, so it is the sharper
                // statement: the pair's x momentum must stay at zero.
                assert!(
                    momentum.x.abs() < 1e-3,
                    "tick {tick}: the pair gained {} of x momentum from nowhere",
                    momentum.x
                );
            }

            // Vacuity guards. A pair that never moved would conserve its
            // momentum perfectly, and so would one whose Tongue had quietly
            // gone slack, so both bodies must have been pulled hard and must
            // still be apart and alive at the end of the window.
            let hp = world.snapshot().body(hero).expect("hero").pos;
            let ep = world.snapshot().body(enemy).expect("enemy").pos;
            assert!(
                hp.x > 1.0 && ep.x < 9.0,
                "the window measured almost no motion: hero {hp:?}, Enemy {ep:?}"
            );
            assert!(
                (ep - hp).length() > REST_LENGTH,
                "the bodies met inside the window, so combat entered the measurement"
            );
            assert_eq!(
                world.snapshot().tongues.len(),
                1,
                "the Tongue must be Attached throughout"
            );
        }

        #[test]
        fn one_tongue_on_terrain_and_one_on_an_enemy_is_solved_as_one_system() {
            // The AC's three-body clause. The hero-Enemy pair's momentum is
            // *not* conserved here, and the reason is the point: the Fixed
            // Anchor injects impulse into the pair from outside it. A
            // simulation that solved the two Tongues as independent pairs would
            // still show that, so the assertion is quantitative: the residual
            // has to be large against the same measurement taken with the Fixed
            // Anchor absent, which is the two-body control.
            let level = {
                let mut l = duel(1.0);
                // A ceiling face 10 units straight up, for the second Tongue.
                l.static_terrain
                    .push(Aabb::new(Vec2::new(0.0, 10.5), Vec2::splat(0.5)));
                l
            };
            let hero = World::from_level(&level).hero_id();

            let residual = |level: &Level, second_tongue: bool| -> Vec2 {
                let mut world = World::from_level(level);
                let enemy = enemy_of(&world);
                // Two overlapping fingers, because ADR-0006 makes that the only
                // way to hold two Tongues: a tap, a lift and another tap
                // returns to slot 0 and Releases it.
                let mut frame = vec![TouchSample {
                    id: 1,
                    phase: TouchPhase::Started,
                    pos: Vec2::new(10.0, 0.0),
                }];
                if second_tongue {
                    frame.push(TouchSample {
                        id: 2,
                        phase: TouchPhase::Started,
                        pos: Vec2::new(0.0, 10.0),
                    });
                }
                world.apply_touches(&frame);
                assert_eq!(
                    world.snapshot().tongues.len(),
                    0,
                    "a Tongue fired between ticks must not appear in the tick already published"
                );
                for _ in 0..CLEAN_TICKS {
                    world.step();
                }
                let expected = if second_tongue { 2 } else { 1 };
                assert_eq!(
                    world.snapshot().tongues.len(),
                    expected,
                    "the fixture must hold {expected} Tongues for this measurement"
                );
                let hv = world.body_velocity(hero).expect("hero");
                let ev = world.body_velocity(enemy).expect("enemy");
                let gravity = 2.0 * crate::GRAVITY * DT * CLEAN_TICKS as f32;
                hv + ev - gravity
            };

            let coupled = residual(&level, true);
            let pair_only = residual(&level, false);
            // The control is exact to one ulp: 6e-8 measured, which is float
            // noise in the velocity update and not a physical residual.
            assert!(
                pair_only.length() < 1e-5,
                "the two-body control must conserve the pair's momentum, got {pair_only:?}"
            );

            // The Fixed Anchor's pull starts at `k * (10 - REST_LENGTH)` = 360
            // and falls as the hero climbs toward it, reaching roughly 160 by
            // the end of the window, so the impulse it injects over ten ticks
            // is between `160/6` and `360/6`: 27 to 60. Bracketed on both
            // sides, because "far too much" is as wrong as "nothing" and only
            // one of the two is a vacuity guard. Measured: 42.95, against the
            // control's 6e-8.
            let injected = coupled.length();
            assert!(
                (20.0..100.0).contains(&injected),
                "the Fixed Anchor injected {injected} into the pair, outside the analytic \
                 20 to 100: {coupled:?}"
            );
            // And it is injected along the Fixed Anchor's pull, which is
            // straight up: a residual of the right size pointing anywhere else
            // would be a bug that happened to be large. The sideways part is
            // real but small, because the hero drifts toward the Enemy and
            // tilts the ceiling Tongue off vertical as it climbs.
            assert!(
                coupled.y > 0.0 && coupled.x.abs() < coupled.y * 0.3,
                "the injected impulse {coupled:?} does not point at the Fixed Anchor"
            );
        }

        #[test]
        fn a_slot_whose_enemy_despawns_is_freed_on_the_same_tick() {
            // ADR-0014, through the seam. The force and the drawn line already
            // vanish with the body; what this pins is that the slot does too,
            // so the player is not left holding a Tongue to nothing.
            let mut level = duel(1.0);
            // Tight enough that one shove puts the Enemy outside it, and wide
            // enough that the hero stays in. Deliberately not square, so a test
            // that compared the wrong axis would not pass.
            level.world_bounds = Aabb::new(Vec2::new(5.0, 0.0), Vec2::new(30.0, 12.0));
            let mut world = World::from_level(&level);
            let enemy = enemy_of(&world);

            tap(&mut world, 1, Vec2::new(10.0, 0.0));
            // Lifted at once, so slot 0 is unowned and the re-fire below can
            // reach it. ADR-0006 hands a Gesture the lowest slot no other
            // finger is driving, and a finger left down would push the next tap
            // onto slot 1 and prove nothing about slot 0 at all.
            lift(&mut world, 1);
            world.step();
            assert_eq!(
                world
                    .snapshot()
                    .tongues
                    .iter()
                    .find(|t| t.slot == 0)
                    .map(|t| t.anchor),
                Some(Anchor::Movable(enemy)),
                "the fixture must Attach to the Enemy, or nothing below is about a Movable Anchor"
            );

            // Flung out of the Kill Bound in one tick, so the despawn and the
            // slot are the same tick's business.
            world.place(enemy, Vec2::new(10.0, 0.0), Vec2::new(0.0, 4_000.0));
            world.step();

            assert!(
                world.snapshot().body(enemy).is_none(),
                "the fixture must despawn the Enemy"
            );
            assert!(
                world.snapshot().tongues.is_empty(),
                "the slot must be free the tick its Anchor left: {:?}",
                world.snapshot().tongues
            );

            // And free means usable: the next tap fires rather than Releasing
            // the ghost, which is the whole reason the slot is cleared. Under
            // ADR-0006 a tap on an occupied slot Releases it, so a slot left
            // claimed would answer this tap with an empty snapshot and the
            // player would have spent an input discovering the Tongue was gone.
            tap(&mut world, 2, Vec2::new(10_000.0, 0.0));
            world.step();
            let refired = world
                .snapshot()
                .tongues
                .iter()
                .find(|t| t.slot == 0)
                .map(|t| t.anchor);
            match refired {
                Some(Anchor::Fixed(p)) => assert!(
                    (p.x - 20.0).abs() < 1e-2,
                    "the re-fire must land on the wall's inner face, got {p:?}"
                ),
                other => panic!("a freed slot must fire on the next tap, got {other:?}"),
            }
        }

        #[test]
        fn a_slot_whose_enemy_is_destroyed_is_freed_too() {
            // The other way ADR-0011 loses a body, and the one its Thrown
            // matrix set the precedent on: destruction frees the Anchor slot.
            // Easy to miss, because destruction and the Kill Bound remove
            // bodies at two different points in the tick.
            let mut level = duel(1.0);
            level.enemy_spawns[0].toughness = 1.0;
            let mut world = World::from_level(&level);
            let hero = world.hero_id();
            let enemy = enemy_of(&world);

            tap(&mut world, 1, Vec2::new(10.0, 0.0));
            world.step();
            assert_eq!(
                world
                    .snapshot()
                    .tongues
                    .iter()
                    .find(|t| t.slot == 0)
                    .map(|t| t.anchor),
                Some(Anchor::Movable(enemy)),
                "the fixture must Attach to the Enemy"
            );

            // Driven into the Enemy hard enough to empty a 1 unit pool.
            let before = world.snapshot().body(enemy).expect("enemy").pos;
            world.place(hero, before - Vec2::new(2.0, 0.0), Vec2::new(60.0, 0.0));
            world.step();

            assert!(
                world.snapshot().body(enemy).is_none(),
                "the fixture must destroy the Enemy"
            );
            assert!(
                world.snapshot().tongues.is_empty(),
                "a destroyed Anchor must free its slot: {:?}",
                world.snapshot().tongues
            );
            // The hero keeps its momentum. ADR-0014 makes this a Release, and
            // ADR-0012 makes a Release instant retraction with nothing spent,
            // so the only thing that may have touched the hero's velocity is
            // the impact itself.
            let v = world.body_velocity(hero).expect("hero");
            assert!(
                v.x > 0.0,
                "the hero must keep the momentum it had, got {v:?}"
            );
        }

        #[test]
        fn a_tongue_on_a_fixed_anchor_is_unaffected_by_a_despawn_elsewhere() {
            // The mutation half of the claim above. If the sweep freed slots
            // for any reason other than a dead Movable Anchor, a Fixed Anchor
            // would be collateral and no test above would notice.
            let mut level = duel(1.0);
            level.world_bounds = Aabb::new(Vec2::new(5.0, 0.0), Vec2::new(30.0, 12.0));
            // A ceiling 10 units straight up. The Enemy is level with the hero,
            // so a tap upward has it exactly perpendicular to the aim and well
            // outside any arc, which is how this fixture gets a Fixed Anchor
            // while an Enemy is still in the world to despawn.
            level
                .static_terrain
                .push(Aabb::new(Vec2::new(0.0, 10.5), Vec2::splat(0.5)));
            let mut world = World::from_level(&level);
            let enemy = enemy_of(&world);

            tap(&mut world, 1, Vec2::new(0.0, 1_000.0));
            world.step();
            let anchor = world
                .snapshot()
                .tongues
                .iter()
                .find(|t| t.slot == 0)
                .map(|t| t.anchor);
            assert_eq!(
                anchor,
                Some(Anchor::Fixed(Vec2::new(0.0, 10.0))),
                "the fixture must Attach to the ceiling and not to the Enemy"
            );

            world.place(enemy, Vec2::new(10.0, 0.0), Vec2::new(0.0, 4_000.0));
            world.step();
            assert!(
                world.snapshot().body(enemy).is_none(),
                "the Enemy must leave"
            );
            assert_eq!(
                world
                    .snapshot()
                    .tongues
                    .iter()
                    .find(|t| t.slot == 0)
                    .map(|t| t.anchor),
                anchor,
                "a Fixed Anchor lost its slot to someone else's despawn"
            );
        }
    }
}
