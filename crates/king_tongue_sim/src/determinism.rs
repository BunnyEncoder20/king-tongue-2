//! The determinism harness: run a scripted scene twice, prove it agreed.
//!
//! This is the test that matters more than the rest. ADR-0002's fixed Sim Tick
//! and ADR-0007's integrator were both bought to make the same Level plus the
//! same inputs produce the same sequence of [`Snapshot`]s, and without a harness
//! asserting it the whole suite is measuring a moving target.
//!
//! Every later ticket extends this rather than writing its own: the Tongue, the
//! Gesture tracker, Slam, and the Combo each arrive as a [`Script`] run through
//! [`assert_deterministic`].
//!
//! Gated behind `test-util`, like `Level::from_ascii`, so nothing here can reach
//! a build that ships.

use crate::frame::Snapshot;
use crate::input::{TouchPhase, TouchSample};
use crate::level::Level;
use crate::world::World;
use glam::Vec2;

/// A scripted run's input: given the Sim Tick about to be simulated and the
/// `World` about to simulate it, apply whatever a player would have done.
///
/// A plain `fn` and not a closure, deliberately. A closure could capture state
/// of its own, and a script that remembers something between runs is exactly
/// the thing that makes a determinism test pass while the simulation diverges.
/// A function pointer cannot.
pub type Script = fn(tick: u64, world: &mut World);

/// The script that presses nothing. A hero falling, and nothing else.
pub fn no_input(_tick: u64, _world: &mut World) {}

/// Taps once, aiming `aim` away from wherever the hero is at that moment.
///
/// The aim is relative to the hero and so to the simulation's own state, which
/// is what makes this worth scripting: a fixed world-space point would exercise
/// the ray cast, but an aim that tracks the hero also makes the tap's *result*
/// depend on every tick before it, so a divergence anywhere upstream reaches
/// the Anchor and shows up in the trace.
fn aim(world: &mut World, id: u64, phase: TouchPhase, offset: Vec2) {
    let Some(pos) = world.snapshot().body(world.hero_id()).map(|body| body.pos) else {
        return;
    };
    world.apply_touches(&[TouchSample {
        id,
        phase,
        pos: pos + offset,
    }]);
}

/// Fires one Tongue up and to the left, then a second up and to the right,
/// lifting each finger the frame after it lands.
///
/// This is the script that makes the harness mean something for the first time.
/// Until there was an input path, the only thing it could assert was that a
/// hero falls the same way twice, which no integrator would get wrong. Firing
/// brings in the parts that actually could: a ray cast against terrain, slot
/// claiming keyed on touch ids, and a spring force whose Anchor is chosen from
/// simulation state.
///
/// The phases are deliberate. `Started` on one frame and `Ended` or
/// `Cancelled` on the next is exactly how long the platform makes those
/// visible, and the `Stationary` in between is what a finger that is still down
/// reports, so a script that re-fired on a hold would show up here as two
/// Tongues where one was intended.
pub fn fires_two_tongues(tick: u64, world: &mut World) {
    const UP_LEFT: Vec2 = Vec2::new(-2.0, 4.0);
    const UP_RIGHT: Vec2 = Vec2::new(3.0, 2.0);
    match tick {
        20 => aim(world, 1, TouchPhase::Started, UP_LEFT),
        21 => aim(world, 1, TouchPhase::Stationary, UP_LEFT),
        22 => aim(world, 1, TouchPhase::Ended, UP_LEFT),
        // Well after the first, so the hero is Swinging on one Tongue when the
        // second Gesture arrives and the second Anchor depends on where that
        // Swing had carried it.
        90 => aim(world, 2, TouchPhase::Started, UP_RIGHT),
        // Cancelled rather than Ended, because the platform uses it when the
        // app loses focus and ADR-0006 forbids reading either as a Release.
        91 => aim(world, 2, TouchPhase::Cancelled, UP_RIGHT),
        _ => {}
    }
}

/// Taps three times in the same frame, every frame, forever.
///
/// Hostile input as a script. Three concurrent ids must leave two Tongues, and
/// hammering the same ids every frame must not re-fire them, so the whole run
/// after the first frame has to be identical to a run that tapped once.
pub fn hammers_three_fingers(_tick: u64, world: &mut World) {
    for id in 1..=3 {
        aim(
            world,
            id,
            TouchPhase::Started,
            Vec2::new(id as f32 - 2.0, 3.0),
        );
    }
}

