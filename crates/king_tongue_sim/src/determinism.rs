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

/// Applies several touches in one call, each aimed relative to the hero.
///
/// One call and not one per finger, because that is how the platform reports a
/// frame: every live touch arrives together. Two fingers that landed in
/// separate calls would never exercise the ascending-id ordering that ADR-0006
/// makes the determinism rule.
fn aim_all(world: &mut World, touches: &[(u64, TouchPhase, Vec2)]) {
    let Some(pos) = world.snapshot().body(world.hero_id()).map(|body| body.pos) else {
        return;
    };
    let samples: Vec<TouchSample> = touches
        .iter()
        .map(|(id, phase, offset)| TouchSample {
            id: *id,
            phase: *phase,
            pos: pos + *offset,
        })
        .collect();
    world.apply_touches(&samples);
}

/// Fires one Tongue up and to the left and, while that finger is still down, a
/// second up and to the right.
///
/// This is the script that makes the harness mean something for the first time.
/// Until there was an input path, the only thing it could assert was that a
/// hero falls the same way twice, which no integrator would get wrong. Firing
/// brings in the parts that actually could: a ray cast against terrain, slot
/// claiming keyed on touch ids, and a spring force whose Anchor is chosen from
/// simulation state.
///
/// The first finger is **held down** until after the second has landed, and
/// that is now load-bearing rather than incidental. Ticket #12 lifted it, which
/// worked only while a tap on an occupied slot was inert; under ADR-0006's
/// completed rule that second tap is a Release, so lifting first would end the
/// run with no Tongues at all. Two Tongues at once means two fingers at once,
/// which is exactly what ADR-0006 says dual Tongues are.
///
/// The phases are otherwise as deliberate as before. `Started` on one frame and
/// `Ended` or `Cancelled` on the next is exactly how long the platform makes
/// those visible, and `Stationary` is what a finger that is still down reports
/// every frame, so a script that re-fired on a hold would show up here as a
/// Tongue that kept re-Attaching.
pub fn fires_two_tongues(tick: u64, world: &mut World) {
    const UP_LEFT: Vec2 = Vec2::new(-2.0, 4.0);
    const UP_RIGHT: Vec2 = Vec2::new(3.0, 2.0);
    match tick {
        20 => aim_all(world, &[(1, TouchPhase::Started, UP_LEFT)]),
        // Well after the first, so the hero is Swinging on one Tongue when the
        // second Gesture arrives and the second Anchor depends on where that
        // Swing had carried it. Both fingers in one call, as a frame reports.
        90 => aim_all(
            world,
            &[
                (1, TouchPhase::Stationary, UP_LEFT),
                (2, TouchPhase::Started, UP_RIGHT),
            ],
        ),
        // Cancelled rather than Ended for the second, because the platform uses
        // it when the app loses focus and ADR-0006 forbids reading either as a
        // Release. Both Tongues must survive it.
        91 => aim_all(
            world,
            &[
                (1, TouchPhase::Ended, UP_LEFT),
                (2, TouchPhase::Cancelled, UP_RIGHT),
            ],
        ),
        21..=89 => aim_all(world, &[(1, TouchPhase::Stationary, UP_LEFT)]),
        _ => {}
    }
}

/// Fires a Tongue, Swings on it, Releases it with a second tap, and fires
/// again at whatever the ray finds from the new position.
///
/// Release is the case most likely to lose determinism in this module, for a
/// reason none of the other scripts reach: it changes the *number* of Tongues
/// mid-run in both directions, and the Anchor the third tap finds depends on
/// the entire ballistic arc the Release threw the hero along. A slot cleared in
/// the wrong order, or an Anchor re-resolved from a stale position, diverges
/// here and nowhere else.
///
/// Each tap is a separate finger that lifts on the following frame, which is
/// the discrete tap loop ADR-0006 describes: tap, Swing, tap, fly.
pub fn releases_mid_swing(tick: u64, world: &mut World) {
    const UP_LEFT: Vec2 = Vec2::new(-2.0, 4.0);
    const UP_RIGHT: Vec2 = Vec2::new(3.0, 3.0);
    match tick {
        10 => aim_all(world, &[(1, TouchPhase::Started, UP_LEFT)]),
        11 => aim_all(world, &[(1, TouchPhase::Ended, UP_LEFT)]),
        // Mid-Swing, so the hero leaves on a tangent carrying real momentum
        // rather than from a hang at rest.
        70 => aim_all(world, &[(2, TouchPhase::Started, UP_LEFT)]),
        71 => aim_all(world, &[(2, TouchPhase::Ended, UP_LEFT)]),
        // Re-fired from wherever the ballistic arc carried the hero, into a
        // slot that has been occupied, emptied, and is now filled again.
        140 => aim_all(world, &[(3, TouchPhase::Started, UP_RIGHT)]),
        141 => aim_all(world, &[(3, TouchPhase::Cancelled, UP_RIGHT)]),
        _ => {}
    }
}

