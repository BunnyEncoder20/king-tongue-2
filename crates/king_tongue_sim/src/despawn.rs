//! The Kill Bound: the world-space rectangle outside which a body is removed.
//!
//! GLOSSARY calls this the Kill Bound, and the level declares it as
//! `world_bounds`. ADR-0011 is the contract, and the one thing it insists on is
//! the coordinate space: the test is world-space and reads the level's declared
//! rectangle, never anything derived from the screen or the viewport. Screen
//! dimensions vary across the device floor, so a screen-space test would make
//! the same inputs produce different outcomes on different phones and void the
//! determinism ADR-0002 and ADR-0007 were bought for. The Core Sim has no
//! concept of a screen and must not gain one, which is why the only input to
//! this module is level data.
//!
//! The module is pure. It takes a rectangle and a position and answers a
//! question; it owns no state and mutates nothing, so the one place in the Sim
//! Tick allowed to remove a body stays in `world`.

use crate::level::Aabb;
use glam::Vec2;

/// Whether a body at `pos` has left the Kill Bound and must be removed.
///
/// The test is on the body's centre at the end of the Sim Tick, not on its
/// bounding circle and not on the path it travelled. A Kill Bound is authored
/// deliberately wider than the playfield (the shipped scene's is twice it), so
/// the difference a radius would make is never visible, and a centre test is one
/// comparison that cannot disagree with itself between bodies of different size.
///
/// Touching the boundary exactly is still inside: [`Aabb::contains`] is
/// inclusive, so a body resting on the bound is not removed and a level whose
/// bound hugs its terrain does not delete the scene. Leaving means strictly
/// outside.
///
/// A non-finite position also counts as having left. Every float comparison
/// against a NaN is false, so `contains` already answers "not inside", and that
/// is the useful answer: a body whose position has been poisoned can never come
/// back, and keeping it would spread NaN into the Snapshot the Shell draws.
pub fn left_kill_bound(bound: &Aabb, pos: Vec2) -> bool {
    !bound.contains(pos)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bound with different extents per axis, so a test cannot pass by
    /// comparing against the wrong axis.
    fn bound() -> Aabb {
        Aabb::new(Vec2::new(10.0, 4.0), Vec2::new(6.0, 2.0))
    }

    #[test]
    fn a_body_inside_the_bound_stays() {
        assert!(!left_kill_bound(&bound(), Vec2::new(10.0, 4.0)));
    }

    #[test]
    fn each_edge_of_the_bound_removes_a_body_that_crosses_it() {
        let b = bound();
        // x spans 4 to 16, y spans 2 to 6, read off the level data rather than
        // written as literals twice.
        let (min, max) = (b.min(), b.max());
        for outside in [
            Vec2::new(min.x - 0.01, b.centre.y),
            Vec2::new(max.x + 0.01, b.centre.y),
            Vec2::new(b.centre.x, min.y - 0.01),
            Vec2::new(b.centre.x, max.y + 0.01),
        ] {
            assert!(
                left_kill_bound(&b, outside),
                "{outside:?} is outside {b:?} and must despawn"
            );
        }
    }

    #[test]
    fn resting_exactly_on_the_boundary_is_still_inside() {
        // A level may legitimately declare a bound that hugs its terrain, and an
        // exclusive test would delete a body standing on the edge of it. Also
        // the tangency case the ASCII sketch produces, where the bound is
        // exactly the sketch's extent.
        let b = bound();
        for on_edge in [b.min(), b.max(), Vec2::new(b.min().x, b.max().y)] {
            assert!(
                !left_kill_bound(&b, on_edge),
                "{on_edge:?} sits on the boundary of {b:?} and must survive"
            );
        }
    }

    /// The same rule asserted where it matters: through the Core Sim's one
    /// seam, by building a `Level`, advancing Sim Ticks, and reading
    /// `Snapshot`s. Nothing here touches `World` internals or depends on the
    /// order bodies happen to occupy in a list.
    mod through_the_world_seam {
        use super::*;
        use crate::level::{EnemySpawn, Level};
        use crate::world::World;
        use crate::EntityId;

        /// An arena with the Kill Bound given explicitly and no terrain at all,
        /// so a body flung at an edge meets the bound and nothing else.
        ///
        /// The bound is deliberately not square: a test that passed by
        /// comparing the wrong axis would survive a square one.
        fn arena(bound: Aabb, enemy_spawns: Vec<EnemySpawn>) -> Level {
            let level = Level {
                world_bounds: bound,
                static_terrain: vec![],
                player_spawn: bound.centre,
                enemy_spawns,
            };
            level.validate().expect("fixture must be a valid level");
            level
        }

        fn enemy_at(pos: Vec2) -> EnemySpawn {
            EnemySpawn {
                pos,
                toughness: 100.0,
                mass: 1.0,
            }
        }

        /// Every identity in the current Snapshot.
        fn ids(world: &World) -> Vec<EntityId> {
            world.snapshot().bodies.iter().map(|b| b.id).collect()
        }

        /// The one body that is not the hero.
        fn only_enemy(world: &World) -> EntityId {
            let found: Vec<EntityId> = ids(world)
                .into_iter()
                .filter(|id| *id != world.hero_id())
                .collect();
            assert_eq!(found.len(), 1, "this fixture spawns exactly one Enemy");
            found[0]
        }

        #[test]
        fn a_body_pushed_past_each_edge_of_the_bound_despawns() {
            // Each of the four edges in turn, because a containment test can be
            // wrong on one side only: a sign slip or a `min` where a `max` was
            // meant leaves three edges working.
            let bound = Aabb::new(Vec2::new(10.0, 4.0), Vec2::new(6.0, 3.0));
            for push in [
                Vec2::new(-60.0, 0.0),
                Vec2::new(60.0, 0.0),
                Vec2::new(0.0, -60.0),
                Vec2::new(0.0, 60.0),
            ] {
                let level = arena(bound, vec![enemy_at(bound.centre)]);
                let mut world = World::from_level(&level);
                let enemy = only_enemy(&world);
                world.place(enemy, bound.centre, push);

                // Sixty ticks is one second: at this speed the body is well
                // clear of the bound, and the assertion is that it is gone
                // rather than that it left on a particular tick.
                for _ in 0..60 {
                    world.step();
                }

                assert!(
                    !ids(&world).contains(&enemy),
                    "a body pushed {push:?} out of {bound:?} must despawn"
                );
            }
        }

        #[test]
        fn the_bound_applies_to_the_hero_as_well_as_to_enemies() {
            // "Any body leaving the level's Kill Bound is removed" is taken at
            // its word. ADR-0011 names the Enemy case because that is what the
            // rule is for, but it excludes nothing, and ADR-0010 settles it
            // from the other side: it validates `player_spawn` against the
            // bound precisely because "a hero spawned outside the kill bound
            // would despawn on tick one". A rule with a hero-shaped hole in it
            // would also be a branch on `BodyKind` in the one place the tick is
            // allowed to remove a body.
            //
            // This is not the hero's fail state, which stays deferred in the
            // map's fog: nothing here is a lose condition, a restart, or a run
            // structure. The hero simply stops being simulated, which the Shell
            // already handles by drawing the world origin at the centre of the
            // screen rather than by branching on it.
            let bound = Aabb::new(Vec2::ZERO, Vec2::new(10.0, 10.0));
            let level = arena(bound, vec![]);
            let mut world = World::from_level(&level);
            let hero = world.hero_id();
            world.place(hero, Vec2::ZERO, Vec2::new(60.0, 0.0));

            for _ in 0..60 {
                world.step();
            }

            assert!(
                world.snapshot().body(hero).is_none(),
                "the hero left the Kill Bound and must be removed like anything else"
            );
            // And the simulation keeps running rather than panicking or
            // wedging, because a Sim Tick with no hero in it is reachable the
            // moment the rule above holds.
            let tick = world.snapshot().tick;
            world.step();
            assert_eq!(world.snapshot().tick, tick + 1);
            assert_eq!(
                world.hero_id(),
                hero,
                "the hero's identity must not be reissued to anything"
            );
        }

        #[test]
        fn a_despawned_hero_takes_its_tongues_out_of_the_snapshot_with_it() {
            // A despawned body must not be drawn, and a Tongue drawn from where
            // the hero used to be is the same body still on screen under
            // another name. The Shell draws whatever `Snapshot.tongues` holds
            // and is not allowed a branch to decide otherwise, so the absence
            // has to be the Core Sim's doing.
            let bound = Aabb::new(Vec2::new(5.0, 5.0), Vec2::new(5.0, 5.0));
            let level = Level {
                world_bounds: bound,
                // Something to Attach to, directly above the hero.
                static_terrain: vec![Aabb::new(Vec2::new(5.0, 9.0), Vec2::new(5.0, 0.5))],
                player_spawn: Vec2::new(5.0, 5.0),
                enemy_spawns: vec![],
            };
            level.validate().expect("fixture must be a valid level");

            let mut world = World::from_level(&level);
            let hero = world.hero_id();
            world.apply_touches(&[crate::input::TouchSample {
                id: 1,
                phase: crate::input::TouchPhase::Started,
                pos: Vec2::new(5.0, 9.0),
            }]);
            world.step();
            assert!(
                !world.snapshot().tongues.is_empty(),
                "the fixture must land a Tongue, or this asserts nothing"
            );

            // Flung sideways out of the bound while still Attached.
            world.place(hero, Vec2::new(5.0, 5.0), Vec2::new(90.0, 0.0));
            for _ in 0..60 {
                world.step();
            }

            assert!(world.snapshot().body(hero).is_none(), "the hero despawned");
            assert!(
                world.snapshot().tongues.is_empty(),
                "a Tongue must not be drawn from a hero that is no longer there"
            );
        }

        #[test]
        fn the_bound_is_the_one_the_level_declared_and_not_a_fixed_distance() {
            // The closest a headless test can come to asserting "never the
            // viewport". Two runs identical in every respect except the
            // rectangle the level declares, with the same body flung the same
            // way: the wide bound keeps it and the narrow one removes it. A
            // despawn rule reading a screen size, a hardcoded distance, or the
            // extent of the terrain instead of `world_bounds` would give both
            // runs the same answer, and this is the only assertion that would
            // notice.
            let fling = Vec2::new(30.0, 0.0);
            let centre = Vec2::new(10.0, 4.0);
            let narrow = Aabb::new(centre, Vec2::new(6.0, 3.0));
            let wide = Aabb::new(centre, Vec2::new(600.0, 300.0));

            let mut outcomes = Vec::new();
            for bound in [narrow, wide] {
                let level = arena(bound, vec![enemy_at(centre)]);
                let mut world = World::from_level(&level);
                let enemy = only_enemy(&world);
                world.place(enemy, centre, fling);
                for _ in 0..30 {
                    world.step();
                }
                outcomes.push(ids(&world).contains(&enemy));
            }

            assert_eq!(
                outcomes,
                vec![false, true],
                "the narrow bound must remove the body and the wide one must keep it"
            );
        }

        #[test]
        fn a_despawn_mid_flight_does_not_slide_a_surviving_body_across_the_level() {
            // The regression the `EntityId` correspondence rule of ADR-0009
            // exists to prevent, provoked through the simulation rather than
            // from two hand-built snapshots. `frame` already tests the rule on
            // literal snapshots; what nothing covered is whether the Kill Bound
            // produces the shape of snapshot pair that rule needs, which is the
            // half of it this ticket could get wrong.
            //
            // The body that leaves is spawned *before* the survivor on purpose.
            // Removing it shifts every later body's index down by one, so a
            // frame matched on slice position would blend the survivor against
            // the departed body's last coordinates and draw it travelling the
            // whole way across the level in a single frame.
            let bound = Aabb::new(Vec2::ZERO, Vec2::new(120.0, 120.0));
            let departing_spawn = Vec2::new(-100.0, 0.0);
            let surviving_spawn = Vec2::new(20.0, 0.0);
            let level = arena(
                bound,
                vec![enemy_at(departing_spawn), enemy_at(surviving_spawn)],
            );

            let mut world = World::from_level(&level);
            let departing = world
                .snapshot()
                .bodies
                .iter()
                .find(|b| b.pos == departing_spawn)
                .expect("the departing Enemy must spawn")
                .id;
            let surviving = world
                .snapshot()
                .bodies
                .iter()
                .find(|b| b.pos == surviving_spawn)
                .expect("the surviving Enemy must spawn")
                .id;

            world.place(departing, departing_spawn, Vec2::new(-60.0, 0.0));
            world.place(surviving, surviving_spawn, Vec2::new(5.0, 0.0));

            // Step until the Kill Bound takes the first one, so the pair of
            // snapshots under test is exactly the one that straddles the
            // despawn: `previous` still holds it and `current` does not.
            let mut ticks = 0;
            while world.snapshot().body(departing).is_some() {
                world.step();
                ticks += 1;
                assert!(ticks < 600, "the departing Enemy never left the bound");
            }
            let last_seen = world
                .previous_snapshot()
                .body(departing)
                .expect("the tick before the despawn must still hold it")
                .pos;
            let before = world
                .previous_snapshot()
                .body(surviving)
                .expect("the survivor must be in both snapshots")
                .pos;
            let after = world
                .snapshot()
                .body(surviving)
                .expect("the survivor must be in both snapshots")
                .pos;

            let mut view = crate::frame::Snapshot::default();
            crate::frame::interpolate_into(
                world.previous_snapshot(),
                world.snapshot(),
                0.5,
                &mut view,
            );

            assert!(
                view.body(departing).is_none(),
                "a body only in `previous` despawned and must not be drawn at all"
            );
            let drawn = view
                .body(surviving)
                .expect("the survivor must still be drawn")
                .pos;

            // Within its own tick's travel, which is a fraction of a unit, and
            // nowhere near where the departed body was. The gap is asserted
            // rather than assumed, so the test cannot pass by the two bodies
            // happening to be in the same place.
            assert!(
                (last_seen - before).length() > 50.0,
                "the fixture must put the two bodies far apart to be a real test: \
                 departed at {last_seen:?}, survivor at {before:?}"
            );
            let lo = before.min(after);
            let hi = before.max(after);
            assert!(
                drawn.cmpge(lo - 1e-4).all() && drawn.cmple(hi + 1e-4).all(),
                "the survivor must be drawn between its own two positions \
                 {before:?} and {after:?}, got {drawn:?}"
            );
        }

        #[test]
        fn an_identity_removed_by_the_bound_is_never_handed_out_again() {
            // Interpolation correctness rests on identity, so a recycled
            // `EntityId` would blend two different bodies into one another and
            // the frame contract has no way to notice. Asserted as the property
            // itself: once an identity leaves a Snapshot it never reappears in
            // one, for the whole run.
            let bound = Aabb::new(Vec2::ZERO, Vec2::new(40.0, 40.0));
            let level = arena(
                bound,
                vec![
                    enemy_at(Vec2::new(-30.0, 0.0)),
                    enemy_at(Vec2::new(30.0, 0.0)),
                ],
            );
            let mut world = World::from_level(&level);

            let start = ids(&world);
            assert_eq!(start.len(), 3, "one hero and two Enemies");
            for id in &start {
                if *id != world.hero_id() {
                    // Flung outward, so both leave the bound on different ticks
                    // and the run has two separate despawns in it.
                    let pos = world.snapshot().body(*id).expect("just read it").pos;
                    world.place(*id, pos, pos.normalize_or_zero() * 40.0);
                }
            }

            let mut gone: Vec<EntityId> = Vec::new();
            for _ in 0..400 {
                world.step();
                let present = ids(&world);
                for id in &gone {
                    assert!(
                        !present.contains(id),
                        "{id:?} came back after despawning, so an identity was reused"
                    );
                }
                for id in &start {
                    if !present.contains(id) && !gone.contains(id) {
                        gone.push(*id);
                    }
                }
            }

            assert_eq!(
                gone.len(),
                2,
                "both Enemies should have left the bound, so the assertion above \
                 had something to guard"
            );
        }
    }

    #[test]
    fn a_poisoned_position_counts_as_having_left() {
        // Hostile on purpose. A NaN compares false against everything, so a
        // containment test built the other way round ("is it outside?") would
        // answer no and keep a body whose position can never recover, and the
        // NaN would reach the Snapshot the Shell draws.
        let b = bound();
        assert!(left_kill_bound(&b, Vec2::new(f32::NAN, 4.0)));
        assert!(left_kill_bound(&b, Vec2::new(10.0, f32::NAN)));
        assert!(left_kill_bound(&b, Vec2::splat(f32::INFINITY)));
        assert!(left_kill_bound(&b, Vec2::splat(f32::NEG_INFINITY)));
    }
}