/// Launches the hero across a line of Enemies hard enough to cover all three of
/// ADR-0011's impact bands in one run.
///
/// The hero's momentum is assigned rather than earned by Swinging, which is what
/// lets combat be scripted before a Tongue exists. The first Enemy is a one-shot
/// and each impact drains the hero, so later Enemies are chipped and the last of
/// them is only Kicked: destruction, a spent pool, and a repel all appear in one
/// trace, and destruction is the case most likely to diverge because it changes
/// the body count mid-run.
pub fn slam_run(tick: u64, world: &mut World) {
    if tick == 0 {
        let hero = world.hero_id();
        world.place(hero, Vec2::new(0.5, 2.5), Vec2::new(120.0, 0.0));
    }
}

/// Flings every body out of the level's Kill Bound, each on its own tick.
///
/// The hero is launched along the floor and out of the right edge; the Enemies
/// are launched straight up and out of the top. They start at different
/// distances from the edge they leave through, so the bodies despawn on
/// different ticks rather than all at once, which is what makes the trace carry
/// a changing body count for several ticks running.
///
/// Nothing in this run carries enough momentum to destroy anything, so every
/// removal in it is the Kill Bound and not combat. That matters for what the
/// trace proves: ADR-0011's two ways of losing a body are easy to confuse, and
/// a script where both happened could not tell which one had diverged.
pub fn kill_bound_run(tick: u64, world: &mut World) {
    if tick != 0 {
        return;
    }

    let hero = world.hero_id();
    // Collected before anything is placed: `place` needs the `World` mutably,
    // and reading the snapshot first keeps the two borrows apart. Snapshot
    // order is stable within a run, so the order these are applied in is too.
    let enemies: Vec<(crate::EntityId, Vec2)> = world
        .snapshot()
        .bodies
        .iter()
        .filter(|b| b.id != hero)
        .map(|b| (b.id, b.pos))
        .collect();
    for (id, pos) in enemies {
        world.place(id, pos, Vec2::new(0.0, 90.0));
    }

    let hero_pos = world.snapshot().body(hero).map(|b| b.pos);
    if let Some(pos) = hero_pos {
        world.place(hero, pos, Vec2::new(90.0, 0.0));
    }
}

/// Runs `ticks` Sim Ticks and returns every `Snapshot` along the way, starting
/// with the one from before any tick ran.
pub fn record(level: &Level, ticks: u32, script: Script) -> Vec<Snapshot> {
    let mut world = World::from_level(level);
    let mut trace = Vec::with_capacity(ticks as usize + 1);
    trace.push(world.snapshot().clone());
    for _ in 0..ticks {
        script(world.snapshot().tick, &mut world);
        world.step();
        trace.push(world.snapshot().clone());
    }
    trace
}

/// Runs the same scene and script twice and fails if the two traces differ in
/// any bit.
pub fn assert_deterministic(level: &Level, ticks: u32, script: Script) {
    let first = record(level, ticks, script);
    let second = record(level, ticks, script);
    if let Some(where_) = first_divergence(&first, &second) {
        panic!("the same Level and Script diverged between runs: {where_}");
    }
}

/// Describes the first place two traces disagree, or `None` if they are
/// identical.
///
/// Compared bit by bit rather than with `==`, because the claim is that the
/// traces are *identical* and float equality is looser than that: it reads
/// `-0.0` and `0.0` as the same number, and a sign flip at a contact is the
/// kind of divergence worth catching early.
///
/// Position within each list is compared directly. That is not the forbidden
/// kind of order dependence: the harness does not assume any particular order,
/// it requires that two runs of the same code produce the same one, which is
/// part of what byte-identical means.
pub fn first_divergence(left: &[Snapshot], right: &[Snapshot]) -> Option<String> {
    if left.len() != right.len() {
        return Some(format!(
            "trace lengths differ: {} against {}",
            left.len(),
            right.len()
        ));
    }

    for (a, b) in left.iter().zip(right) {
        if a.tick != b.tick {
            return Some(format!(
                "tick numbers differ: {} against {}",
                a.tick, b.tick
            ));
        }
        let tick = a.tick;

        if a.bodies.len() != b.bodies.len() {
            return Some(format!(
                "tick {tick}: body counts differ, {} against {}",
                a.bodies.len(),
                b.bodies.len()
            ));
        }
        for (i, (x, y)) in a.bodies.iter().zip(&b.bodies).enumerate() {
            if x.id != y.id {
                return Some(format!(
                    "tick {tick}: body {i} identities differ, {:?} against {:?}",
                    x.id, y.id
                ));
            }
            if x.kind != y.kind {
                return Some(format!(
                    "tick {tick}: body {:?} kinds differ, {:?} against {:?}",
                    x.id, x.kind, y.kind
                ));
            }
            if differs(x.pos, y.pos) {
                return Some(format!(
                    "tick {tick}: body {:?} positions differ, {:?} against {:?}",
                    x.id, x.pos, y.pos
                ));
            }
            if differs(x.facing, y.facing) {
                return Some(format!(
                    "tick {tick}: body {:?} facings differ, {:?} against {:?}",
                    x.id, x.facing, y.facing
                ));
            }
        }

        if a.tongues.len() != b.tongues.len() {
            return Some(format!(
                "tick {tick}: Tongue counts differ, {} against {}",
                a.tongues.len(),
                b.tongues.len()
            ));
        }
        for (x, y) in a.tongues.iter().zip(&b.tongues) {
            if x.slot != y.slot || x.anchor_id != y.anchor_id {
                return Some(format!(
                    "tick {tick}: Tongue slots or Anchors differ, {x:?} against {y:?}"
                ));
            }
            if differs(x.from, y.from) || differs(x.to, y.to) {
                return Some(format!(
                    "tick {tick}: Tongue slot {} endpoints differ, {x:?} against {y:?}",
                    x.slot
                ));
            }
        }
    }

    None
}