/// Holds two Tongues at once on two overlapping fingers, braces between them
/// for a hundred ticks with a stray third finger hammering throughout, then
/// Releases one slot and keeps Swinging on the other.
///
/// The dual-Tongue run the ticket #14 AC asks for, and it is not the same trace
/// as [`fires_two_tongues`]: that script fires both and lifts both, so the two
/// pulls only ever sum. This one also takes one away, which is where a dual
/// trace can lose determinism and a single-Tongue one cannot. Two pulls summing
/// is order-independent arithmetic; *which* slot a later Gesture lands on is not,
/// because ADR-0006 decides it from ownership, so a frame read in a different
/// order would Release the other Tongue and every tick after it would disagree.
///
/// The phases are the platform's own. Finger 1 stays down for the whole run,
/// reporting `Stationary` every frame, which is the only way to hold two Tongues
/// at all: under ADR-0006's slot rule a tap, a lift, and another tap return to
/// slot 0 and Release it. Finger 2 lifts at tick 150 without Releasing anything,
/// and the tap that follows finds slot 1 owned by nobody and occupied by a
/// Tongue, so it Releases that one and leaves slot 0 Attached.
///
/// The stray finger between ticks 120 and 129 arrives on a **fresh id every
/// frame**, which is the hostile version: an id the tracker has never seen
/// cannot be refused for already owning a slot, so the only thing that can
/// discard it is the slot cap itself.
pub fn braces_between_two_anchors(tick: u64, world: &mut World) {
    const UP_LEFT: Vec2 = Vec2::new(-2.0, 4.0);
    const UP_RIGHT: Vec2 = Vec2::new(3.0, 2.0);
    const DOWN: Vec2 = Vec2::new(0.0, -3.0);
    const HOLD_1: (u64, TouchPhase, Vec2) = (1, TouchPhase::Stationary, UP_LEFT);
    const HOLD_2: (u64, TouchPhase, Vec2) = (2, TouchPhase::Stationary, UP_RIGHT);
    match tick {
        20 => aim_all(world, &[(1, TouchPhase::Started, UP_LEFT)]),
        // The second finger lands while the first is still down, which is what
        // makes this two Tongues rather than a fire and a Release.
        40 => aim_all(world, &[HOLD_1, (2, TouchPhase::Started, UP_RIGHT)]),
        // The end of the second Gesture, not a Release: both Tongues stay.
        150 => aim_all(world, &[HOLD_1, (2, TouchPhase::Ended, UP_RIGHT)]),
        // Slot 0 is still owned by finger 1, so this lands on slot 1 and
        // Releases it. Its aim is irrelevant to a Release and deliberately
        // points somewhere neither Tongue is Attached.
        151 => aim_all(world, &[HOLD_1, (4, TouchPhase::Started, DOWN)]),
        121..=130 => aim_all(
            world,
            &[HOLD_1, HOLD_2, (100 + tick, TouchPhase::Started, DOWN)],
        ),
        21..=39 => aim_all(world, &[HOLD_1]),
        41..=149 => aim_all(world, &[HOLD_1, HOLD_2]),
        152.. => aim_all(world, &[HOLD_1, (4, TouchPhase::Stationary, DOWN)]),
        _ => {}
    }
}

/// Attaches to an Enemy, braces against terrain with a second Tongue so all
/// three bodies are coupled, then flings the Enemy out of the Kill Bound and
/// keeps Swinging on what is left.
///
/// The Movable Anchor run the ticket #17 AC asks for, and it is the richest
/// trace in this module: it is the only one where the **Tongue** count and the
/// **body** count both change mid-run, and where they change for connected
/// reasons. Three things in it can lose determinism and nothing earlier could.
///
/// A Movable Anchor is resolved from simulation state on every tick rather than
/// being a constant, so a divergence in the Enemy's position reaches the hero's
/// force on the same tick instead of waiting for the next tap. The three-body
/// phase makes both bodies' forces depend on each other, so anything that read
/// one body's half-updated state would show up as the pair's momentum differing
/// between runs. And ADR-0014's slot sweep runs after two separate removals, so
/// a sweep that ran at a different point in the tick on one pass than on another
/// would free the slot a tick early in one run and never in the other.
///
/// The fling is scripted rather than earned by a Slam. Both ways of losing a
/// body would free the slot, and a script where either could have happened
/// could not say which one had diverged. Nothing here carries enough momentum
/// to empty a sketched Enemy's 100 Toughness, so every removal in this trace is
/// the Kill Bound's.
pub fn swings_on_an_enemy(tick: u64, world: &mut World) {
    const UP: Vec2 = Vec2::new(0.0, 4.0);
    // Shallow on purpose. By tick 40 the mass-ratio pull has carried the hero
    // most of the way up to the Enemy, and a steeper aim from there leaves the
    // sketch through the open sky above the wall rather than meeting it, which
    // is a tap that Attaches nothing and a three-body phase that never happens.
    const UP_LEFT: Vec2 = Vec2::new(-4.0, 1.0);
    const HOLD_1: (u64, TouchPhase, Vec2) = (1, TouchPhase::Stationary, UP);
    const HOLD_2: (u64, TouchPhase, Vec2) = (2, TouchPhase::Stationary, UP_LEFT);
    match tick {
        // Straight up at the Enemy. The ray strikes it, so this is an Attach to
        // the body and not to the open sky behind it.
        10 => aim_all(world, &[(1, TouchPhase::Started, UP)]),
        // The second finger lands while the first is still down, which ADR-0006
        // makes the only way to hold two Tongues. One Movable Anchor and one
        // Fixed, which is the three-body system ADR-0003 describes.
        40 => aim_all(world, &[HOLD_1, (2, TouchPhase::Started, UP_LEFT)]),
        // Flung straight up and out of the bound. Scripted rather than left to
        // the pull, which carries the Enemy out of the sketch on its own by
        // tick 105, so that the tick the body count changes on is one this file
        // chose: a removal somewhere in the middle of a Swing could not say
        // whether the Kill Bound or an impact had taken it.
        60 => {
            let hero = world.hero_id();
            let enemy: Option<(crate::EntityId, Vec2)> = world
                .snapshot()
                .bodies
                .iter()
                .find(|b| b.id != hero)
                .map(|b| (b.id, b.pos));
            if let Some((id, pos)) = enemy {
                world.place(id, pos, Vec2::new(0.0, 90.0));
            }
            aim_all(world, &[HOLD_1, HOLD_2]);
        }
        11..=39 => aim_all(world, &[HOLD_1]),
        41..=59 | 61.. => aim_all(world, &[HOLD_1, HOLD_2]),
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
            if x.slot != y.slot || x.anchor != y.anchor {
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
    fn releasing_produces_identical_snapshot_sequences() {
        // The AC's determinism clause. Release is the only scripted event that
        // removes a Tongue, and what follows it is unconstrained ballistic
        // motion whose landing point feeds the next tap's ray cast, so a
        // divergence of a single bit at the moment of Release is amplified by
        // every tick after it rather than damped by the spring.
        assert_deterministic(&enclosed_scene(), 300, releases_mid_swing);
    }

    #[test]
    fn the_release_script_really_attaches_releases_and_re_attaches() {
        // Guards the test above from passing vacuously, the same way the firing
        // script is guarded. A script whose second tap silently fired a second
        // Tongue instead of Releasing the first would be perfectly
        // deterministic and would assert nothing about Release at all, which is
        // precisely the bug this ticket fixed.
        let trace = record(&enclosed_scene(), 300, releases_mid_swing);
        assert_eq!(
            trace[12].tongues.len(),
            1,
            "the first Gesture must have Attached: {:?}",
            trace[12].tongues
        );
        assert!(
            trace[72].tongues.is_empty(),
            "the second Gesture must have Released, leaving nothing: {:?}",
            trace[72].tongues
        );
        assert_eq!(
            trace[142].tongues.len(),
            1,
            "the third Gesture must have re-Attached: {:?}",
            trace[142].tongues
        );
        // And the slot is reused rather than the Tongue walking up the slots,
        // which is what a Release that merely hid the Tongue would look like.
        assert_eq!(
            trace[142].tongues[0].slot, 0,
            "a re-fire must come back to slot 0"
        );
    }

    #[test]
    fn bracing_between_two_anchors_produces_identical_snapshot_sequences() {
        // Ticket #14's determinism clause. Two pulls summing is arithmetic that
        // cannot care about order, but which slot the Gesture at tick 151
        // Releases is decided from ownership, and a frame read in a different
        // order would take the other Tongue away. From there the two runs are
        // Swinging on different Anchors and every later tick disagrees.
        assert_deterministic(&enclosed_scene(), 300, braces_between_two_anchors);
    }

    #[test]
    fn the_bracing_script_really_holds_two_tongues_and_then_one() {
        // Guards the test above from passing vacuously, which for a dual-Tongue
        // script is a live risk rather than a formality: ADR-0006's slot rule
        // means a script written as tap, lift, tap ends with *no* Tongues, and
        // it would agree with itself perfectly while asserting nothing about
        // holding two.
        let trace = record(&enclosed_scene(), 300, braces_between_two_anchors);
        let anchors = |tick: usize| -> Vec<_> {
            let mut a: Vec<_> = trace[tick]
                .tongues
                .iter()
                .map(|t| (t.slot, t.anchor))
                .collect();
            a.sort_by_key(|(slot, _)| *slot);
            a
        };

        assert_eq!(
            trace[21].tongues.len(),
            1,
            "the first Gesture must have Attached: {:?}",
            trace[21].tongues
        );
        assert_eq!(
            trace[41].tongues.len(),
            2,
            "two overlapping fingers must hold two Tongues: {:?}",
            trace[41].tongues
        );
        let braced = anchors(41);
        assert_ne!(
            braced[0].1, braced[1].1,
            "the two Tongues must be on different Anchors, or this is not a brace"
        );

        // The stray finger ran for ten frames in the middle of that, and
        // changed nothing at all: same count, same slots, same Anchors.
        assert_eq!(
            anchors(131),
            anchors(121),
            "a third finger disturbed a slot while two Tongues were held"
        );

        // Then one slot was Released and the other kept Swinging.
        assert_eq!(
            trace[152].tongues.len(),
            1,
            "the Gesture at tick 151 must Release exactly one Tongue: {:?}",
            trace[152].tongues
        );
        assert_eq!(
            trace[152].tongues[0].slot, 0,
            "slot 1 was the one Released, so slot 0 must be what is left"
        );
        assert_eq!(
            trace[152].tongues[0].anchor, braced[0].1,
            "the surviving Tongue must be on the Anchor it has had all along"
        );
        let last = trace.last().expect("a trace is never empty");
        assert_eq!(
            last.tongues.len(),
            1,
            "the run must end still Attached by one slot: {:?}",
            last.tongues
        );
        assert!(
            last.tongues
                .iter()
                .all(|t| t.to.is_finite() && t.from.is_finite()),
            "a Tongue endpoint went non-finite: {:?}",
            last.tongues
        );
    }

    /// A hero with an Enemy four units straight above it, walls on both sides
    /// for a Fixed Anchor, and **open sky above**, so an Enemy flung upward
    /// leaves the Kill Bound instead of stopping against a ceiling.
    ///
    /// Flush to the left margin, because `from_ascii` counts leading
    /// indentation as columns and the four unit gap is the whole point: at
    /// `REST_LENGTH` of 1 that is a Stretch of 3, so the Tongue is genuinely
    /// loaded rather than slack.
    fn enemy_overhead_scene() -> Level {
        Level::from_ascii(
            "\
#.........#
#....e....#
#.........#
#.........#
#.........#
#....@....#
###########
",
        )
    }

    #[test]
    fn swinging_on_an_enemy_produces_identical_snapshot_sequences() {
        // Ticket #17's determinism clause, over the Movable Anchor. Every
        // earlier script resolves its Anchors from level data or from a tap;
        // this one resolves one of them from another body's position on every
        // single tick, so a divergence anywhere in the Enemy's motion reaches
        // the hero's force immediately rather than waiting for the next tap.
        assert_deterministic(&enemy_overhead_scene(), 300, swings_on_an_enemy);
    }

    #[test]
    fn the_enemy_anchor_script_really_holds_a_movable_anchor_and_then_loses_it() {
        // Guards the test above from passing vacuously, and there are three
        // separate ways for it to be vacuous here: the tap could have missed
        // the Enemy and Attached to the sky behind it, the second finger could
        // have failed to give a three-body phase, and the fling could have left
        // the Enemy inside the bound so the slot was never freed. Each is
        // checked, because any one of them leaves a perfectly deterministic run
        // that proves nothing about a Movable Anchor.
        let trace = record(&enemy_overhead_scene(), 300, swings_on_an_enemy);
        let anchors = |tick: usize| -> Vec<crate::Anchor> {
            let mut a: Vec<_> = trace[tick]
                .tongues
                .iter()
                .map(|t| (t.slot, t.anchor))
                .collect();
            a.sort_by_key(|(slot, _)| *slot);
            a.into_iter().map(|(_, anchor)| anchor).collect()
        };

        let attached = anchors(11);
        assert!(
            matches!(attached.as_slice(), [crate::Anchor::Movable(_)]),
            "the first Gesture must Attach to the Enemy itself, got {attached:?}"
        );
        let braced = anchors(41);
        assert_eq!(
            braced.len(),
            2,
            "two overlapping fingers must hold two Tongues"
        );
        assert!(
            braced
                .iter()
                .any(|a| matches!(a, crate::Anchor::Movable(_)))
                && braced.iter().any(|a| matches!(a, crate::Anchor::Fixed(_))),
            "the three-body phase needs one Movable Anchor and one Fixed, got {braced:?}"
        );

        // The Enemy really left, and the Kill Bound is the only thing that
        // could have taken it: the removal lands on the tick the fling put it
        // outside the sketch, and a sketched Enemy's 100 Toughness is far above
        // anything in this run.
        let bodies = |tick: usize| trace[tick].bodies.len();
        assert_eq!(bodies(0), 2, "one hero and one Enemy");
        assert_eq!(
            bodies(62),
            2,
            "the Enemy must survive until the fling lands"
        );
        assert_eq!(bodies(63), 1, "the fling must despawn the Enemy");

        // And the slot went with it, on exactly that tick rather than a tick
        // later or never. Both halves matter: the Movable slot is free, and the
        // Fixed one beside it is untouched, so the sweep took the dead Anchor
        // and not simply every Tongue it could see.
        assert_eq!(
            anchors(62).len(),
            2,
            "both Tongues must still be Attached the tick before: {:?}",
            anchors(62)
        );
        let after = anchors(63);
        assert!(
            matches!(after.as_slice(), [crate::Anchor::Fixed(_)]),
            "the dead Anchor's slot must be freed and the Fixed one kept, got {after:?}"
        );
        assert_eq!(
            after,
            braced
                .iter()
                .copied()
                .filter(|a| matches!(a, crate::Anchor::Fixed(_)))
                .collect::<Vec<_>>(),
            "the surviving Tongue must be on the Anchor it has held all along"
        );
        // And it stays free for the rest of the run rather than coming back.
        let last = trace.last().expect("a trace is never empty");
        assert!(
            !last
                .tongues
                .iter()
                .any(|t| matches!(t.anchor, crate::Anchor::Movable(_))),
            "a Movable Anchor reappeared after its body was gone: {:?}",
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