/// Bitwise inequality, so `-0.0` against `0.0` counts and a NaN never silently
/// compares equal to itself.
fn differs(a: Vec2, b: Vec2) -> bool {
    a.x.to_bits() != b.x.to_bits() || a.y.to_bits() != b.y.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene with the shape of the shipped one: a hero above a floor, with
    /// Enemies and a platform to make the trace carry more than one body.
    fn scene() -> Level {
        Level::from_ascii(
            "
            ....@....
            .........
            ..####...
            .e.....e.
            #########
            ",
        )
    }

    /// The same scene, walled in, so a Gesture in any direction meets terrain
    /// and a scripted tap always has something to Attach to.
    fn enclosed_scene() -> Level {
        Level::from_ascii(
            "
            ###########
            #....@....#
            #.........#
            #..####...#
            #.e.....e.#
            ###########
            ",
        )
    }

    #[test]
    fn the_same_level_and_script_produce_identical_snapshot_sequences() {
        // ADR-0002 and ADR-0007 were bought for exactly this property. Without
        // it a regression is anecdotal rather than reproducible.
        assert_deterministic(&scene(), 300, no_input);
    }

    /// A line of Enemies at the hero's height, with a floor under everything.
    /// `slam_run` launches the hero along the row.
    fn combat_scene() -> Level {
        Level::from_ascii(
            "
            .........
            @.eeeee..
            .........
            #########
            ",
        )
    }

    #[test]
    fn a_scripted_slam_run_produces_identical_snapshot_sequences() {
        // Combat is where determinism is easiest to lose: an impact removes a
        // body mid-run, so the body list changes length and anything that
        // depended on iteration order or on a reused `EntityId` diverges here
        // and nowhere else.
        assert_deterministic(&combat_scene(), 300, slam_run);
    }

    #[test]
    fn the_slam_run_script_really_destroys_enemies() {
        // Guards the test above from passing vacuously. A script that failed to
        // land a single impact would still be perfectly deterministic, and the
        // combat trace would be asserting nothing about combat.
        let trace = record(&combat_scene(), 300, slam_run);
        let before = trace
            .first()
            .expect("a trace has a first tick")
            .bodies
            .len();
        let after = trace.last().expect("a trace has a last tick").bodies.len();
        assert!(
            after < before,
            "the scripted run destroyed nothing: {before} bodies throughout"
        );
    }

    /// A hero on a floor with a row of Enemies well above its path, so that
    /// `kill_bound_run` can fling everything out of the bound without anything
    /// touching anything else. The Kill Bound is the sketch's own extent.
    fn kill_bound_scene() -> Level {
        Level::from_ascii(
            "
            .eee.....
            .........
            @........
            #########
            ",
        )
    }

    #[test]
    fn a_run_that_despawns_bodies_produces_identical_snapshot_sequences() {
        // Despawn changes the length of the body list mid-run, which is the
        // same class of divergence combat risks and arrives by a different
        // route: a `retain` whose predicate read anything outside level data,
        // or that ran at a different point in the tick on one pass than on
        // another, would show up here and nowhere else.
        assert_deterministic(&kill_bound_scene(), 300, kill_bound_run);
    }

    #[test]
    fn the_kill_bound_script_really_despawns_everything() {
        // Guards the test above from passing vacuously, the same way the slam
        // run's guard does. A script that flung nothing far enough would be
        // perfectly deterministic and would assert nothing about despawn.
        let trace = record(&kill_bound_scene(), 300, kill_bound_run);
        let before = trace
            .first()
            .expect("a trace has a first tick")
            .bodies
            .len();
        assert_eq!(before, 4, "one hero and three Enemies");

        // Nothing in this scene can be destroyed: the Enemies a sketch spawns
        // carry 100 Toughness and the fastest thing here carries 90 momentum,
        // so an emptied trace can only be the Kill Bound's work.
        let after = trace.last().expect("a trace has a last tick").bodies.len();
        assert_eq!(after, 0, "every body should have left the bound");

        // And they left on different ticks rather than all at once, so the
        // trace really does carry several changes of body count.
        let counts: Vec<usize> = trace.iter().map(|s| s.bodies.len()).collect();
        let drops = counts.windows(2).filter(|w| w[1] < w[0]).count();
        assert!(
            drops >= 2,
            "the bodies should leave on separate ticks, got counts {counts:?}"
        );
    }

    #[test]
    fn firing_and_attaching_produce_identical_snapshot_sequences() {
        // The property ADR-0002 and ADR-0007 were bought for, now over the part
        // of the simulation that could plausibly lose it: a ray cast choosing
        // an Anchor, a slot claimed by touch id, and a spring integrated for
        // three hundred ticks afterwards.
        assert_deterministic(&enclosed_scene(), 300, fires_two_tongues);
    }

    #[test]
    fn the_firing_script_really_attaches_two_tongues() {
        // Without this the test above is vacuous. A script whose taps all
        // missed, or whose ids never claimed a slot, would agree with itself
        // perfectly and prove nothing about firing at all.
        let trace = record(&enclosed_scene(), 300, fires_two_tongues);
        assert!(
            trace[21].tongues.len() == 1,
            "the first Gesture must have Attached by the tick after it: {:?}",
            trace[21].tongues
        );
        let last = trace.last().expect("a trace is never empty");
        assert_eq!(
            last.tongues.len(),
            2,
            "both Gestures must leave a Tongue Attached: {:?}",
            last.tongues
        );
        assert!(
            last.tongues.iter().all(|t| t.to.is_finite()),
            "an Anchor went non-finite: {:?}",
            last.tongues
        );
    }

    #[test]
    fn hostile_input_is_deterministic_too() {
        // Three fingers hammering every frame. The slot cap, the refusal to
        // re-fire a held id, and the ray cast all run on every single tick
        // here, so anything order-dependent in the Gesture tracker has three
        // hundred chances to disagree with itself.
        assert_deterministic(&enclosed_scene(), 300, hammers_three_fingers);
        let trace = record(&enclosed_scene(), 300, hammers_three_fingers);
        assert_eq!(
            trace.last().expect("a trace is never empty").tongues.len(),
            crate::MAX_TONGUES,
            "three fingers must leave exactly two Tongues"
        );
    }

    #[test]
    fn the_harness_notices_a_divergence() {
        // A comparison that cannot fail is worse than no comparison, because it
        // reports success forever. Two genuinely different runs must be
        // reported as different.
        let mut other = scene();
        other.player_spawn.x += 0.5;
        let a = record(&scene(), 60, no_input);
        let b = record(&other, 60, no_input);
        assert!(
            first_divergence(&a, &b).is_some(),
            "the harness failed to notice two different scenes"
        );
    }

    #[test]
    fn running_the_catch_up_ticks_in_batches_does_not_change_the_outcome() {
        // The Shell runs between one and MAX_CATCHUP_TICKS ticks per frame
        // depending on the device, so a `World` whose result depended on how
        // its ticks were grouped would make difficulty a function of hardware.
        // A Sim Tick must be a Sim Tick.
        let level = scene();
        let straight = record(&level, 300, no_input);

        let mut world = World::from_level(&level);
        let mut batched = Vec::with_capacity(301);
        batched.push(world.snapshot().clone());
        let mut done = 0;
        for batch in [1usize, 2, 5, 3, 4].iter().cycle() {
            for _ in 0..*batch {
                if done == 300 {
                    break;
                }
                world.step();
                batched.push(world.snapshot().clone());
                done += 1;
            }
            if done == 300 {
                break;
            }
        }

        assert!(
            first_divergence(&straight, &batched).is_none(),
            "{}",
            first_divergence(&straight, &batched).unwrap_or_default()
        );
    }
}
